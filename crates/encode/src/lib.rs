#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(optimize_attribute))]

#[cfg(feature = "alloc")]
extern crate alloc;

pub(crate) mod block_encoder;
#[cfg(feature = "std")]
pub mod context;
pub(crate) mod dfast;
pub(crate) mod fast;
pub(crate) mod primitives;
pub(crate) mod sequences;
pub mod strategy;
#[cfg(feature = "std")]
pub mod streaming;

#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::strategy::Strategy;
use zrip_core::error::CompressError;
use zrip_core::frame::{MAX_BLOCK_SIZE, ZSTD_MAGIC};
use zrip_core::xxhash::xxh64;

pub(crate) fn block_looks_incompressible(data: &[u8]) -> bool {
    const SAMPLE: usize = 1024;
    const DISTINCT_THRESHOLD: u32 = 200;
    const MAX_FREQ_DENOM: u32 = 24;
    if data.len() < SAMPLE {
        return false;
    }
    let mut counts = [0u16; 256];
    for &b in &data[..SAMPLE] {
        counts[b as usize] += 1;
    }
    let mut distinct: u32 = 0;
    let mut max_freq: u16 = 0;
    for &c in &counts {
        distinct += (c > 0) as u32;
        max_freq = max_freq.max(c);
    }
    distinct >= DISTINCT_THRESHOLD && (max_freq as u32) <= SAMPLE as u32 / MAX_FREQ_DENOM
}

pub(crate) fn clamp_params_to_src_size(params: &mut strategy::LevelParams, src_len: usize) {
    if src_len >= 2 {
        let src_log = 32 - ((src_len as u32) - 1).leading_zeros();
        params.hash_log = params.hash_log.min(src_log);
        params.chain_log = params.chain_log.min(src_log);
        params.window_log = params.window_log.min(src_log);
    }
}

pub fn compress_with_params(
    input: &[u8],
    params: &strategy::LevelParams,
) -> Result<Vec<u8>, CompressError> {
    let mut params = *params;
    clamp_params_to_src_size(&mut params, input.len());
    compress_inner(input, &params)
}

pub fn compress(input: &[u8], level: i32) -> Result<Vec<u8>, CompressError> {
    let mut params = strategy::level_params(level).ok_or(CompressError::InvalidLevel(level))?;
    clamp_params_to_src_size(&mut params, input.len());
    compress_inner(input, &params)
}

fn compress_inner(input: &[u8], params: &strategy::LevelParams) -> Result<Vec<u8>, CompressError> {
    let mut output = Vec::with_capacity(input.len() + 32);
    compress_frame(input, params, &mut output);
    Ok(output)
}

fn compress_frame(input: &[u8], params: &strategy::LevelParams, output: &mut Vec<u8>) {
    output.extend_from_slice(&ZSTD_MAGIC.to_le_bytes());

    let fcs_size = if input.len() <= 255 {
        1
    } else if input.len() <= 0xFFFF + 256 {
        2
    } else if input.len() <= 0xFFFFFFFF {
        4
    } else {
        8
    };

    let fcs_flag = match fcs_size {
        1 => 0,
        2 => 1,
        4 => 2,
        8 => 3,
        _ => unreachable!(),
    };

    let descriptor = 0x20 | 0x04 | (fcs_flag << 6);
    output.push(descriptor);

    match fcs_size {
        1 => output.push(input.len() as u8),
        2 => {
            let v = (input.len() - 256) as u16;
            output.extend_from_slice(&v.to_le_bytes());
        }
        4 => output.extend_from_slice(&(input.len() as u32).to_le_bytes()),
        8 => output.extend_from_slice(&(input.len() as u64).to_le_bytes()),
        _ => unreachable!(),
    }

    if input.is_empty() {
        block_encoder::encode_raw_block(&[], true, output);
    } else {
        let hash_size = 1usize << params.hash_log;
        let mut rep_offsets = [1u32, 4, 8];
        let mut offset = 0;
        let mut sequences = Vec::with_capacity(MAX_BLOCK_SIZE / 8);
        let mut workspace = block_encoder::BlockEncodeWorkspace::new();

        match params.strategy {
            Strategy::Fast => {
                let mut hash_table = vec![0u32; hash_size];
                while offset < input.len() {
                    let chunk_size = (input.len() - offset).min(MAX_BLOCK_SIZE);
                    let block_end = offset + chunk_size;
                    let is_last = block_end >= input.len();

                    if block_looks_incompressible(&input[offset..block_end]) {
                        block_encoder::encode_raw_block(&input[offset..block_end], is_last, output);
                    } else {
                        fast::compress_fast_block(
                            input,
                            offset,
                            block_end,
                            params,
                            &rep_offsets,
                            &mut hash_table,
                            &mut sequences,
                        );
                        if params.force_raw_literals {
                            block_encoder::encode_compressed_block_raw(
                                &input[offset..block_end],
                                &sequences,
                                &mut rep_offsets,
                                is_last,
                                output,
                                &mut workspace,
                            );
                        } else {
                            block_encoder::encode_compressed_block(
                                &input[offset..block_end],
                                &sequences,
                                &mut rep_offsets,
                                is_last,
                                output,
                                &mut workspace,
                            );
                        }
                    }
                    offset = block_end;
                }
            }
            Strategy::DFast => {
                let short_size = 1usize << params.chain_log;
                let long_size = 1usize << params.hash_log;
                let mut hash_short = vec![0u32; short_size];
                let mut hash_long = vec![0u32; long_size];
                while offset < input.len() {
                    let chunk_size = (input.len() - offset).min(MAX_BLOCK_SIZE);
                    let block_end = offset + chunk_size;
                    let is_last = block_end >= input.len();

                    if block_looks_incompressible(&input[offset..block_end]) {
                        block_encoder::encode_raw_block(&input[offset..block_end], is_last, output);
                    } else {
                        dfast::compress_dfast_block(
                            input,
                            offset,
                            block_end,
                            params,
                            &rep_offsets,
                            &mut hash_short,
                            &mut hash_long,
                            &mut sequences,
                        );
                        block_encoder::encode_compressed_block(
                            &input[offset..block_end],
                            &sequences,
                            &mut rep_offsets,
                            is_last,
                            output,
                            &mut workspace,
                        );
                    }
                    offset = block_end;
                }
            }
        }
    }

    let hash = xxh64(input, 0);
    let checksum = (hash & 0xFFFFFFFF) as u32;
    output.extend_from_slice(&checksum.to_le_bytes());
}

pub fn compress_with_dict(
    input: &[u8],
    level: i32,
    dict: &zrip_core::dict::Dictionary,
) -> Result<Vec<u8>, CompressError> {
    let mut params = strategy::level_params(level).ok_or(CompressError::InvalidLevel(level))?;
    clamp_params_to_src_size(&mut params, input.len());

    let mut output = Vec::with_capacity(input.len() + 32);

    output.extend_from_slice(&ZSTD_MAGIC.to_le_bytes());

    let fcs_size = if input.len() <= 255 {
        1
    } else if input.len() <= 0xFFFF + 256 {
        2
    } else if input.len() <= 0xFFFFFFFF {
        4
    } else {
        8
    };

    let fcs_flag = match fcs_size {
        1 => 0,
        2 => 1,
        4 => 2,
        8 => 3,
        _ => unreachable!(),
    };

    let dict_id = dict.id();
    let dict_id_flag = if dict_id <= 0xFF {
        1u8
    } else if dict_id <= 0xFFFF {
        2
    } else {
        3
    };

    let descriptor = 0x20 | 0x04 | (fcs_flag << 6) | dict_id_flag;
    output.push(descriptor);

    match dict_id_flag {
        1 => output.push(dict_id as u8),
        2 => output.extend_from_slice(&(dict_id as u16).to_le_bytes()),
        3 => output.extend_from_slice(&dict_id.to_le_bytes()),
        _ => unreachable!(),
    }

    match fcs_size {
        1 => output.push(input.len() as u8),
        2 => {
            let v = (input.len() - 256) as u16;
            output.extend_from_slice(&v.to_le_bytes());
        }
        4 => output.extend_from_slice(&(input.len() as u32).to_le_bytes()),
        8 => output.extend_from_slice(&(input.len() as u64).to_le_bytes()),
        _ => unreachable!(),
    }

    if input.is_empty() {
        block_encoder::encode_raw_block(&[], true, &mut output);
    } else {
        let prefix = dict.content();
        let mut rep_offsets = *dict.rep_offsets();
        let mut workspace = block_encoder::BlockEncodeWorkspace::new();

        if input.len() <= MAX_BLOCK_SIZE {
            let sequences = match params.strategy {
                Strategy::Fast => {
                    fast::compress_fast_with_prefix(input, &params, &rep_offsets, prefix)
                }
                Strategy::DFast => {
                    dfast::compress_dfast_with_prefix(input, &params, &rep_offsets, prefix)
                }
            };
            if params.force_raw_literals {
                block_encoder::encode_compressed_block_raw(
                    input,
                    &sequences,
                    &mut rep_offsets,
                    true,
                    &mut output,
                    &mut workspace,
                );
            } else {
                block_encoder::encode_compressed_block(
                    input,
                    &sequences,
                    &mut rep_offsets,
                    true,
                    &mut output,
                    &mut workspace,
                );
            }
        } else {
            let mut combined = Vec::with_capacity(prefix.len() + input.len());
            combined.extend_from_slice(prefix);
            combined.extend_from_slice(input);
            let plen = prefix.len();
            let hash_size = 1usize << params.hash_log;
            let mut sequences = Vec::new();

            match params.strategy {
                Strategy::Fast => {
                    let mut hash_table = vec![0u32; hash_size];
                    fast::prefill_hash_table(&combined, plen, params.hash_log, &mut hash_table);
                    let mut offset = 0;
                    while offset < input.len() {
                        let chunk_size = (input.len() - offset).min(MAX_BLOCK_SIZE);
                        let is_last = offset + chunk_size >= input.len();
                        fast::compress_fast_block(
                            &combined,
                            plen + offset,
                            plen + offset + chunk_size,
                            &params,
                            &rep_offsets,
                            &mut hash_table,
                            &mut sequences,
                        );
                        if params.force_raw_literals {
                            block_encoder::encode_compressed_block_raw(
                                &input[offset..offset + chunk_size],
                                &sequences,
                                &mut rep_offsets,
                                is_last,
                                &mut output,
                                &mut workspace,
                            );
                        } else {
                            block_encoder::encode_compressed_block(
                                &input[offset..offset + chunk_size],
                                &sequences,
                                &mut rep_offsets,
                                is_last,
                                &mut output,
                                &mut workspace,
                            );
                        }
                        offset += chunk_size;
                    }
                }
                Strategy::DFast => {
                    let short_size = 1usize << params.chain_log;
                    let long_size = 1usize << params.hash_log;
                    let mut hash_short = vec![0u32; short_size];
                    let mut hash_long = vec![0u32; long_size];
                    dfast::prefill_hash_tables(
                        &combined,
                        plen,
                        params.hash_log,
                        params.chain_log,
                        &mut hash_short,
                        &mut hash_long,
                    );
                    let mut offset = 0;
                    while offset < input.len() {
                        let chunk_size = (input.len() - offset).min(MAX_BLOCK_SIZE);
                        let is_last = offset + chunk_size >= input.len();
                        dfast::compress_dfast_block(
                            &combined,
                            plen + offset,
                            plen + offset + chunk_size,
                            &params,
                            &rep_offsets,
                            &mut hash_short,
                            &mut hash_long,
                            &mut sequences,
                        );
                        block_encoder::encode_compressed_block(
                            &input[offset..offset + chunk_size],
                            &sequences,
                            &mut rep_offsets,
                            is_last,
                            &mut output,
                            &mut workspace,
                        );
                        offset += chunk_size;
                    }
                }
            }
        }
    }

    let hash = xxh64(input, 0);
    let checksum = (hash & 0xFFFFFFFF) as u32;
    output.extend_from_slice(&checksum.to_le_bytes());

    Ok(output)
}

pub fn compress_into(input: &[u8], output: &mut [u8], level: i32) -> Result<usize, CompressError> {
    let mut params = strategy::level_params(level).ok_or(CompressError::InvalidLevel(level))?;
    clamp_params_to_src_size(&mut params, input.len());
    let mut buf = Vec::with_capacity(output.len());
    compress_frame(input, &params, &mut buf);
    if buf.len() > output.len() {
        return Err(CompressError::OutputTooSmall);
    }
    output[..buf.len()].copy_from_slice(&buf);
    Ok(buf.len())
}
