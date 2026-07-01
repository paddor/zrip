#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(optimize_attribute))]
#![cfg_attr(feature = "paranoid", forbid(unsafe_code))]

#[cfg(feature = "alloc")]
extern crate alloc;

pub(crate) mod block_decoder;
#[cfg(feature = "std")]
pub mod context;
pub(crate) mod exec;
pub(crate) mod fast_vec;
pub(crate) mod literals;
pub(crate) mod ring_buffer;
pub(crate) mod seq_table;
pub(crate) mod sequences;
#[cfg(feature = "std")]
pub mod streaming;

#[cfg(feature = "alloc")]
use alloc::boxed::Box;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::exec::decode_execute_sequences;
use crate::literals::decode_literals_ws;
use crate::sequences::{SequenceDecodeTables, parse_sequence_count, parse_sequence_tables_ws};
use zrip_core::block::{BlockType, parse_block_header};
use zrip_core::error::DecompressError;
use zrip_core::frame::MAX_WINDOW_SIZE;
use zrip_core::frame::header::parse_frame_header;
use zrip_core::huffman::HuffmanDecodeEntry;
use zrip_core::xxhash::Xxh64State;

pub(crate) struct BlockDecodeWorkspace {
    pub literal_buf: Vec<u8>,
    pub huf_table: Vec<HuffmanDecodeEntry>,
    pub huf_table_log: u8,
    pub huf_valid: bool,
    pub huf_all_weights: Vec<u8>,
    pub huf_rank_count: Vec<u32>,
    pub huf_rank_start: Vec<u32>,
    pub huf_weights: Vec<u8>,
    pub huf_last_weights: Vec<u8>,
    pub huf_last_weights_valid: bool,
    pub fse_dist: Vec<i16>,
    pub fse_symbol_next: Vec<u16>,
    pub fse_build_buf: Vec<zrip_core::fse::FseDecodeEntry>,
    pub cached_dict_tables: Option<SequenceDecodeTables>,
    pub cached_dict_rep: [u32; 3],
    pub cached_dict_huf: Option<(Vec<HuffmanDecodeEntry>, u8)>,
}

impl BlockDecodeWorkspace {
    pub(crate) fn new() -> Self {
        Self {
            literal_buf: Vec::new(),
            huf_table: Vec::new(),
            huf_table_log: 0,
            huf_valid: false,
            huf_all_weights: Vec::new(),
            huf_rank_count: Vec::new(),
            huf_rank_start: Vec::new(),
            huf_weights: Vec::new(),
            huf_last_weights: Vec::new(),
            huf_last_weights_valid: false,
            fse_dist: Vec::new(),
            fse_symbol_next: Vec::new(),
            fse_build_buf: Vec::new(),
            cached_dict_tables: None,
            cached_dict_rep: [1, 4, 8],
            cached_dict_huf: None,
        }
    }

    pub(crate) fn reset_huffman_state(&mut self) {
        self.huf_valid = false;
        self.huf_last_weights_valid = false;
    }

    #[cfg(feature = "std")]
    pub(crate) fn cache_dict(&mut self, dict: &zrip_core::dict::Dictionary) {
        let mut st = SequenceDecodeTables::new_default();
        if let Some((t, l)) = dict.of_table() {
            st.of_table = crate::seq_table::SeqTable::promote_of(t);
            st.of_accuracy = l;
            st.of_set = true;
        }
        if let Some((t, l)) = dict.ml_table() {
            st.ml_table = crate::seq_table::SeqTable::promote_ml(t);
            st.ml_accuracy = l;
            st.ml_set = true;
        }
        if let Some((t, l)) = dict.ll_table() {
            st.ll_table = crate::seq_table::SeqTable::promote_ll(t);
            st.ll_accuracy = l;
            st.ll_set = true;
        }
        self.cached_dict_tables = Some(st);
        self.cached_dict_rep = *dict.rep_offsets();
        if let Some((t, l)) = dict.huf_table() {
            self.cached_dict_huf = Some((t.to_vec(), l));
        }
    }
}

pub(crate) fn skip_skippable_frame(data: &[u8]) -> Option<usize> {
    if data.len() < 8 {
        return None;
    }
    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if (magic & 0xFFFF_FFF0) != 0x184D_2A50 {
        return None;
    }
    let frame_size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let total = 8 + frame_size;
    if total > data.len() {
        return None;
    }
    Some(total)
}

pub fn decompress(input: &[u8]) -> Result<Vec<u8>, DecompressError> {
    decompress_with_dict(input, None)
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
    let mut output = Vec::new();
    let mut ws = Box::new(BlockDecodeWorkspace::new());
    let mut offset = 0;
    while offset < input.len() {
        let remaining = &input[offset..];
        if let Some(skip_len) = skip_skippable_frame(remaining) {
            offset += skip_len;
            continue;
        }
        let consumed = decompress_frame(remaining, &mut output, max_output_size, None, &mut ws)?;
        offset += consumed;
    }
    Ok(output)
}

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
        let consumed = decompress_frame(remaining, output, max_output, None, &mut ws)?;
        offset += consumed;
    }
    Ok(output.len() - start)
}

pub fn decompress_with_dict(
    input: &[u8],
    dict: Option<&zrip_core::dict::Dictionary>,
) -> Result<Vec<u8>, DecompressError> {
    let max_output = zrip_core::DEFAULT_DECOMPRESS_LIMIT;
    let mut output = Vec::new();
    let mut ws = Box::new(BlockDecodeWorkspace::new());
    let mut offset = 0;

    while offset < input.len() {
        let remaining = &input[offset..];
        if let Some(skip_len) = skip_skippable_frame(remaining) {
            offset += skip_len;
            continue;
        }
        let consumed = decompress_frame(remaining, &mut output, max_output, dict, &mut ws)?;
        offset += consumed;
    }

    Ok(output)
}

pub(crate) fn decompress_frame(
    input: &[u8],
    output: &mut Vec<u8>,
    max_output: usize,
    dict: Option<&zrip_core::dict::Dictionary>,
    ws: &mut BlockDecodeWorkspace,
) -> Result<usize, DecompressError> {
    let header = parse_frame_header(input)?;

    if header.window_size > MAX_WINDOW_SIZE && !header.single_segment {
        return Err(DecompressError::WindowTooLarge {
            requested: header.window_size,
            max: MAX_WINDOW_SIZE,
        });
    }

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

    if let Some(fcs) = header.frame_content_size {
        if max_output < usize::MAX && fcs as usize > max_output {
            return Err(DecompressError::OutputTooSmall);
        }
        let hint = (fcs as usize).min(MAX_WINDOW_SIZE as usize);
        output.reserve(hint + 32);
    }

    let mut offset = header.header_size;
    let output_start = output.len();

    let dict_history: &[u8] = if let Some(d) = dict { d.content() } else { &[] };

    let (mut seq_tables, mut rep_offsets) = if let Some(ref cached) = ws.cached_dict_tables {
        (cached.clone(), ws.cached_dict_rep)
    } else if let Some(d) = dict {
        let mut st = SequenceDecodeTables::new_default();
        if let Some((t, l)) = d.of_table() {
            st.of_table = crate::seq_table::SeqTable::promote_of(t);
            st.of_accuracy = l;
            st.of_set = true;
        }
        if let Some((t, l)) = d.ml_table() {
            st.ml_table = crate::seq_table::SeqTable::promote_ml(t);
            st.ml_accuracy = l;
            st.ml_set = true;
        }
        if let Some((t, l)) = d.ll_table() {
            st.ll_table = crate::seq_table::SeqTable::promote_ll(t);
            st.ll_accuracy = l;
            st.ll_set = true;
        }
        (st, *d.rep_offsets())
    } else {
        (SequenceDecodeTables::new_default(), [1u32, 4, 8])
    };
    ws.reset_huffman_state();
    if let Some((ref t, l)) = ws.cached_dict_huf {
        ws.huf_table.clear();
        ws.huf_table.extend_from_slice(t);
        ws.huf_table_log = l;
        ws.huf_valid = true;
    } else if let Some(d) = dict
        && let Some((t, l)) = d.huf_table()
    {
        ws.huf_table.clear();
        ws.huf_table.extend_from_slice(t);
        ws.huf_table_log = l;
        ws.huf_valid = true;
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

        if block_size > zrip_core::frame::MAX_BLOCK_SIZE {
            match block_header.block_type {
                BlockType::Raw | BlockType::Rle => {
                    return Err(DecompressError::BlockTooLarge);
                }
                BlockType::Compressed => {}
            }
        }

        let block_output_start = output.len();
        match block_header.block_type {
            BlockType::Raw => {
                if offset + block_size > input.len() {
                    return Err(DecompressError::InputExhausted);
                }
                if output.len() - output_start + block_size > max_output {
                    return Err(DecompressError::OutputTooSmall);
                }
                output.extend_from_slice(&input[offset..offset + block_size]);
                offset += block_size;
            }
            BlockType::Rle => {
                if offset >= input.len() {
                    return Err(DecompressError::InputExhausted);
                }
                if output.len() - output_start + block_size > max_output {
                    return Err(DecompressError::OutputTooSmall);
                }
                let byte = input[offset];
                output.resize(output.len() + block_size, byte);
                offset += 1;
            }
            BlockType::Compressed => {
                if offset + block_size > input.len() {
                    return Err(DecompressError::InputExhausted);
                }
                let block_data = &input[offset..offset + block_size];
                decode_compressed_block(
                    block_data,
                    output,
                    output_start,
                    max_output,
                    &mut seq_tables,
                    &mut rep_offsets,
                    ws,
                    dict_history,
                )?;
                offset += block_size;
            }
        }
        if let Some(ref mut hasher) = hasher {
            hasher.update(&output[block_output_start..]);
        }

        if block_header.last_block {
            break;
        }
    }

    if let Some(ref mut hasher) = hasher {
        let hash = hasher.finish();
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

#[allow(clippy::too_many_arguments)]
fn decode_compressed_block(
    data: &[u8],
    output: &mut Vec<u8>,
    output_start: usize,
    max_output: usize,
    seq_tables: &mut SequenceDecodeTables,
    rep_offsets: &mut [u32; 3],
    ws: &mut BlockDecodeWorkspace,
    dict_history: &[u8],
) -> Result<(), DecompressError> {
    let lit_consumed = decode_literals_ws(data, ws)?;

    let remaining = &data[lit_consumed..];

    if remaining.is_empty() {
        if output.len() - output_start + ws.literal_buf.len() > max_output {
            return Err(DecompressError::OutputTooSmall);
        }
        output.extend_from_slice(&ws.literal_buf);
        return Ok(());
    }

    let (num_sequences, seq_count_size) = parse_sequence_count(remaining)?;

    if num_sequences == 0 {
        if output.len() - output_start + ws.literal_buf.len() > max_output {
            return Err(DecompressError::OutputTooSmall);
        }
        output.extend_from_slice(&ws.literal_buf);
        return Ok(());
    }

    let table_data = &remaining[seq_count_size..];
    let tables_consumed = parse_sequence_tables_ws(table_data, seq_tables, ws)?;

    let seq_data = &table_data[tables_consumed..];

    let before = output.len();

    let result = decode_sequences_dispatch(
        seq_data,
        num_sequences,
        seq_tables,
        rep_offsets,
        &ws.literal_buf,
        output,
        dict_history,
    );
    result?;
    if output.len() - before > zrip_core::frame::MAX_BLOCK_SIZE {
        return Err(DecompressError::BlockTooLarge);
    }

    Ok(())
}

#[inline(always)]
pub(crate) fn decode_sequences_dispatch(
    seq_data: &[u8],
    num_sequences: u32,
    seq_tables: &mut SequenceDecodeTables,
    rep_offsets: &mut [u32; 3],
    literals: &[u8],
    output: &mut Vec<u8>,
    history: &[u8],
) -> Result<(), DecompressError> {
    #[cfg(all(feature = "std", feature = "simd"))]
    {
        use std::sync::OnceLock;
        static LEVEL: OnceLock<fearless_simd::Level> = OnceLock::new();
        let level = *LEVEL.get_or_init(fearless_simd::Level::new);
        return fearless_simd::dispatch!(level, _simd => {
            if history.is_empty() {
                decode_execute_sequences::<false>(
                    seq_data,
                    num_sequences,
                    seq_tables,
                    rep_offsets,
                    literals,
                    output,
                    history,
                )
            } else {
                decode_execute_sequences::<true>(
                    seq_data,
                    num_sequences,
                    seq_tables,
                    rep_offsets,
                    literals,
                    output,
                    history,
                )
            }
        });
    }

    #[allow(unreachable_code)]
    if history.is_empty() {
        decode_execute_sequences::<false>(
            seq_data,
            num_sequences,
            seq_tables,
            rep_offsets,
            literals,
            output,
            history,
        )
    } else {
        decode_execute_sequences::<true>(
            seq_data,
            num_sequences,
            seq_tables,
            rep_offsets,
            literals,
            output,
            history,
        )
    }
}

#[cfg(all(test, miri, not(feature = "paranoid")))]
mod ub_tests {
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
    fn compressed_block_trailing_literals_overrun_wildcopy_headroom() {
        // Issue: sequence execution reserves one block plus 64 bytes of wild-copy
        // headroom and checks each sequence against that limit, but the final
        // trailing literals are appended afterward without a matching capacity or
        // block-size check. This safe frame seeds one prior byte, emits a 128 KiB
        // match at offset 1, then appends 65 literals; miri reports the resulting
        // out-of-bounds write in fast_extend_from_slice.
        let frame = frame_with_oversized_compressed_block_output();
        let _ = decompress(&frame);
    }
}
