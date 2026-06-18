#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(optimize_attribute))]

#[cfg(feature = "alloc")]
extern crate alloc;

pub(crate) mod block_decoder;
#[cfg(feature = "std")]
pub mod context;
pub(crate) mod exec;
pub(crate) mod literals;
pub(crate) mod ring_buffer;
pub(crate) mod sequences;
#[cfg(feature = "std")]
pub mod streaming;

#[allow(dead_code)]
pub(crate) mod simd_decode;

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
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
use zrip_core::simd::CpuTier;
use zrip_core::xxhash::Xxh64State;

pub(crate) struct BlockDecodeWorkspace {
    pub literal_buf: Vec<u8>,
    pub huf_table: Vec<HuffmanDecodeEntry>,
    pub huf_table_log: u8,
    pub huf_valid: bool,
    pub huf_all_weights: Vec<u8>,
    pub huf_rank_count: Vec<u32>,
    pub huf_rank_start: Vec<u32>,
    pub fse_dist: Vec<i16>,
    pub fse_symbol_next: Vec<u16>,
    pub fse_build_buf: Vec<zrip_core::fse::FseDecodeEntry>,
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
            fse_dist: Vec::new(),
            fse_symbol_next: Vec::new(),
            fse_build_buf: Vec::new(),
        }
    }
}

pub(crate) fn skip_skippable_frame(data: &[u8]) -> Option<usize> {
    if data.len() < 8 {
        return None;
    }
    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if (magic & 0xFFFFFFF0) != 0x184D2A50 {
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

    if header.window_size > MAX_WINDOW_SIZE {
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

    let mut seq_tables = if let Some(d) = dict {
        let mut st = SequenceDecodeTables::new_default();
        if let Some((t, l)) = d.of_table() {
            st.of_table = zrip_core::fse::promote_of_table(t);
            st.of_accuracy = l;
        }
        if let Some((t, l)) = d.ml_table() {
            st.ml_table = zrip_core::fse::promote_ml_table(t);
            st.ml_accuracy = l;
        }
        if let Some((t, l)) = d.ll_table() {
            st.ll_table = zrip_core::fse::promote_ll_table(t);
            st.ll_accuracy = l;
        }
        st
    } else {
        SequenceDecodeTables::new_default()
    };
    let mut rep_offsets: [u32; 3] = if let Some(d) = dict {
        *d.rep_offsets()
    } else {
        [1, 4, 8]
    };
    ws.huf_valid = false;
    if let Some(d) = dict {
        if let Some((t, l)) = d.huf_table() {
            ws.huf_table.clear();
            ws.huf_table.extend_from_slice(t);
            ws.huf_table_log = l;
            ws.huf_valid = true;
        }
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
                    return Err(DecompressError::CorruptSequences);
                }
                BlockType::Compressed => {}
            }
        }

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

        if block_header.last_block {
            break;
        }
    }

    if let Some(ref mut hasher) = hasher {
        hasher.update(&output[output_start..]);
        let hash = hasher.finish();
        let expected_checksum = (hash & 0xFFFFFFFF) as u32;

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

    if let Some(fcs) = header.frame_content_size {
        if (output.len() - output_start) as u64 != fcs {
            return Err(DecompressError::CorruptSequences);
        }
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

    #[cfg(target_arch = "x86_64")]
    {
        if zrip_core::simd::cpu_tier() >= CpuTier::Avx2 {
            decode_execute_block_avx2(
                seq_data,
                num_sequences,
                seq_tables,
                rep_offsets,
                &ws.literal_buf,
                output,
                dict_history,
            )?;
            if output.len() - before > zrip_core::frame::MAX_BLOCK_SIZE {
                return Err(DecompressError::CorruptSequences);
            }
            return Ok(());
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if zrip_core::simd::cpu_tier() >= CpuTier::Neon {
            decode_execute_block_neon(
                seq_data,
                num_sequences,
                seq_tables,
                rep_offsets,
                &ws.literal_buf,
                output,
                dict_history,
            )?;
            if output.len() - before > zrip_core::frame::MAX_BLOCK_SIZE {
                return Err(DecompressError::CorruptSequences);
            }
            return Ok(());
        }
    }

    decode_execute_sequences(
        seq_data,
        num_sequences,
        seq_tables,
        rep_offsets,
        &ws.literal_buf,
        output,
        dict_history,
    )?;
    if output.len() - before > zrip_core::frame::MAX_BLOCK_SIZE {
        return Err(DecompressError::CorruptSequences);
    }

    Ok(())
}

#[cfg(target_arch = "x86_64")]
fn decode_execute_block_avx2(
    seq_data: &[u8],
    num_sequences: u32,
    tables: &mut SequenceDecodeTables,
    rep_offsets: &mut [u32; 3],
    literals: &[u8],
    output: &mut Vec<u8>,
    history: &[u8],
) -> Result<(), DecompressError> {
    crate::simd_decode::x86_64::decode::decode_execute_avx2_safe(
        seq_data,
        num_sequences,
        tables,
        rep_offsets,
        literals,
        output,
        history,
    )
}

#[cfg(target_arch = "aarch64")]
fn decode_execute_block_neon(
    seq_data: &[u8],
    num_sequences: u32,
    tables: &mut SequenceDecodeTables,
    rep_offsets: &mut [u32; 3],
    literals: &[u8],
    output: &mut Vec<u8>,
    history: &[u8],
) -> Result<(), DecompressError> {
    crate::simd_decode::aarch64::decode::decode_execute_neon_safe(
        seq_data,
        num_sequences,
        tables,
        rep_offsets,
        literals,
        output,
        history,
    )
}
