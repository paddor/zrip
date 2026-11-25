//! Sans-io frame state shared by [`FrameDecoder`](crate::streaming::FrameDecoder)
//! and [`PushDecoder`](crate::push::PushDecoder).
//!
//! The caller finds frame headers, block headers, block bodies and checksums
//! in its input and hands each one over as a byte slice. This module checks
//! and decodes them, keeps the window history, and puts each block's output
//! in [`FrameState::block`].
#![forbid(unsafe_code)]

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::BlockDecodeWorkspace;
use crate::decode_sequences_dispatch_room;
use crate::exec::SequenceOutputScope;
use crate::fast_vec::WILDCOPY_OVERLENGTH;
use crate::literals::decode_literals_ws;
use crate::sequences::{SequenceDecodeTables, parse_sequence_count, parse_sequence_tables_ws};
use zrip_core::block::{BlockHeader, BlockType};
use zrip_core::dict::Dictionary;
use zrip_core::error::DecompressError;
use zrip_core::frame::header::parse_frame_header;
use zrip_core::frame::{MAX_BLOCK_SIZE, MAX_WINDOW_SIZE};
use zrip_core::xxhash::Xxh64State;

/// Magic plus frame header descriptor: enough to size the rest of the header.
pub(crate) const FRAME_PREFIX_SIZE: usize = 5;
/// Largest zstd frame header, magic included.
#[cfg_attr(not(feature = "std"), allow(dead_code))]
pub(crate) const MAX_FRAME_HEADER_SIZE: usize = 18;
/// Block header: last-block flag, block type and 21-bit size, little-endian.
pub(crate) const BLOCK_HEADER_SIZE: usize = 3;
/// Content checksum: the low 32 bits of XXH64.
pub(crate) const CHECKSUM_SIZE: usize = 4;
/// Skippable frame magic plus its 4-byte length.
pub(crate) const SKIPPABLE_HEADER_SIZE: usize = 8;
/// The most history a frame header reserves for its content size. A header
/// alone, which may declare any size, reserves no more; a frame's output up
/// to here then backs the rest (`start_block`).
const HEADER_RESERVE: usize = 8 << 20;

/// Returns true if `magic` starts a skippable frame:
/// `0x184D2A50..=0x184D2A5F`.
pub(crate) fn is_skippable_magic(magic: u32) -> bool {
    (magic & 0xFFFF_FFF0) == 0x184D_2A50
}

/// Total frame header size, magic included, from the magic and descriptor.
pub(crate) fn frame_header_size(prefix: &[u8]) -> usize {
    // Descriptor bits: 7-6 content size flag, 5 single segment, 1-0
    // dictionary ID flag. Single-segment frames have no window byte, and
    // their flag 0 means a 1-byte content size instead of none.
    let descriptor = prefix[4];
    let single_segment = (descriptor & 0x20) != 0;
    let dict_id_size = [0, 1, 2, 4][usize::from(descriptor & 0x03)];
    let fcs_size = match descriptor >> 6 {
        0 if single_segment => 1,
        0 => 0,
        1 => 2,
        2 => 4,
        _ => 8,
    };
    FRAME_PREFIX_SIZE + usize::from(!single_segment) + dict_id_size + fcs_size
}

/// Appends `data` to `history[..*len]`, over the room after it.
fn append(history: &mut Vec<u8>, len: &mut usize, data: &[u8]) {
    let start = *len;
    let (over, past) = data.split_at(data.len().min(history.len() - start));
    history[start..start + over.len()].copy_from_slice(over);
    history.extend_from_slice(past);
    *len += data.len();
}

/// What follows a block.
pub(crate) enum AfterBlock {
    /// Another block header.
    Block,
    /// The 4-byte content checksum, then the frame ends.
    Checksum,
    /// Nothing: the frame ended.
    FrameEnd,
}

/// Decoder state for a stream of frames: tables, repeat offsets, limits,
/// the checksum, and the window history the blocks decode into.
pub(crate) struct FrameState {
    ws: Box<BlockDecodeWorkspace>,
    seq_tables: SequenceDecodeTables,
    rep_offsets: [u32; 3],
    hasher: Option<Xxh64State>,
    /// Output limit for the whole stream, in bytes.
    max_output: usize,
    /// Explicit window cap. `None` means `MAX_WINDOW_SIZE`.
    max_window: Option<u64>,
    /// Output of all frames so far, checked against `max_output`.
    bytes_output: u64,
    frame_content_size: Option<u64>,
    /// Output of the current frame so far.
    frame_bytes: u64,
    dict: Option<Dictionary>,
    /// Window history, then the current block's output from `block_start`,
    /// up to `history_len`. Blocks decode straight into it, so their output
    /// needs no copy. Bytes past `history_len` are room earlier blocks
    /// zeroed or wrote: kept, so a block zeroes only room no block had.
    history: Vec<u8>,
    history_len: usize,
    block_start: usize,
    /// The current frame's window, in bytes.
    window_size: usize,
    /// Set once a block wrote more than its first room: later blocks get
    /// all the room they may use.
    full_room: bool,
}

impl FrameState {
    /// A state for a stream of at most `max_output` output bytes, with an
    /// optional dictionary for every frame.
    pub(crate) fn new(dict: Option<Dictionary>, max_output: usize) -> Self {
        Self {
            ws: Box::new(BlockDecodeWorkspace::new()),
            seq_tables: SequenceDecodeTables::new_default(),
            // RFC 8878's initial repeat offsets.
            rep_offsets: [1, 4, 8],
            hasher: None,
            max_output,
            max_window: None,
            bytes_output: 0,
            frame_content_size: None,
            frame_bytes: 0,
            dict,
            history: Vec::new(),
            history_len: 0,
            block_start: 0,
            window_size: 0,
            full_room: false,
        }
    }

    /// Sets the largest window to accept, replacing the default cap.
    pub(crate) fn set_max_window(&mut self, bytes: u64) {
        self.max_window = Some(bytes);
    }

    /// Forgets the current stream, keeping allocations.
    pub(crate) fn reset(&mut self) {
        self.rep_offsets = [1, 4, 8];
        self.seq_tables.clear_repeat_flags();
        self.ws.reset_huffman_state();
        self.hasher = None;
        self.bytes_output = 0;
        self.frame_content_size = None;
        self.frame_bytes = 0;
        self.history_len = 0;
        self.block_start = 0;
        self.window_size = 0;
        self.full_room = false;
    }

    /// The current block's output so far.
    pub(crate) fn block(&self) -> &[u8] {
        &self.history[self.block_start..self.history_len]
    }

    /// Starts a frame from its full header, magic included.
    pub(crate) fn begin_frame(&mut self, header: &[u8]) -> Result<(), DecompressError> {
        let header = parse_frame_header(header)?;

        if let Some(frame_dict_id) = header.dict_id {
            match &self.dict {
                Some(d) if d.id() == frame_dict_id => {}
                Some(d) => {
                    return Err(DecompressError::DictMismatch {
                        expected: frame_dict_id,
                        got: d.id(),
                    });
                }
                None => return Err(DecompressError::DictRequired),
            }
        }

        let too_large = |max| DecompressError::WindowTooLarge {
            requested: header.window_size,
            max,
        };
        let window_size = match self.max_window {
            // An explicit cap replaces the default, up or down.
            Some(max) if header.window_size > max => return Err(too_large(max)),
            Some(_) => {
                usize::try_from(header.window_size).map_err(|_| too_large(usize::MAX as u64))?
            }
            // As C zstd's `ZSTD_d_windowLogMax`, also for single-segment
            // frames, whose window is their content size.
            None if header.window_size > MAX_WINDOW_SIZE => {
                return Err(too_large(MAX_WINDOW_SIZE));
            }
            None => header.window_size as usize,
        };

        // As in C zstd's streaming decoder, a content size over the output
        // limit fails only once the output passes the limit.

        self.window_size = window_size;
        self.history_len = 0;
        self.frame_content_size = header.frame_content_size;
        self.frame_bytes = 0;
        self.hasher = header.content_checksum.then(|| Xxh64State::new(0));

        if let Some(ref d) = self.dict {
            self.rep_offsets = *d.rep_offsets();
            append(&mut self.history, &mut self.history_len, d.content());
            self.seq_tables.reset_default();
            self.seq_tables.load_dict(d);
            self.ws.seq_tables_replaced();
            self.ws.reset_huffman_state();
            if let Some((t, l)) = d.huf_table() {
                self.ws.huf_table.clear();
                self.ws.huf_table.extend_from_slice(t);
                self.ws.huf_table_log = l;
                self.ws.huf_valid = true;
                self.ws.huffman_table_replaced();
            }
        } else {
            // A frame starts without repeat tables. Predefined and
            // compressed modes rebuild what they use, as in one-shot decoding.
            self.rep_offsets = [1, 4, 8];
            self.seq_tables.clear_repeat_flags();
            self.ws.reset_huffman_state();
        }
        self.block_start = self.history_len;

        // A known content size bounds the history, so reserve it once
        // instead of growing block by block: up to `HEADER_RESERVE`, and
        // no more than the stream may still output.
        if let Some(fcs) = self.frame_content_size {
            let left = (self.max_output as u64).saturating_sub(self.bytes_output);
            let fcs = usize::try_from(fcs.min(left)).unwrap_or(usize::MAX);
            let room = fcs
                .min(HEADER_RESERVE)
                .min(self.most_history() - self.history_len)
                + WILDCOPY_OVERLENGTH;
            let target = self.history_len + room;
            self.history
                .reserve_exact(target.saturating_sub(self.history.len()));
        }
        Ok(())
    }

    /// Checks a parsed block header against `Block_Maximum_Size`: the
    /// window, at most 128 KiB. As in C zstd, it bounds the compressed size
    /// too.
    ///
    /// As in C zstd's streaming decoder, an RLE block counts as one byte
    /// here, and its output is held to the maximum, too.
    pub(crate) fn check_block_header(&self, header: BlockHeader) -> Result<(), DecompressError> {
        let size = header.block_size as usize;
        let rle = header.block_type == BlockType::Rle;
        let max = self.block_size_max();
        if size.max(usize::from(rle)) > max {
            return Err(DecompressError::BlockTooLarge);
        }
        Ok(())
    }

    /// `Block_Maximum_Size`: the window, at most 128 KiB.
    fn block_size_max(&self) -> usize {
        self.window_size.min(MAX_BLOCK_SIZE)
    }

    /// The most a block may write: `Block_Maximum_Size`, or less once a
    /// known content size is nearly reached. A small window caps it, so a
    /// stream on a 64 KiB window reserves and zeroes 64 KiB, not 128.
    fn max_block(&self) -> usize {
        let max = self.block_size_max();
        match self.frame_content_size {
            Some(fcs) => fcs.saturating_sub(self.frame_bytes).min(max as u64) as usize,
            None => max,
        }
    }

    /// The most `history` holds: two windows, or the dictionary and a window,
    /// plus a block and its copy slack.
    fn most_history(&self) -> usize {
        let dict_len = self.dict.as_ref().map_or(0, |d| d.content().len());
        (2 * self.window_size).max(dict_len + self.window_size)
            + self.block_size_max()
            + WILDCOPY_OVERLENGTH
    }

    /// Starts a block that writes `room` bytes: a raw or RLE block's size.
    /// A compressed block passes 0 and sizes its room when it decodes. The
    /// previous block's output must be delivered: this may drop history
    /// older than a window.
    ///
    /// The history keeps up to two windows and then drops the older one, so
    /// the `memmove` happens once per window of output, not once per block.
    /// Dictionary content stays reachable until the frame has produced a full
    /// window.
    pub(crate) fn start_block(&mut self, room: usize) {
        let window = self.window_size;
        if self.frame_bytes >= window as u64 && self.history_len > 2 * window {
            let start = self.history_len - window;
            self.history.copy_within(start..self.history_len, 0);
            self.history_len = window;
        }
        self.block_start = self.history_len;
        self.reserve_block(room);
    }

    /// Reserves `room` bytes of block output, plus copy slack.
    ///
    /// Capacity grows like `Vec`, with the output, but stops at the most the
    /// history can hold. Plain doubling would reserve four windows.
    ///
    /// A known content size bounds the history too. Past what its header
    /// reserved (`HEADER_RESERVE`), a frame grows once to the rest of it,
    /// within the output limit: one copy of the history, not one per
    /// doubling.
    fn reserve_block(&mut self, room: usize) {
        let needed = self.block_start + room + WILDCOPY_OVERLENGTH;
        if needed > self.history.capacity() {
            let doubled = 2 * self.history.capacity();
            let target = match self.frame_content_size {
                Some(fcs) => {
                    let left = (self.max_output as u64).saturating_sub(self.bytes_output);
                    let rest = fcs.saturating_sub(self.frame_bytes).min(left);
                    let rest = usize::try_from(rest).unwrap_or(usize::MAX);
                    let most = self.most_history().min(
                        self.block_start
                            .saturating_add(rest)
                            .saturating_add(WILDCOPY_OVERLENGTH),
                    );
                    if self.history.capacity() >= HEADER_RESERVE {
                        most
                    } else {
                        doubled.min(most)
                    }
                }
                None => doubled.min(self.most_history()),
            };
            self.history
                .reserve_exact(target.max(needed).saturating_sub(self.history.len()));
        }
    }

    /// Appends raw block bytes, which may arrive in pieces.
    pub(crate) fn extend_block(&mut self, data: &[u8]) {
        append(&mut self.history, &mut self.history_len, data);
    }

    /// Adds the next `size` bytes to the history and returns them. They
    /// hold whatever the room held: the caller overwrites them.
    fn grow(&mut self, size: usize) -> &mut [u8] {
        let start = self.history_len;
        let end = start + size;
        if self.history.len() < end {
            self.history.resize(end, 0);
        }
        self.history_len = end;
        &mut self.history[start..end]
    }

    /// Makes room for a whole raw block and returns it to fill.
    #[cfg_attr(not(feature = "std"), allow(dead_code))]
    pub(crate) fn raw_block(&mut self, size: usize) -> &mut [u8] {
        self.grow(size)
    }

    /// Appends `size` copies of `byte` as an RLE block's output.
    pub(crate) fn rle_block(&mut self, byte: u8, size: usize) {
        self.grow(size).fill(byte);
    }

    /// Drops the current block's output, after an error.
    pub(crate) fn discard_block(&mut self) {
        self.history_len = self.block_start;
    }

    /// Decodes a compressed block body. On error, the block has no output.
    pub(crate) fn decode_compressed(&mut self, block_data: &[u8]) -> Result<(), DecompressError> {
        let result = self.decode_compressed_inner(block_data);
        if result.is_err() {
            self.discard_block();
        }
        result
    }

    fn decode_compressed_inner(&mut self, block_data: &[u8]) -> Result<(), DecompressError> {
        // Matches reach the last window before the block. Until the frame has
        // produced a full window, all history stays reachable, so the whole
        // dictionary is.
        let output_base = if self.frame_bytes >= self.window_size as u64 {
            self.block_start.saturating_sub(self.window_size)
        } else {
            0
        };
        let max_block = self.max_block();

        let block_size_max = self.block_size_max();
        let (lit_consumed, lits_at) = decode_literals_ws(block_data, &mut self.ws, block_size_max)?;
        let remaining = &block_data[lit_consumed..];

        // As in C zstd, the sequence count is required, and a block without
        // sequences ends right after it.
        let (num_sequences, seq_count_size) = parse_sequence_count(remaining)?;
        if num_sequences == 0 {
            if remaining.len() != seq_count_size {
                return Err(DecompressError::CorruptSequences);
            }
            self.reserve_block(lits_at.len());
            let lits = lits_at.resolve(block_data, &self.ws.literal_buf);
            append(&mut self.history, &mut self.history_len, lits.as_slice());
            return Ok(());
        }

        let table_data = &remaining[seq_count_size..];
        let tables_consumed =
            parse_sequence_tables_ws(table_data, &mut self.seq_tables, &mut self.ws)?;
        let seq_data = &table_data[tables_consumed..];

        // The sequence loop zeroes the block's room up front. As in one-shot
        // decoding, the room starts at a guess from the block, or the room
        // earlier blocks left if that is more, so a fresh decoder on small
        // blocks zeroes and reserves little. A block that writes more starts
        // over with all the room it may use, and so do the stream's later
        // blocks: each one would start over too while the history grows,
        // which made a new decoder up to 18% slower on 1000-byte matches.
        let left = self
            .history
            .len()
            .saturating_sub(self.block_start + WILDCOPY_OVERLENGTH);
        let first_rep_offsets = self.rep_offsets;
        let mut max_block_output = if self.full_room {
            max_block
        } else {
            seq_data
                .len()
                .saturating_mul(crate::ROOM_PER_SEQUENCE_BYTE)
                .saturating_add(lits_at.len())
                .max(left)
                .min(max_block)
        };
        loop {
            self.reserve_block(max_block_output);
            // The block's output stays within one block, so `end_block`
            // reports a crossed output limit as `OutputTooSmall`.
            let scope = SequenceOutputScope {
                output_base,
                max_block_output,
                history: &[],
            };
            let result = decode_sequences_dispatch_room(
                seq_data,
                num_sequences,
                &mut self.seq_tables,
                &mut self.rep_offsets,
                lits_at.resolve(block_data, &self.ws.literal_buf),
                &mut self.history,
                &mut self.history_len,
                scope,
            );
            match result {
                Err(DecompressError::BlockTooLarge) if max_block_output < max_block => {
                    self.history_len = self.block_start;
                    // A lone sequence sets its repeat offsets before it runs.
                    self.rep_offsets = first_rep_offsets;
                    self.full_room = true;
                    max_block_output = max_block;
                }
                result => break result?,
            }
        }
        if self.history_len - self.block_start > self.block_size_max() {
            return Err(DecompressError::BlockTooLarge);
        }
        Ok(())
    }

    /// Accounts for the block: checksum, limits, and the frame content size.
    /// On error, the block has no output.
    pub(crate) fn end_block(&mut self, last: bool) -> Result<AfterBlock, DecompressError> {
        let result = self.account_block(last);
        if result.is_err() {
            self.discard_block();
        }
        result
    }

    fn account_block(&mut self, last: bool) -> Result<AfterBlock, DecompressError> {
        let block = &self.history[self.block_start..self.history_len];
        // As in C zstd, `Block_Maximum_Size` bounds every block's output.
        if block.len() > self.block_size_max() {
            return Err(DecompressError::BlockTooLarge);
        }
        if let Some(ref mut hasher) = self.hasher {
            hasher.update(block);
        }
        self.bytes_output += block.len() as u64;
        self.frame_bytes += block.len() as u64;
        if self.bytes_output > self.max_output as u64 {
            return Err(DecompressError::OutputTooSmall);
        }
        if let Some(fcs) = self.frame_content_size {
            let written = self.frame_bytes;
            if written > fcs || last && written != fcs {
                return Err(DecompressError::FrameSizeMismatch);
            }
        }
        if !last {
            return Ok(AfterBlock::Block);
        }
        Ok(if self.hasher.is_some() {
            AfterBlock::Checksum
        } else {
            AfterBlock::FrameEnd
        })
    }

    /// Checks the 4-byte content checksum that ends the frame.
    pub(crate) fn verify_checksum(&self, stored: &[u8]) -> Result<(), DecompressError> {
        let stored = u32::from_le_bytes([stored[0], stored[1], stored[2], stored[3]]);
        if let Some(ref hasher) = self.hasher {
            let got = (hasher.finish() & 0xFFFF_FFFF) as u32;
            if got != stored {
                return Err(DecompressError::ChecksumMismatch {
                    expected: stored,
                    got,
                });
            }
        }
        Ok(())
    }
}
