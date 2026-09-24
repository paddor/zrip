#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(feature = "nightly", feature(optimize_attribute))]
#![cfg_attr(feature = "paranoid", forbid(unsafe_code))]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(not(feature = "paranoid"))]
macro_rules! paranoid_unsafe_call {
    ($e:expr) => {
        unsafe { $e }
    };
}

#[cfg(feature = "paranoid")]
macro_rules! paranoid_unsafe_call {
    ($e:expr) => {
        $e
    };
}

/// Evaluates `$body` compiled for x86-64-v3 (AVX2, BMI1/2, LZCNT) when the CPU
/// supports it, and compiled for the build target otherwise.
///
/// Use it as a whole function body around a call to an `#[inline(always)]`
/// implementation, so the implementation is duplicated into both contexts.
/// `fearless_simd` selects the level at runtime and provides the safe
/// target-feature entry point.
#[cfg(all(feature = "simd", target_arch = "x86_64", not(target_feature = "avx2")))]
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

#[cfg(not(all(feature = "simd", target_arch = "x86_64", not(target_feature = "avx2"))))]
macro_rules! simd_body {
    ($body:expr) => {
        $body
    };
}

pub(crate) mod block_encoder;
#[cfg(feature = "std")]
pub mod context;
pub(crate) mod dfast;
pub(crate) mod fast;
#[cfg(feature = "ldm")]
pub(crate) mod ldm;
mod output;
pub(crate) mod primitives;
pub mod strategy;
#[cfg(feature = "std")]
pub mod streaming;

#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::output::{OutputSink, SliceSink};
use crate::strategy::Strategy;
use zrip_core::error::CompressError;
use zrip_core::frame::{MAX_BLOCK_SIZE, MAX_WINDOW_SIZE, ZSTD_MAGIC};
use zrip_core::xxhash::xxh64;

pub(crate) fn write_frame_header(
    output: &mut impl OutputSink,
    content_size: usize,
    dict_id: Option<u32>,
    window_log: u32,
) -> Result<(), CompressError> {
    write_frame_header_inner(output, Some(content_size), dict_id, window_log, true)
}

#[cfg_attr(not(feature = "std"), allow(dead_code))]
pub(crate) fn write_frame_header_with_checksum(
    output: &mut impl OutputSink,
    content_size: usize,
    dict_id: Option<u32>,
    window_log: u32,
    content_checksum: bool,
) -> Result<(), CompressError> {
    write_frame_header_inner(
        output,
        Some(content_size),
        dict_id,
        window_log,
        content_checksum,
    )
}

#[cfg_attr(not(feature = "std"), allow(dead_code))]
pub(crate) fn write_frame_header_without_content_size(
    output: &mut impl OutputSink,
    dict_id: Option<u32>,
    window_log: u32,
) -> Result<(), CompressError> {
    write_frame_header_inner(output, None, dict_id, window_log, true)
}

fn write_frame_header_inner(
    output: &mut impl OutputSink,
    content_size: Option<usize>,
    dict_id: Option<u32>,
    window_log: u32,
    content_checksum: bool,
) -> Result<(), CompressError> {
    output.extend_from_slice(&ZSTD_MAGIC.to_le_bytes())?;

    let single_segment =
        dict_id.is_none() && content_size.is_some_and(|size| size as u64 <= MAX_WINDOW_SIZE);
    let fcs_size = content_size.map_or(0, |size| {
        frame_content_size_field_size(size, single_segment)
    });
    let fcs_flag: u8 = match fcs_size {
        0 => 0,
        1 => 0,
        2 => 1,
        4 => 2,
        _ => 3,
    };

    let dict_id_flag: u8 = match dict_id {
        None => 0,
        Some(id) if id <= 0xFF => 1,
        Some(id) if id <= 0xFFFF => 2,
        Some(_) => 3,
    };

    let descriptor = if single_segment { 0x20 } else { 0 }
        | if content_checksum { 0x04 } else { 0 }
        | (fcs_flag << 6)
        | dict_id_flag;
    output.push(descriptor)?;

    if !single_segment {
        output.push(window_descriptor_for_log(window_log))?;
    }

    match dict_id {
        Some(id) if id <= 0xFF => output.push(id as u8)?,
        Some(id) if id <= 0xFFFF => output.extend_from_slice(&(id as u16).to_le_bytes())?,
        Some(id) => output.extend_from_slice(&id.to_le_bytes())?,
        None => {}
    }

    let Some(content_size) = content_size else {
        return Ok(());
    };
    match fcs_size {
        0 => {}
        1 => output.push(content_size as u8)?,
        2 => {
            let v = (content_size - 256) as u16;
            output.extend_from_slice(&v.to_le_bytes())?;
        }
        4 => output.extend_from_slice(&(content_size as u32).to_le_bytes())?,
        _ => output.extend_from_slice(&(content_size as u64).to_le_bytes())?,
    }
    Ok(())
}

fn frame_content_size_field_size(content_size: usize, single_segment: bool) -> usize {
    if single_segment && content_size <= 255 {
        1
    } else if (256..=0xFFFF + 256).contains(&content_size) {
        2
    } else if content_size <= 0xFFFF_FFFF {
        4
    } else {
        8
    }
}

fn window_descriptor_for_log(window_log: u32) -> u8 {
    let window_log = window_log.clamp(strategy::WINDOW_LOG_MIN, strategy::WINDOW_LOG_MAX);
    ((window_log - 10) as u8) << 3
}

/// Whether to store `data`, a block of an `input_len`-byte input, raw without
/// a match search.
///
/// Inputs that keep raw literals skip the sampling: it costs about as much as
/// their match search, and a block without matches is stored raw anyway.
pub(crate) fn skip_match_search(
    params: &strategy::LevelParams,
    input_len: usize,
    data: &[u8],
) -> bool {
    !strategy::keeps_raw_literals(params, input_len) && block_looks_incompressible(data)
}

fn block_looks_incompressible(data: &[u8]) -> bool {
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
    params.hash_log = params
        .hash_log
        .clamp(strategy::HASH_LOG_MIN, strategy::HASH_LOG_MAX);
    params.chain_log = params
        .chain_log
        .clamp(strategy::HASH_LOG_MIN, strategy::HASH_LOG_MAX);
    params.window_log = params
        .window_log
        .clamp(strategy::WINDOW_LOG_MIN, strategy::WINDOW_LOG_MAX);
    if src_len >= 2 {
        let src_log = 32 - ((src_len as u32) - 1).leading_zeros();
        params.hash_log = params.hash_log.min(src_log).max(strategy::HASH_LOG_MIN);
        params.chain_log = params.chain_log.min(src_log).max(strategy::HASH_LOG_MIN);
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
    let params = strategy::level_params_for_size(level, input.len())
        .ok_or(CompressError::InvalidLevel(level))?;
    compress_inner(input, &params)
}

pub fn compress_opts(
    input: &[u8],
    level: i32,
    opts: &strategy::Options,
) -> Result<Vec<u8>, CompressError> {
    let mut params = strategy::level_params_for_size(level, input.len())
        .ok_or(CompressError::InvalidLevel(level))?;
    strategy::apply_options(&mut params, opts);
    clamp_params_to_src_size(&mut params, input.len());
    compress_inner(input, &params)
}

#[allow(clippy::unnecessary_wraps)]
fn compress_inner(input: &[u8], params: &strategy::LevelParams) -> Result<Vec<u8>, CompressError> {
    let mut output = Vec::with_capacity(input.len() + 32);
    compress_frame(input, params, &mut output)?;
    Ok(output)
}

fn compress_frame(
    input: &[u8],
    params: &strategy::LevelParams,
    output: &mut impl OutputSink,
) -> Result<(), CompressError> {
    write_frame_header(output, input.len(), None, params.window_log)?;

    if input.is_empty() {
        block_encoder::encode_raw_block(&[], true, output)?;
    } else {
        let mut rep_offsets = [1u32, 4, 8];
        let mut offset = 0;
        let mut sequences = Vec::with_capacity(MAX_BLOCK_SIZE / 8);
        let mut workspace = block_encoder::BlockEncodeWorkspace::new();

        #[cfg(feature = "ldm")]
        let mut ldm_state = params.ldm_params.as_ref().map(ldm::LdmState::new);

        match params.strategy {
            Strategy::Fast => {
                let hash_size = 1usize << params.hash_log;
                let mut hash_table = vec![0u32; hash_size];
                while offset < input.len() {
                    let chunk_size = (input.len() - offset).min(MAX_BLOCK_SIZE);
                    let block_end = offset + chunk_size;
                    let is_last = block_end >= input.len();
                    let block = &input[offset..block_end];

                    if skip_match_search(params, input.len(), block) {
                        block_encoder::encode_raw_block(block, is_last, output)?;
                    } else {
                        #[cfg(feature = "ldm")]
                        let used_ldm = if let Some(ref mut ldm) = ldm_state {
                            let mut empty = Vec::new();
                            ldm.compress_block(
                                input,
                                offset,
                                block_end,
                                params,
                                &rep_offsets,
                                &mut hash_table,
                                &mut empty,
                                &mut sequences,
                            );
                            true
                        } else {
                            false
                        };
                        #[cfg(not(feature = "ldm"))]
                        let used_ldm = false;

                        if !used_ldm {
                            fast::compress_fast_block(
                                input,
                                offset,
                                block_end,
                                params,
                                &rep_offsets,
                                &mut hash_table,
                                &mut sequences,
                            );
                        }
                        if params.force_raw_literals {
                            block_encoder::encode_compressed_block_raw(
                                block,
                                &sequences,
                                &mut rep_offsets,
                                is_last,
                                output,
                                &mut workspace,
                            )?;
                        } else {
                            block_encoder::encode_compressed_block(
                                block,
                                &sequences,
                                &mut rep_offsets,
                                is_last,
                                output,
                                &mut workspace,
                                strategy::block_policy(params, input.len()),
                            )?;
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
                    let block = &input[offset..block_end];

                    if skip_match_search(params, input.len(), block) {
                        block_encoder::encode_raw_block(block, is_last, output)?;
                    } else {
                        #[cfg(feature = "ldm")]
                        let used_ldm = if let Some(ref mut ldm) = ldm_state {
                            ldm.compress_block(
                                input,
                                offset,
                                block_end,
                                params,
                                &rep_offsets,
                                &mut hash_short,
                                &mut hash_long,
                                &mut sequences,
                            );
                            true
                        } else {
                            false
                        };
                        #[cfg(not(feature = "ldm"))]
                        let used_ldm = false;

                        if !used_ldm {
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
                        }
                        block_encoder::encode_compressed_block(
                            block,
                            &sequences,
                            &mut rep_offsets,
                            is_last,
                            output,
                            &mut workspace,
                            strategy::block_policy(params, input.len()),
                        )?;
                    }
                    offset = block_end;
                }
            }
        }
    }

    let hash = xxh64(input, 0);
    let checksum = (hash & 0xFFFF_FFFF) as u32;
    output.extend_from_slice(&checksum.to_le_bytes())?;
    Ok(())
}

pub fn compress_with_dict(
    input: &[u8],
    level: i32,
    dict: &zrip_core::dict::Dictionary,
) -> Result<Vec<u8>, CompressError> {
    let total_window = dict.content().len() + input.len();
    let params = strategy::level_params_for_size(level, total_window)
        .ok_or(CompressError::InvalidLevel(level))?;

    let mut output = Vec::with_capacity(input.len() + 32);
    write_frame_header(&mut output, input.len(), Some(dict.id()), params.window_log)?;

    if input.is_empty() {
        block_encoder::encode_raw_block(&[], true, &mut output)?;
    } else {
        let prefix = dict.content();
        let mut rep_offsets = *dict.rep_offsets();
        let mut workspace = block_encoder::BlockEncodeWorkspace::new();

        workspace.prev_ll = dict
            .ll_table()
            .map(|(dt, al)| block_encoder::FseEncodeTable::from_decode_table(dt, al, 35));
        workspace.prev_of = dict
            .of_table()
            .map(|(dt, al)| block_encoder::FseEncodeTable::from_decode_table(dt, al, 31));
        workspace.prev_ml = dict
            .ml_table()
            .map(|(dt, al)| block_encoder::FseEncodeTable::from_decode_table(dt, al, 52));
        workspace.prev_huffman = dict.huf_table().and_then(|(dt, tl)| {
            zrip_core::huffman::encode::HuffmanEncodeTable::from_decode_table(dt, tl)
        });

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
                )?;
            } else {
                block_encoder::encode_compressed_block(
                    input,
                    &sequences,
                    &mut rep_offsets,
                    true,
                    &mut output,
                    &mut workspace,
                    strategy::block_policy(&params, input.len()),
                )?;
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
                            )?;
                        } else {
                            block_encoder::encode_compressed_block(
                                &input[offset..offset + chunk_size],
                                &sequences,
                                &mut rep_offsets,
                                is_last,
                                &mut output,
                                &mut workspace,
                                strategy::block_policy(&params, input.len()),
                            )?;
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
                        params.min_match,
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
                            strategy::block_policy(&params, input.len()),
                        )?;
                        offset += chunk_size;
                    }
                }
            }
        }
    }

    let hash = xxh64(input, 0);
    let checksum = (hash & 0xFFFF_FFFF) as u32;
    output.extend_from_slice(&checksum.to_le_bytes());

    Ok(output)
}

pub fn compress_into(input: &[u8], output: &mut [u8], level: i32) -> Result<usize, CompressError> {
    let params = strategy::level_params_for_size(level, input.len())
        .ok_or(CompressError::InvalidLevel(level))?;
    let mut sink = SliceSink::new(output);
    compress_frame(input, &params, &mut sink)?;
    Ok(sink.pos())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zrip_core::frame::header::parse_frame_header;

    #[test]
    fn clamp_params_normalizes_public_log_values() {
        let mut params = strategy::level_params(1).unwrap();
        params.hash_log = 0;
        params.chain_log = 40;
        params.window_log = 40;

        clamp_params_to_src_size(&mut params, usize::MAX);

        assert_eq!(params.hash_log, strategy::HASH_LOG_MIN);
        assert_eq!(params.chain_log, strategy::HASH_LOG_MAX);
        assert_eq!(params.window_log, strategy::WINDOW_LOG_MAX);
    }

    #[test]
    fn options_clamp_window_log_before_ldm_defaults() {
        let mut params = strategy::level_params(1).unwrap();
        let opts = strategy::Options::default().window_log(0);

        strategy::apply_options(&mut params, &opts);

        assert_eq!(params.window_log, strategy::WINDOW_LOG_MIN);
        #[cfg(feature = "ldm")]
        {
            let mut params = strategy::level_params(1).unwrap();
            let opts = strategy::Options::default().window_log(0).ldm(true);
            strategy::apply_options(&mut params, &opts);

            let ldm = params.ldm_params.unwrap();
            assert!(ldm.hash_log >= ldm.bucket_size_log);
        }
    }

    /// Bytes from a skewed alphabet: about 3.5 bits of order-0 entropy per
    /// byte and few repeated 6-byte strings.
    fn skewed_bytes(len: usize) -> Vec<u8> {
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let r = (x >> 32) as u32;
                b"eeeeeeettttaaoonnissshhrdl  "[(r % 28) as usize] + (r >> 30) as u8
            })
            .collect()
    }

    /// Words from a 64-word vocabulary. Repeats are short and sparse, so a
    /// sparse match search misses most of them.
    fn wordy_bytes(len: usize) -> Vec<u8> {
        const WORDS: &str = "the of and to in is was for on that with as by at \
            from his her they this have had were which their are but not one \
            all been when there she would what so if will more no out up into \
            could them than then some other time very about only upon over \
            such said great before after little";
        let words: Vec<&str> = WORDS.split_whitespace().collect();
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut out = Vec::with_capacity(len + 16);
        while out.len() < len {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            out.extend_from_slice(words[(x >> 58) as usize % words.len()].as_bytes());
            out.push(b' ');
        }
        out.truncate(len);
        out
    }

    #[test]
    fn negative_levels_skip_huffman_on_tiny_inputs() {
        // No usable matches: only Huffman could shrink these bytes.
        let data = skewed_bytes(2048);
        for level in -8..=-1 {
            let out = compress(&data, level).unwrap();
            assert!(out.len() > data.len(), "L{level}: {} compressed", out.len());
        }
        for level in 1..=4 {
            let out = compress(&data, level).unwrap();
            assert!(out.len() * 5 < data.len() * 4, "L{level}: {}", out.len());
        }
    }

    #[test]
    fn negative_levels_still_match_tiny_inputs() {
        let data = wordy_bytes(2048);
        let sizes: Vec<usize> = (-8..=-1)
            .map(|level| compress(&data, level).unwrap().len())
            .collect();
        // At least 10% smaller, and no smaller output at a faster level.
        assert!(sizes.iter().all(|&n| n * 11 < data.len() * 10), "{sizes:?}");
        assert!(sizes.windows(2).all(|w| w[0] >= w[1]), "{sizes:?}");
    }

    #[test]
    fn negative_levels_huffman_code_above_their_raw_limit() {
        let data = skewed_bytes(32 * 1024);
        for level in -8..=-1 {
            let out = compress(&data, level).unwrap();
            assert!(out.len() * 5 < data.len() * 4, "L{level}: {}", out.len());
        }
    }

    #[test]
    fn faster_levels_keep_raw_literals_longer() {
        let limits: Vec<usize> = (-8..=-1)
            .map(|level| strategy::raw_literals_limit(level).unwrap())
            .collect();
        assert_eq!(limits.last(), Some(&2048));
        assert!(limits.windows(2).all(|w| w[0] > w[1]), "{limits:?}");
        assert_eq!(strategy::raw_literals_limit(1), None);
    }

    #[test]
    fn small_plain_frame_uses_single_segment_header() {
        let mut output = Vec::new();

        write_frame_header(&mut output, 12, None, 19).unwrap();
        let header = parse_frame_header(&output).unwrap();

        assert!(header.single_segment);
        assert_eq!(header.frame_content_size, Some(12));
        assert_eq!(header.window_size, 12);
        assert_eq!(header.dict_id, None);
        assert!(header.content_checksum);
        assert_eq!(header.header_size, 6);
    }

    #[test]
    fn large_plain_frame_uses_bounded_window_descriptor() {
        let mut output = Vec::new();
        let content_size = MAX_WINDOW_SIZE as usize + 1;

        write_frame_header(&mut output, content_size, None, 19).unwrap();
        let header = parse_frame_header(&output).unwrap();

        assert!(!header.single_segment);
        assert_eq!(header.frame_content_size, Some(content_size as u64));
        assert_eq!(header.window_size, 1 << 19);
        assert_eq!(header.dict_id, None);
        assert!(header.content_checksum);
        assert_eq!(header.header_size, 10);
    }

    #[test]
    fn dict_frame_uses_window_descriptor_even_when_small() {
        let mut output = Vec::new();

        write_frame_header(&mut output, 12, Some(0x1234), 10).unwrap();
        let header = parse_frame_header(&output).unwrap();

        assert!(!header.single_segment);
        assert_eq!(header.frame_content_size, Some(12));
        assert_eq!(header.window_size, 1 << 10);
        assert_eq!(header.dict_id, Some(0x1234));
        assert!(header.content_checksum);
        assert_eq!(header.header_size, 12);
    }

    #[test]
    fn no_fcs_frame_uses_window_descriptor() {
        let mut output = Vec::new();

        write_frame_header_without_content_size(&mut output, None, 19).unwrap();
        let header = parse_frame_header(&output).unwrap();

        assert!(!header.single_segment);
        assert_eq!(header.frame_content_size, None);
        assert_eq!(header.window_size, 1 << 19);
        assert_eq!(header.dict_id, None);
        assert!(header.content_checksum);
        assert_eq!(header.header_size, 6);
    }
}
