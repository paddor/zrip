//! Sans-io frame state shared by [`FrameDecoder`](crate::streaming::FrameDecoder)
//! and [`PushDecoder`](crate::push::PushDecoder).
//!
//! The caller finds frame headers, block headers, block bodies and checksums
//! in its input and hands each one over as a byte slice. This module checks
//! and decodes them, keeps the window history, and puts each block's output
//! in [`FrameState::output`].
#![forbid(unsafe_code)]

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::BlockDecodeWorkspace;
use crate::decode_sequences_dispatch;
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
pub(crate) const BLOCK_HEADER_SIZE: usize = 3;
pub(crate) const CHECKSUM_SIZE: usize = 4;
/// Skippable frame magic plus its 4-byte length.
pub(crate) const SKIPPABLE_HEADER_SIZE: usize = 8;

/// Returns true if `magic` starts a skippable frame.
pub(crate) fn is_skippable_magic(magic: u32) -> bool {
    (magic & 0xFFFF_FFF0) == 0x184D_2A50
}

/// Total frame header size, magic included, from the magic and descriptor.
pub(crate) fn frame_header_size(prefix: &[u8]) -> usize {
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

/// What follows a block.
pub(crate) enum AfterBlock {
    Block,
    Checksum,
    FrameEnd,
}

pub(crate) struct FrameState {
    ws: Box<BlockDecodeWorkspace>,
    seq_tables: SequenceDecodeTables,
    rep_offsets: [u32; 3],
    hasher: Option<Xxh64State>,
    max_output: usize,
    max_window: Option<u64>,
    bytes_output: usize,
    frame_content_size: Option<u64>,
    frame_bytes: usize,
    dict: Option<Dictionary>,
    /// Window history, then the current block's output from `block_start`.
    /// Blocks decode straight into it, so their output needs no copy.
    history: Vec<u8>,
    block_start: usize,
    window_size: usize,
}

impl FrameState {
    pub(crate) fn new(dict: Option<Dictionary>, max_output: usize) -> Self {
        Self {
            ws: Box::new(BlockDecodeWorkspace::new()),
            seq_tables: SequenceDecodeTables::new_default(),
            rep_offsets: [1, 4, 8],
            hasher: None,
            max_output,
            max_window: None,
            bytes_output: 0,
            frame_content_size: None,
            frame_bytes: 0,
            dict,
            history: Vec::new(),
            block_start: 0,
            window_size: 0,
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
        self.history.clear();
        self.block_start = 0;
        self.window_size = 0;
    }

    /// The current block's output so far.
    pub(crate) fn block(&self) -> &[u8] {
        &self.history[self.block_start..]
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
            // A single-segment frame's window is its content size, which
            // `max_output` already bounds.
            None if header.window_size > MAX_WINDOW_SIZE && header.single_segment => {
                MAX_WINDOW_SIZE as usize
            }
            None if header.window_size > MAX_WINDOW_SIZE => {
                return Err(too_large(MAX_WINDOW_SIZE));
            }
            None => header.window_size as usize,
        };

        // The limit spans the stream, so earlier frames' output counts.
        if let Some(fcs) = header.frame_content_size
            && fcs > self.max_output.saturating_sub(self.bytes_output) as u64
        {
            return Err(DecompressError::OutputTooSmall);
        }

        self.window_size = window_size;
        self.history.clear();
        self.frame_content_size = header.frame_content_size;
        self.frame_bytes = 0;
        self.hasher = header.content_checksum.then(|| Xxh64State::new(0));

        if let Some(ref d) = self.dict {
            self.rep_offsets = *d.rep_offsets();
            self.history.extend_from_slice(d.content());
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
        self.block_start = self.history.len();

        // A known content size bounds the history, so reserve it once
        // instead of growing block by block.
        if let Some(fcs) = self.frame_content_size {
            let fcs = usize::try_from(fcs).unwrap_or(usize::MAX);
            let room = fcs.min(self.most_history() - self.history.len()) + WILDCOPY_OVERLENGTH;
            self.history.reserve_exact(room);
        }
        Ok(())
    }

    /// Checks a parsed block header against the format's size limits.
    pub(crate) fn check_block_header(header: BlockHeader) -> Result<(), DecompressError> {
        match header.block_type {
            BlockType::Raw | BlockType::Rle if header.block_size as usize > MAX_BLOCK_SIZE => {
                Err(DecompressError::BlockTooLarge)
            }
            _ => Ok(()),
        }
    }

    /// The most a block may write: 128 KiB, or less once a known content
    /// size is nearly reached.
    fn max_block(&self) -> usize {
        match self.frame_content_size {
            Some(fcs) => fcs
                .saturating_sub(self.frame_bytes as u64)
                .min(MAX_BLOCK_SIZE as u64) as usize,
            None => MAX_BLOCK_SIZE,
        }
    }

    /// The most `history` holds: two windows, or the dictionary and a window,
    /// plus a block and its copy slack.
    fn most_history(&self) -> usize {
        let dict_len = self.dict.as_ref().map_or(0, |d| d.content().len());
        (2 * self.window_size).max(dict_len + self.window_size)
            + MAX_BLOCK_SIZE
            + WILDCOPY_OVERLENGTH
    }

    /// Starts a block. The previous block's output must be delivered: this
    /// may drop history older than a window.
    ///
    /// The history keeps up to two windows and then drops the older one, so
    /// the `memmove` happens once per window of output, not once per block.
    /// Dictionary content stays reachable until the frame has produced a full
    /// window. Capacity grows like `Vec` but stops at the most the history can
    /// hold. Plain doubling would reserve four windows.
    pub(crate) fn start_block(&mut self) {
        let window = self.window_size;
        if self.frame_bytes >= window && self.history.len() > 2 * window {
            let start = self.history.len() - window;
            self.history.copy_within(start.., 0);
            self.history.truncate(window);
        }
        self.block_start = self.history.len();
        let needed = self.block_start + self.max_block() + WILDCOPY_OVERLENGTH;
        if needed > self.history.capacity() {
            let target = (2 * self.history.capacity())
                .min(self.most_history())
                .max(needed);
            self.history.reserve_exact(target - self.history.len());
        }
    }

    /// Appends raw block bytes, which may arrive in pieces.
    pub(crate) fn extend_block(&mut self, data: &[u8]) {
        self.history.extend_from_slice(data);
    }

    /// Makes room for a whole raw block and returns it to fill.
    #[cfg_attr(not(feature = "std"), allow(dead_code))]
    pub(crate) fn raw_block(&mut self, size: usize) -> &mut [u8] {
        let start = self.history.len();
        self.history.resize(start + size, 0);
        &mut self.history[start..]
    }

    pub(crate) fn rle_block(&mut self, byte: u8, size: usize) {
        let end = self.history.len() + size;
        self.history.resize(end, byte);
    }

    /// Drops the current block's output, after an error.
    pub(crate) fn discard_block(&mut self) {
        self.history.truncate(self.block_start);
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
        let output_base = if self.frame_bytes >= self.window_size {
            self.block_start.saturating_sub(self.window_size)
        } else {
            0
        };
        let max_block_output = self.max_block();

        let (lit_consumed, lits_at) = decode_literals_ws(block_data, &mut self.ws)?;
        let remaining = &block_data[lit_consumed..];

        if remaining.is_empty() {
            let lits = lits_at.resolve(block_data, &self.ws.literal_buf);
            self.history.extend_from_slice(lits.as_slice());
            return Ok(());
        }

        let (num_sequences, seq_count_size) = parse_sequence_count(remaining)?;
        if num_sequences == 0 {
            let lits = lits_at.resolve(block_data, &self.ws.literal_buf);
            self.history.extend_from_slice(lits.as_slice());
            return Ok(());
        }

        let table_data = &remaining[seq_count_size..];
        let tables_consumed =
            parse_sequence_tables_ws(table_data, &mut self.seq_tables, &mut self.ws)?;
        let seq_data = &table_data[tables_consumed..];

        // The block's output stays within one block, so `end_block` reports a
        // crossed output limit as `OutputTooSmall`.
        let scope = SequenceOutputScope {
            output_base,
            max_block_output,
            history: &[],
        };
        decode_sequences_dispatch(
            seq_data,
            num_sequences,
            &mut self.seq_tables,
            &mut self.rep_offsets,
            lits_at.resolve(block_data, &self.ws.literal_buf),
            &mut self.history,
            scope,
        )?;
        if self.history.len() - self.block_start > MAX_BLOCK_SIZE {
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
        let block = &self.history[self.block_start..];
        if let Some(ref mut hasher) = self.hasher {
            hasher.update(block);
        }
        self.bytes_output += block.len();
        self.frame_bytes += block.len();
        if self.bytes_output > self.max_output {
            return Err(DecompressError::OutputTooSmall);
        }
        if let Some(fcs) = self.frame_content_size {
            let written = self.frame_bytes as u64;
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
