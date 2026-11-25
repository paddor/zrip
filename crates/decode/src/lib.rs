//! zstd (RFC 8878) decoder for zrip: one-shot, reusable-context, streaming
//! (`std::io::Read`) and push (sans-io) decoding. Internal crate; use it
//! through the `zrip` facade.
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(optimize_attribute))]
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;

/// Evaluates `$body` compiled for x86-64-v3 (AVX2, BMI1/2, LZCNT) when the CPU
/// supports it, and compiled for the build target otherwise.
///
/// Use it as a whole function body around a call to an `#[inline(always)]`
/// implementation, so the implementation is duplicated into both contexts.
/// `fearless_simd` selects the level at runtime and provides the safe
/// target-feature entry point.
#[cfg(all(
    feature = "std",
    feature = "simd",
    target_arch = "x86_64",
    not(target_feature = "avx2")
))]
macro_rules! simd_body {
    ($body:expr) => {
        match fearless_simd::Level::new().as_avx2() {
            Some(avx2) => fearless_simd::Simd::vectorize(
                avx2,
                #[inline(always)]
                || $body,
            ),
            None => $body,
        }
    };
}

#[cfg(not(all(
    feature = "std",
    feature = "simd",
    target_arch = "x86_64",
    not(target_feature = "avx2")
)))]
macro_rules! simd_body {
    ($body:expr) => {
        $body
    };
}

#[cfg(feature = "alloc")]
pub mod context;
pub(crate) mod exec;
pub(crate) mod fast_vec;
#[cfg(feature = "alloc")]
pub(crate) mod frame_state;
pub(crate) mod literals;
#[cfg(feature = "alloc")]
pub mod push;
pub(crate) mod seq_table;
pub(crate) mod sequences;
#[cfg(feature = "std")]
pub mod streaming;

#[cfg(feature = "alloc")]
use alloc::boxed::Box;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::exec::{
    SequenceOutputScope, decode_execute_sequences, decode_execute_sequences_room,
    decode_execute_single_sequence,
};
use crate::fast_vec::{Literals, WILDCOPY_OVERLENGTH};
use crate::literals::decode_literals_ws;
use crate::sequences::{SequenceDecodeTables, parse_sequence_count, parse_sequence_tables_ws};
use zrip_core::block::{BlockType, parse_block_header};
use zrip_core::error::DecompressError;
use zrip_core::frame::MAX_WINDOW_SIZE;
#[cfg(feature = "alloc")]
use zrip_core::frame::header::parse_frame_header_after_magic;
use zrip_core::frame::header::{FrameHeader, parse_frame_header};
use zrip_core::huffman::HuffmanDecodeEntry;
use zrip_core::xxhash::Xxh64State;

/// Block decoding scratch and table caches, kept across blocks and frames
/// so repeated decodes neither reallocate nor rebuild unchanged tables.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct BlockDecodeWorkspace {
    /// Literals decoded out of the block, then `LITERAL_OVERREAD` zero
    /// bytes of slack for the chunked copies.
    pub literal_buf: Vec<u8>,
    /// Huffman decode table for compressed and treeless literals.
    pub huf_table: Vec<HuffmanDecodeEntry>,
    /// Table log of `huf_table`: its longest code, in bits.
    pub huf_table_log: u8,
    /// Two-symbol rows for `huf_table`, built on first use.
    pub huf_pairs: Vec<u32>,
    /// Whether `huf_pairs` describes the current `huf_table`.
    pub huf_pairs_valid: bool,
    /// Single-symbol rows of the 4-stream fast loop for `huf_table`, built
    /// on first use.
    pub huf_rows: Vec<u16>,
    /// Whether `huf_rows` describes the current `huf_table`.
    pub huf_rows_valid: bool,
    /// `huf_table` came from this frame or its dictionary, so treeless
    /// literals may use it. Cleared at each frame start.
    pub huf_valid: bool,
    /// Scratch: `huf_weights` plus the implied last weight.
    pub huf_all_weights: Vec<u8>,
    /// Weights parsed from the current Huffman tree description.
    pub huf_weights: Vec<u8>,
    /// Weights `huf_table` was built from. Equal weights skip the rebuild.
    pub huf_last_weights: Vec<u8>,
    /// Whether `huf_last_weights` describes `huf_table`.
    pub huf_last_weights_valid: bool,
    /// Raw bytes of the last Huffman tree description. Equal bytes skip
    /// parsing the weights.
    pub huf_last_header: Vec<u8>,
    /// Whether `huf_last_header` describes `huf_table`.
    pub huf_last_header_valid: bool,
    /// Scratch: normalized counts of an FSE table description.
    pub fse_dist: Vec<i16>,
    /// Scratch: built FSE decode rows, for sequence tables and Huffman
    /// weights.
    pub fse_build_buf: Vec<zrip_core::fse::FseDecodeEntry>,
    /// One-shot sequence tables, kept between frames to avoid reallocating.
    /// `None` while a frame holds them, or after it failed.
    pub seq_tables: Option<Box<SequenceDecodeTables>>,
    /// Raw bytes of the last sequence table header without repeat modes:
    /// the key of `seq_table_cache`.
    pub seq_table_header: Vec<u8>,
    /// The tables `seq_table_header` decodes to.
    pub seq_table_cache: Option<Box<SequenceDecodeTables>>,
    /// The live tables still equal `seq_table_cache`, so a cache hit copies
    /// only the repeat flags.
    pub seq_table_cache_tables_current: bool,
    /// The context dictionary's sequence tables, built once.
    pub cached_dict_tables: Option<Box<SequenceDecodeTables>>,
    /// The context dictionary's repeat offsets.
    pub cached_dict_rep: [u32; 3],
    /// The context dictionary's Huffman table and table log.
    pub cached_dict_huf: Option<(Vec<HuffmanDecodeEntry>, u8)>,
    /// `huf_table` and `seq_tables` still hold the cached dictionary's
    /// tables, unchanged since a frame restored them, so the next frame
    /// needs no copy. Every write to them clears these.
    pub huf_is_dict: bool,
    /// See `huf_is_dict`.
    pub seq_is_dict: bool,
}

impl BlockDecodeWorkspace {
    /// An empty workspace. Tables are built on first use.
    pub(crate) fn new() -> Self {
        Self {
            literal_buf: Vec::new(),
            huf_table: Vec::new(),
            huf_table_log: 0,
            huf_pairs: Vec::new(),
            huf_pairs_valid: false,
            huf_rows: Vec::new(),
            huf_rows_valid: false,
            huf_valid: false,
            huf_all_weights: Vec::new(),
            huf_weights: Vec::new(),
            huf_last_weights: Vec::new(),
            huf_last_weights_valid: false,
            huf_last_header: Vec::new(),
            huf_last_header_valid: false,
            fse_dist: Vec::new(),
            fse_build_buf: Vec::new(),
            // Built on first use; streaming decoders keep their own tables.
            seq_tables: None,
            seq_table_header: Vec::new(),
            seq_table_cache: None,
            seq_table_cache_tables_current: false,
            cached_dict_tables: None,
            // RFC 8878's initial repeat offsets.
            cached_dict_rep: [1, 4, 8],
            cached_dict_huf: None,
            huf_is_dict: false,
            seq_is_dict: false,
        }
    }

    /// Starts a frame without a Huffman table, so treeless literals fail
    /// until one is set. Keeps the header and weight caches.
    pub(crate) fn reset_huffman_state(&mut self) {
        self.huf_valid = false;
    }

    /// Records that `huf_table` was replaced, so the cached Huffman header and
    /// weights no longer describe it.
    pub(crate) fn huffman_table_replaced(&mut self) {
        self.huf_is_dict = false;
        self.huf_pairs_valid = false;
        self.huf_rows_valid = false;
        self.huf_last_weights_valid = false;
        self.huf_last_header_valid = false;
    }

    /// Records that the live sequence tables were replaced and no longer
    /// match `seq_table_cache`, so the next cache hit copies the cached
    /// tables instead of only their repeat flags.
    pub(crate) fn seq_tables_replaced(&mut self) {
        self.seq_is_dict = false;
        self.seq_table_cache_tables_current = false;
    }

    /// Builds `dict`'s sequence tables and copies its Huffman table once,
    /// for every frame that uses it.
    #[cfg(feature = "alloc")]
    pub(crate) fn cache_dict(&mut self, dict: &zrip_core::dict::Dictionary) {
        let mut st = SequenceDecodeTables::new_default();
        st.load_dict(dict);
        self.cached_dict_tables = Some(Box::new(st));
        self.cached_dict_rep = *dict.rep_offsets();
        if let Some((t, l)) = dict.huf_table() {
            self.cached_dict_huf = Some((t.to_vec(), l));
        }
    }
}

/// Size of the skippable frame at the start of `data`, its 8-byte header
/// included, or `None` if `data` does not start with a whole one.
pub(crate) fn skip_skippable_frame(data: &[u8]) -> Option<usize> {
    if data.len() < 8 {
        return None;
    }
    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    // Skippable magics are `0x184D2A50..=0x184D2A5F`.
    if (magic & 0xFFFF_FFF0) != 0x184D_2A50 {
        return None;
    }
    let frame_size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let total = frame_size.checked_add(8)?;
    if total > data.len() {
        return None;
    }
    Some(total)
}

/// Bytes still allowed under `max_output`, given the output written since
/// `output_start`. Fails with `OutputTooSmall` once the limit is passed.
pub(crate) fn remaining_output_limit(
    output_len: usize,
    output_start: usize,
    max_output: usize,
) -> Result<usize, DecompressError> {
    let written = output_len
        .checked_sub(output_start)
        .expect("output length should not shrink during decompression");
    max_output
        .checked_sub(written)
        .ok_or(DecompressError::OutputTooSmall)
}

/// Decompresses every frame in `input`, skipping skippable frames.
///
/// The output size is unbounded
/// ([`DEFAULT_DECOMPRESS_LIMIT`](zrip_core::DEFAULT_DECOMPRESS_LIMIT)). Use
/// [`decompress_with_limit`] for untrusted input.
pub fn decompress(input: &[u8]) -> Result<Vec<u8>, DecompressError> {
    decompress_with_dict_and_limit(input, None, zrip_core::DEFAULT_DECOMPRESS_LIMIT)
}

/// Decompress with an explicit output size limit.
///
/// Returns [`DecompressError::OutputTooSmall`] if the decompressed output would
/// exceed `max_output_size` bytes. Use [`SAFE_DECOMPRESS_LIMIT`](zrip_core::SAFE_DECOMPRESS_LIMIT)
/// when processing untrusted input to prevent memory exhaustion attacks.
pub fn decompress_with_limit(
    input: &[u8],
    max_output_size: usize,
) -> Result<Vec<u8>, DecompressError> {
    decompress_with_dict_and_limit(input, None, max_output_size)
}

/// Decompresses every frame in `input` and appends the output to `output`.
/// Returns the number of bytes appended.
pub fn decompress_into(input: &[u8], output: &mut Vec<u8>) -> Result<usize, DecompressError> {
    let max_output = zrip_core::DEFAULT_DECOMPRESS_LIMIT;
    let mut ws = Box::new(BlockDecodeWorkspace::new());
    let start = output.len();
    let mut offset = 0;
    while offset < input.len() {
        let remaining = &input[offset..];
        if let Some(skip_len) = skip_skippable_frame(remaining) {
            offset += skip_len;
            continue;
        }
        let frame_limit = remaining_output_limit(output.len(), start, max_output)?;
        let consumed = decompress_frame(remaining, output, frame_limit, None, &mut ws)?;
        offset += consumed;
    }
    Ok(output.len() - start)
}

/// Like [`decompress`], with an optional dictionary.
///
/// A frame that names a dictionary ID fails with
/// [`DecompressError::DictRequired`] without `dict`, and with
/// [`DecompressError::DictMismatch`] if `dict` has another ID.
pub fn decompress_with_dict(
    input: &[u8],
    dict: Option<&zrip_core::dict::Dictionary>,
) -> Result<Vec<u8>, DecompressError> {
    decompress_with_dict_and_limit(input, dict, zrip_core::DEFAULT_DECOMPRESS_LIMIT)
}

/// Like [`decompress_with_dict`], with an output size limit.
///
/// Fails with [`DecompressError::OutputTooSmall`] once the output would
/// exceed `max_output_size` bytes.
pub fn decompress_with_dict_and_limit(
    input: &[u8],
    dict: Option<&zrip_core::dict::Dictionary>,
    max_output_size: usize,
) -> Result<Vec<u8>, DecompressError> {
    let mut output = Vec::new();
    let mut ws = Box::new(BlockDecodeWorkspace::new());
    let mut offset = 0;

    while offset < input.len() {
        let remaining = &input[offset..];
        if let Some(skip_len) = skip_skippable_frame(remaining) {
            offset += skip_len;
            continue;
        }
        let frame_limit = remaining_output_limit(output.len(), 0, max_output_size)?;
        let consumed = decompress_frame(remaining, &mut output, frame_limit, dict, &mut ws)?;
        offset += consumed;
    }

    Ok(output)
}

/// Decodes the frame at the start of `input`, magic included, and appends
/// at most `max_output` bytes to `output`. Returns the input bytes consumed.
pub(crate) fn decompress_frame(
    input: &[u8],
    output: &mut Vec<u8>,
    max_output: usize,
    dict: Option<&zrip_core::dict::Dictionary>,
    ws: &mut BlockDecodeWorkspace,
) -> Result<usize, DecompressError> {
    let header = parse_frame_header(input)?;
    decompress_frame_with_header(input, output, max_output, dict, ws, header)
}

/// Like `decompress_frame`, for a frame whose magic number was stripped.
#[cfg(feature = "alloc")]
pub(crate) fn decompress_frame_after_magic(
    input: &[u8],
    output: &mut Vec<u8>,
    max_output: usize,
    dict: Option<&zrip_core::dict::Dictionary>,
    ws: &mut BlockDecodeWorkspace,
) -> Result<usize, DecompressError> {
    let header = parse_frame_header_after_magic(input, 0)?;
    decompress_frame_with_header(input, output, max_output, dict, ws, header)
}

fn decompress_frame_with_header(
    input: &[u8],
    output: &mut Vec<u8>,
    max_output: usize,
    dict: Option<&zrip_core::dict::Dictionary>,
    ws: &mut BlockDecodeWorkspace,
    header: FrameHeader,
) -> Result<usize, DecompressError> {
    // No window cap here: one-shot output is the history, so the window
    // doesn't bound memory; `max_output` does. Streaming decoders cap it.
    if let Some(frame_dict_id) = header.dict_id {
        match dict {
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

    // A frame may not write more than it declares, so its declared size is
    // its output limit. Each block then reserves at most the bytes still to
    // come, and the reservation up front covers the whole frame.
    let (max_output, overrun) = match header.frame_content_size {
        Some(fcs) => {
            let fcs = usize::try_from(fcs)
                .ok()
                .filter(|&fcs| fcs <= max_output)
                .ok_or(DecompressError::OutputTooSmall)?;
            // Trust the declared size up to 128 MiB, so a forged header
            // cannot force a huge allocation.
            let hint = fcs.min(MAX_WINDOW_SIZE as usize);
            output.reserve(hint + WILDCOPY_OVERLENGTH);
            (fcs, DecompressError::FrameSizeMismatch)
        }
        None => (max_output, DecompressError::OutputTooSmall),
    };

    let mut offset = header.header_size;
    let output_start = output.len();
    let block_size_max = header
        .window_size
        .min(zrip_core::frame::MAX_BLOCK_SIZE as u64) as usize;

    let dict_history: &[u8] = if let Some(d) = dict { d.content() } else { &[] };

    let mut seq_tables = None;
    let mut rep_offsets = [1u32, 4, 8];
    ws.reset_huffman_state();
    if let Some((ref t, l)) = ws.cached_dict_huf {
        // Frames that only reuse the dictionary's table leave it in place.
        if !ws.huf_is_dict {
            ws.huf_table.clear();
            ws.huf_table.extend_from_slice(t);
            ws.huffman_table_replaced();
            ws.huf_is_dict = true;
        }
        ws.huf_table_log = l;
        ws.huf_valid = true;
    } else if let Some(d) = dict
        && let Some((t, l)) = d.huf_table()
    {
        ws.huf_table.clear();
        ws.huf_table.extend_from_slice(t);
        ws.huf_table_log = l;
        ws.huf_valid = true;
        ws.huffman_table_replaced();
    }

    let mut hasher = if header.content_checksum {
        Some(Xxh64State::new(0))
    } else {
        None
    };

    loop {
        if offset + 3 > input.len() {
            return Err(DecompressError::InputExhausted);
        }
        let block_header = parse_block_header(&input[offset..])?;
        offset += 3;

        let block_size = block_header.block_size as usize;

        // `Block_Maximum_Size`: the window, at most 128 KiB. It bounds every
        // block: the compressed size, and the output of raw and RLE blocks.
        if block_size > block_size_max {
            return Err(DecompressError::BlockTooLarge);
        }

        let block_output_start = output.len();
        match block_header.block_type {
            BlockType::Raw => {
                if offset + block_size > input.len() {
                    return Err(DecompressError::InputExhausted);
                }
                if output.len() - output_start + block_size > max_output {
                    return Err(overrun);
                }
                output.extend_from_slice(&input[offset..offset + block_size]);
                offset += block_size;
            }
            BlockType::Rle => {
                if offset >= input.len() {
                    return Err(DecompressError::InputExhausted);
                }
                if output.len() - output_start + block_size > max_output {
                    return Err(overrun);
                }
                let byte = input[offset];
                output.resize(output.len() + block_size, byte);
                offset += 1;
            }
            BlockType::Compressed => {
                if offset + block_size > input.len() {
                    return Err(DecompressError::InputExhausted);
                }
                if seq_tables.is_none() {
                    // A frame that failed after taking the tables dropped
                    // them; fresh tables do not match the table cache.
                    let mut initial_tables = ws.seq_tables.take().unwrap_or_else(|| {
                        ws.seq_tables_replaced();
                        Box::new(SequenceDecodeTables::new_default())
                    });
                    let initial_rep_offsets =
                        initial_sequence_state(initial_tables.as_mut(), ws, dict);
                    seq_tables = Some(initial_tables);
                    rep_offsets = initial_rep_offsets;
                }
                let block_data = &input[offset..offset + block_size];
                decode_compressed_block(
                    block_data,
                    output,
                    output_start,
                    max_output,
                    block_size_max,
                    seq_tables
                        .as_deref_mut()
                        .expect("sequence tables are initialized before compressed blocks"),
                    &mut rep_offsets,
                    ws,
                    dict_history,
                )
                .map_err(|err| {
                    if err == DecompressError::OutputTooSmall {
                        overrun.clone()
                    } else {
                        err
                    }
                })?;
                offset += block_size;
            }
        }
        // As in C zstd, `Block_Maximum_Size` bounds every block's output.
        if output.len() - block_output_start > block_size_max {
            return Err(DecompressError::BlockTooLarge);
        }
        if let Some(ref mut hasher) = hasher {
            hasher.update(&output[block_output_start..]);
        }

        if block_header.last_block {
            break;
        }
    }

    if let Some(tables) = seq_tables.take() {
        ws.seq_tables = Some(tables);
    }

    if let Some(ref mut hasher) = hasher {
        let hash = hasher.finish();
        // The frame stores the low 32 bits of XXH64 with seed 0.
        let expected_checksum = (hash & 0xFFFF_FFFF) as u32;

        if offset + 4 > input.len() {
            return Err(DecompressError::InputExhausted);
        }
        let stored_checksum = u32::from_le_bytes([
            input[offset],
            input[offset + 1],
            input[offset + 2],
            input[offset + 3],
        ]);
        offset += 4;

        if expected_checksum != stored_checksum {
            return Err(DecompressError::ChecksumMismatch {
                expected: stored_checksum,
                got: expected_checksum,
            });
        }
    }

    if let Some(fcs) = header.frame_content_size
        && (output.len() - output_start) as u64 != fcs
    {
        return Err(DecompressError::FrameSizeMismatch);
    }

    Ok(offset)
}

/// Sets up `tables` for a frame's first compressed block. Returns the
/// frame's initial repeat offsets: the dictionary's, or RFC 8878's.
fn initial_sequence_state(
    tables: &mut SequenceDecodeTables,
    ws: &mut BlockDecodeWorkspace,
    dict: Option<&zrip_core::dict::Dictionary>,
) -> [u32; 3] {
    if let Some(ref cached) = ws.cached_dict_tables {
        // Frames that only repeat the dictionary's tables leave them in
        // place. A frame without a dictionary may have cleared their flags.
        if ws.seq_is_dict {
            tables.ll_set = cached.ll_set;
            tables.of_set = cached.of_set;
            tables.ml_set = cached.ml_set;
        } else {
            tables.clone_from(cached);
            ws.seq_tables_replaced();
            ws.seq_is_dict = true;
        }
        ws.cached_dict_rep
    } else if let Some(d) = dict {
        tables.reset_default();
        ws.seq_tables_replaced();
        tables.load_dict(d);
        *d.rep_offsets()
    } else {
        tables.clear_repeat_flags();
        [1u32, 4, 8]
    }
}

/// Match bytes a block's first room allows per byte of its sequence section.
/// A sequence takes two to three bytes there, so only blocks whose matches
/// average well over 100 bytes write more.
const ROOM_PER_SEQUENCE_BYTE: usize = 64;

/// Decodes one compressed block body and appends its output. Fails with
/// `OutputTooSmall` past `max_output` bytes since `output_start`, and with
/// `BlockTooLarge` past `block_size_max`, the frame's `Block_Maximum_Size`,
/// which also bounds the literals.
#[allow(clippy::too_many_arguments)]
fn decode_compressed_block(
    data: &[u8],
    output: &mut Vec<u8>,
    output_start: usize,
    max_output: usize,
    block_size_max: usize,
    seq_tables: &mut SequenceDecodeTables,
    rep_offsets: &mut [u32; 3],
    ws: &mut BlockDecodeWorkspace,
    dict_history: &[u8],
) -> Result<(), DecompressError> {
    let (lit_consumed, lits_at) = decode_literals_ws(data, ws, block_size_max)?;

    let remaining = &data[lit_consumed..];

    // As in C zstd, the sequence count is required, and a block without
    // sequences ends right after it.
    let (num_sequences, seq_count_size) = parse_sequence_count(remaining)?;
    if num_sequences == 0 {
        if remaining.len() != seq_count_size {
            return Err(DecompressError::CorruptSequences);
        }
        let lits = lits_at.resolve(data, &ws.literal_buf).as_slice();
        if output.len() - output_start + lits.len() > max_output {
            return Err(DecompressError::OutputTooSmall);
        }
        output.extend_from_slice(lits);
        return Ok(());
    }

    let table_data = &remaining[seq_count_size..];
    let tables_consumed = parse_sequence_tables_ws(table_data, seq_tables, ws)?;

    let seq_data = &table_data[tables_consumed..];

    let before = output.len();
    let room = remaining_output_limit(output.len(), output_start, max_output)?;
    let max_block = room.min(block_size_max);
    let literals = lits_at.resolve(data, &ws.literal_buf);

    // The sequence loop zeroes the block's room up front. The room starts
    // at a guess from the block's size, so a small block zeroes little even
    // when the frame does not tell its size. A block that writes more
    // starts over with all the room it may use.
    let first_rep_offsets = *rep_offsets;
    let mut max_block_output = seq_data
        .len()
        .saturating_mul(ROOM_PER_SEQUENCE_BYTE)
        .saturating_add(literals.len())
        .min(max_block);
    loop {
        let scope = SequenceOutputScope {
            output_base: output_start,
            max_block_output,
            history: dict_history,
        };
        let result = decode_sequences_dispatch(
            seq_data,
            num_sequences,
            seq_tables,
            rep_offsets,
            literals,
            output,
            scope,
        );
        match result {
            Err(DecompressError::BlockTooLarge) if max_block_output < max_block => {
                output.truncate(before);
                // A lone sequence sets its repeat offsets before it runs.
                *rep_offsets = first_rep_offsets;
                max_block_output = max_block;
            }
            // The output limit, not the block's maximum, ran out.
            Err(DecompressError::BlockTooLarge) if room <= block_size_max => {
                return Err(DecompressError::OutputTooSmall);
            }
            result => return result,
        }
    }
}

/// Decodes and executes a block's sequences. Runs once per block.
///
/// Kept out of line: inlined, the SIMD kernel's code depends on the size of
/// its caller. Dropping a cold check from `decompress_frame_with_header` let
/// LLVM inline the AVX2 kernel there, and decoding got 9% slower.
#[inline(never)]
pub(crate) fn decode_sequences_dispatch(
    seq_data: &[u8],
    num_sequences: u32,
    seq_tables: &mut SequenceDecodeTables,
    rep_offsets: &mut [u32; 3],
    literals: Literals<'_>,
    output: &mut Vec<u8>,
    scope: SequenceOutputScope<'_>,
) -> Result<(), DecompressError> {
    // A lone sequence, common in tiny payloads, skips the loop's setup.
    // Without history, both paths drop the dictionary checks.
    if num_sequences == 1 {
        let literals = literals.as_slice();
        return simd_body!(if scope.history.is_empty() {
            decode_execute_single_sequence::<false>(
                seq_data,
                seq_tables,
                rep_offsets,
                literals,
                output,
                scope,
            )
        } else {
            decode_execute_single_sequence::<true>(
                seq_data,
                seq_tables,
                rep_offsets,
                literals,
                output,
                scope,
            )
        });
    }

    #[cfg(all(feature = "std", feature = "simd"))]
    {
        let level = fearless_simd::Level::new();
        return fearless_simd::dispatch!(level, _simd => {
            if scope.history.is_empty() {
                decode_execute_sequences::<false>(
                    seq_data,
                    num_sequences,
                    seq_tables,
                    rep_offsets,
                    literals,
                    output,
                    scope,
                )
            } else {
                decode_execute_sequences::<true>(
                    seq_data,
                    num_sequences,
                    seq_tables,
                    rep_offsets,
                    literals,
                    output,
                    scope,
                )
            }
        });
    }

    // Scalar build: no `std` or no `simd`.
    #[allow(unreachable_code)]
    if scope.history.is_empty() {
        decode_execute_sequences::<false>(
            seq_data,
            num_sequences,
            seq_tables,
            rep_offsets,
            literals,
            output,
            scope,
        )
    } else {
        decode_execute_sequences::<true>(
            seq_data,
            num_sequences,
            seq_tables,
            rep_offsets,
            literals,
            output,
            scope,
        )
    }
}

/// [`decode_sequences_dispatch`] into a streaming history: the block starts
/// at `*len` in `output`, the room past its output stays for the next
/// block, and `*len` is set to where the output ends
/// ([`decode_execute_sequences_room`]). Runs once per block.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_sequences_dispatch_room(
    seq_data: &[u8],
    num_sequences: u32,
    seq_tables: &mut SequenceDecodeTables,
    rep_offsets: &mut [u32; 3],
    literals: Literals<'_>,
    output: &mut Vec<u8>,
    len: &mut usize,
    scope: SequenceOutputScope<'_>,
) -> Result<(), DecompressError> {
    // A lone sequence appends to the output, so it takes the one-shot path
    // only when no room follows the output.
    if num_sequences == 1 && *len == output.len() {
        let result = decode_sequences_dispatch(
            seq_data,
            num_sequences,
            seq_tables,
            rep_offsets,
            literals,
            output,
            scope,
        );
        *len = output.len();
        return result;
    }

    #[cfg(all(feature = "std", feature = "simd"))]
    {
        let level = fearless_simd::Level::new();
        return fearless_simd::dispatch!(level, _simd => {
            if scope.history.is_empty() {
                decode_execute_sequences_room::<false>(
                    seq_data,
                    num_sequences,
                    seq_tables,
                    rep_offsets,
                    literals,
                    output,
                    len,
                    scope,
                )
            } else {
                decode_execute_sequences_room::<true>(
                    seq_data,
                    num_sequences,
                    seq_tables,
                    rep_offsets,
                    literals,
                    output,
                    len,
                    scope,
                )
            }
        });
    }

    // Scalar build: no `std` or no `simd`.
    #[allow(unreachable_code)]
    if scope.history.is_empty() {
        decode_execute_sequences_room::<false>(
            seq_data,
            num_sequences,
            seq_tables,
            rep_offsets,
            literals,
            output,
            len,
            scope,
        )
    } else {
        decode_execute_sequences_room::<true>(
            seq_data,
            num_sequences,
            seq_tables,
            rep_offsets,
            literals,
            output,
            len,
            scope,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn push_block_header(out: &mut Vec<u8>, last: bool, block_type: u32, block_size: usize) {
        let raw = ((block_size as u32) << 3) | (block_type << 1) | u32::from(last);
        out.push(raw as u8);
        out.push((raw >> 8) as u8);
        out.push((raw >> 16) as u8);
    }

    #[test]
    fn decompresses_frame_after_magic() {
        let mut frame = Vec::new();
        frame.push(0x20);
        frame.push(5);
        push_block_header(&mut frame, true, 0, 5);
        frame.extend_from_slice(b"hello");

        let mut output = Vec::new();
        let mut ws = BlockDecodeWorkspace::new();
        let consumed =
            decompress_frame_after_magic(&frame, &mut output, usize::MAX, None, &mut ws).unwrap();
        assert_eq!(consumed, frame.len());
        assert_eq!(output, b"hello");
    }

    #[test]
    fn frame_reset_keeps_explicit_huffman_cache() {
        let mut ws = BlockDecodeWorkspace::new();
        ws.huf_valid = true;
        ws.huf_last_weights_valid = true;
        ws.huf_last_weights.extend_from_slice(&[1, 2, 3]);

        ws.reset_huffman_state();

        assert!(!ws.huf_valid);
        assert!(ws.huf_last_weights_valid);
        assert_eq!(ws.huf_last_weights, [1, 2, 3]);
    }
}

#[cfg(test)]
mod safety_tests {
    use super::*;
    use alloc::vec::Vec;
    use zrip_core::bitstream::writer::BitWriter;
    use zrip_core::frame::{MAX_BLOCK_SIZE, ZSTD_MAGIC};

    fn push_block_header(out: &mut Vec<u8>, last: bool, block_type: u32, block_size: usize) {
        let raw = ((block_size as u32) << 3) | (block_type << 1) | u32::from(last);
        out.push(raw as u8);
        out.push((raw >> 8) as u8);
        out.push((raw >> 16) as u8);
    }

    fn frame_with_oversized_compressed_block_output() -> Vec<u8> {
        let mut frame = Vec::new();
        frame.extend_from_slice(&ZSTD_MAGIC.to_le_bytes());
        frame.push(0x00);
        frame.push(0x00);

        push_block_header(&mut frame, false, 0, 1);
        frame.push(b'A');

        let mut block = Vec::new();
        let trailing_literals = 65usize;
        block.push(0x04 | (((trailing_literals & 0x0f) as u8) << 4));
        block.push((trailing_literals >> 4) as u8);
        block.extend(core::iter::repeat_n(b'B', trailing_literals));

        block.push(1);
        block.push(0x54);
        block.extend_from_slice(&[0, 2, 52]);

        let mut seq_bits = BitWriter::new();
        let ml_extra = MAX_BLOCK_SIZE as u32 - 65_539;
        seq_bits.write_bits(ml_extra, 16);
        seq_bits.write_bits(0, 2);
        seq_bits.close_reverse_stream();
        block.extend_from_slice(&seq_bits.into_bytes());

        push_block_header(&mut frame, true, 2, block.len());
        frame.extend_from_slice(&block);
        frame
    }

    #[test]
    fn compressed_block_rejects_trailing_literals_over_block_limit() {
        let frame = frame_with_oversized_compressed_block_output();
        assert!(decompress(&frame).is_err());
    }

    /// A frame of a raw block and a compressed block whose `count` sequences
    /// each copy three bytes from offset 3, with a sequence bitstream that
    /// holds the offset bits of only the first `with_bits` of them.
    fn frame_with_short_sequence_stream(count: u8, with_bits: u8) -> Vec<u8> {
        let mut frame = Vec::new();
        frame.extend_from_slice(&ZSTD_MAGIC.to_le_bytes());
        frame.push(0x00);
        frame.push(0x00);

        push_block_header(&mut frame, false, 0, 8);
        frame.extend_from_slice(b"ABCDEFGH");

        let mut block = Vec::new();
        // Raw literals "xy", then RLE tables: literal length code 0, offset
        // code 2 (two extra bits) and match length code 0.
        block.push(2 << 3);
        block.extend_from_slice(b"xy");
        block.push(count);
        block.push(0x54);
        block.extend_from_slice(&[0, 2, 0]);
        let mut seq_bits = BitWriter::new();
        // Offset value 4 + 2: offset 3.
        for _ in 0..with_bits {
            seq_bits.write_bits(2, 2);
        }
        seq_bits.close_reverse_stream();
        block.extend_from_slice(&seq_bits.into_bytes());

        push_block_header(&mut frame, true, 2, block.len());
        frame.extend_from_slice(&block);
        frame
    }

    #[test]
    fn sequence_streams_that_read_past_their_start_are_rejected() {
        assert_eq!(
            decompress(&frame_with_short_sequence_stream(1, 1)).unwrap(),
            b"ABCDEFGHFGHxy"
        );
        assert_eq!(
            decompress(&frame_with_short_sequence_stream(2, 1)),
            Err(DecompressError::CorruptSequences)
        );
        // A block with one sequence takes its own path.
        assert_eq!(
            decompress(&frame_with_short_sequence_stream(1, 0)),
            Err(DecompressError::CorruptSequences)
        );
    }
}
