#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::primitives;
use crate::primitives::first4;
use crate::strategy::LevelParams;
use zrip_core::Sequence;
use zrip_core::hash::{PRIME32_1, PRIME64_1};
use zrip_core::hint::unlikely;

macro_rules! rd32 {
    ($src:expr, $pos:expr) => {
        primitives::rd32($src, $pos)
    };
}

macro_rules! rd64 {
    ($src:expr, $pos:expr) => {
        primitives::rd64($src, $pos)
    };
}

macro_rules! rd64_again {
    ($src:expr, $pos:expr, $v:expr) => {
        primitives::rd64_again($src, $pos, $v)
    };
}

macro_rules! hash_load {
    ($table:expr, $idx:expr) => {
        primitives::hash_load($table, $idx)
    };
}

macro_rules! hash_store {
    ($table:expr, $idx:expr, $val:expr) => {
        primitives::hash_store($table, $idx, $val)
    };
}

macro_rules! count_match {
    ($src:expr, $p1:expr, $p2:expr, $limit:expr) => {
        primitives::count_match($src, $p1, $p2, $limit)
    };
}

pub(crate) fn compress_fast(
    src: &[u8],
    params: &LevelParams,
    rep_offsets: &[u32; 3],
) -> Vec<Sequence> {
    let hash_size = 1usize << params.hash_log;
    let mut hash_table = vec![0u32; hash_size];
    let mut sequences = Vec::new();
    compress_fast_block(
        src,
        0,
        src.len(),
        params,
        rep_offsets,
        &mut hash_table,
        &mut sequences,
    );
    sequences
}

pub(crate) fn compress_fast_block(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    crate::rebase::debug_assert_fits(block_end, params.window_log);
    sequences.clear();
    let mls = params.min_match as usize;
    match (mls, params.hash_log) {
        (4, 14) => compress_fast_block_h14_mls4(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
        (4, 15) => compress_fast_block_h15_mls4(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
        (4, 16) => compress_fast_block_h16_mls4(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
        (4, 17) => compress_fast_block_h17_mls4(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
        (8.., 13) => compress_fast_block_h13_mls8(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
        (8.., _) => compress_fast_block_mls8(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
        (7.., 13) => compress_fast_block_h13_mls7(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
        (7.., 14) => compress_fast_block_h14_mls7(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
        (7.., 15) => compress_fast_block_h15_mls7(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
        (7.., _) => compress_fast_block_mls7(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
        (6, 13) => compress_fast_block_mls6::<13>(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
        (6, 14) => compress_fast_block_mls6::<14>(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
        (6, 15) => compress_fast_block_mls6::<15>(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
        (6, 16) => compress_fast_block_mls6::<16>(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
        (6, _) => compress_fast_block_mls6::<0>(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
        (5..7, 13) => compress_fast_block_h13_mls5(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
        (5..7, _) => compress_fast_block_mls5(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
        _ => compress_fast_block_mls4(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            sequences,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn compress_fast_block_mls5(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    simd_body!(compress_fast_block_impl::<0, 5>(
        src,
        block_start,
        block_end,
        params,
        rep_offsets,
        hash_table,
        sequences,
    ));
}

#[allow(clippy::too_many_arguments)]
fn compress_fast_block_mls4(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    simd_body!(compress_fast_block_impl::<0, 4>(
        src,
        block_start,
        block_end,
        params,
        rep_offsets,
        hash_table,
        sequences,
    ));
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn compress_fast_block_mls6<const HASH_LOG: u32>(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    simd_body!(compress_fast_block_impl::<HASH_LOG, 6>(
        src,
        block_start,
        block_end,
        params,
        rep_offsets,
        hash_table,
        sequences,
    ));
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn compress_fast_block_h14_mls4(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    simd_body!(compress_fast_block_impl::<14, 4>(
        src,
        block_start,
        block_end,
        params,
        rep_offsets,
        hash_table,
        sequences,
    ));
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn compress_fast_block_h15_mls4(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    simd_body!(compress_fast_block_impl::<15, 4>(
        src,
        block_start,
        block_end,
        params,
        rep_offsets,
        hash_table,
        sequences,
    ));
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn compress_fast_block_h16_mls4(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    simd_body!(compress_fast_block_impl::<16, 4>(
        src,
        block_start,
        block_end,
        params,
        rep_offsets,
        hash_table,
        sequences,
    ));
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn compress_fast_block_h17_mls4(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    simd_body!(compress_fast_block_impl::<17, 4>(
        src,
        block_start,
        block_end,
        params,
        rep_offsets,
        hash_table,
        sequences,
    ));
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn compress_fast_block_h13_mls5(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    simd_body!(compress_fast_block_impl::<13, 5>(
        src,
        block_start,
        block_end,
        params,
        rep_offsets,
        hash_table,
        sequences,
    ));
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn compress_fast_block_h13_mls8(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    simd_body!(compress_fast_block_impl::<13, 8>(
        src,
        block_start,
        block_end,
        params,
        rep_offsets,
        hash_table,
        sequences,
    ));
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn compress_fast_block_mls8(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    simd_body!(compress_fast_block_impl::<0, 8>(
        src,
        block_start,
        block_end,
        params,
        rep_offsets,
        hash_table,
        sequences,
    ));
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn compress_fast_block_mls7(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    simd_body!(compress_fast_block_impl::<0, 7>(
        src,
        block_start,
        block_end,
        params,
        rep_offsets,
        hash_table,
        sequences,
    ));
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn compress_fast_block_h13_mls7(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    simd_body!(compress_fast_block_impl::<13, 7>(
        src,
        block_start,
        block_end,
        params,
        rep_offsets,
        hash_table,
        sequences,
    ));
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn compress_fast_block_h14_mls7(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    simd_body!(compress_fast_block_impl::<14, 7>(
        src,
        block_start,
        block_end,
        params,
        rep_offsets,
        hash_table,
        sequences,
    ));
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn compress_fast_block_h15_mls7(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    simd_body!(compress_fast_block_impl::<15, 7>(
        src,
        block_start,
        block_end,
        params,
        rep_offsets,
        hash_table,
        sequences,
    ));
}

/// A read-only hash table of dictionary content kept beside the input's own
/// table. Its positions lie below `prefix_len` in the combined buffer.
#[derive(Clone, Copy)]
pub(crate) struct AttachedDict<'a> {
    pub(crate) table: &'a [u32],
    pub(crate) hash_log: u32,
    pub(crate) prefix_len: usize,
}

impl AttachedDict<'_> {
    const NONE: AttachedDict<'static> = AttachedDict {
        table: &[],
        hash_log: 0,
        prefix_len: 0,
    };
}

#[inline(always)]
fn compress_fast_block_impl<const HASH_LOG: u32, const MLS: usize>(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    fast_block_generic::<HASH_LOG, MLS, false>(
        src,
        block_start,
        block_end,
        params,
        rep_offsets,
        hash_table,
        AttachedDict::NONE,
        sequences,
    );
}

/// Fast search over `combined[dict.prefix_len..block_end]`: the input's
/// positions go into `hash_table`, and a miss there falls back to the
/// dictionary table.
#[allow(clippy::too_many_arguments)]
pub(crate) fn compress_fast_block_attached(
    combined: &[u8],
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    dict: AttachedDict<'_>,
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    sequences.clear();
    let start = dict.prefix_len;
    match params.min_match {
        ..=4 => simd_body!(fast_block_generic::<0, 4, true>(
            combined,
            start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            dict,
            sequences,
        )),
        5 => simd_body!(fast_block_generic::<0, 5, true>(
            combined,
            start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            dict,
            sequences,
        )),
        6 => simd_body!(fast_block_generic::<0, 6, true>(
            combined,
            start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            dict,
            sequences,
        )),
        7 => simd_body!(fast_block_generic::<0, 7, true>(
            combined,
            start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            dict,
            sequences,
        )),
        _ => simd_body!(fast_block_generic::<0, 8, true>(
            combined,
            start,
            block_end,
            params,
            rep_offsets,
            hash_table,
            dict,
            sequences,
        )),
    }
}

/// C zstd-style 4-cursor match finder (port of ZSTD_compressBlock_fast_noDict_generic).
///
/// Pipeline: ip0, ip1=ip0+1, ip2=ip0+step, ip3=ip2+1.
/// Each iteration probes two positions (ip0 and shifted ip0), reusing hash
/// computations across shifts.  Rep offset checked at the step-ahead position
/// (ip2) only; rep2 checked in the post-match loop. With `DICT`, a probe that
/// misses the input table looks up the attached dictionary table.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn fast_block_generic<const HASH_LOG: u32, const MLS: usize, const DICT: bool>(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    dict: AttachedDict<'_>,
    sequences: &mut Vec<Sequence>,
) {
    // C zstd hashes MLS bytes, but the fast parser confirms 4 bytes and
    // extends from there, so MLS 5 to 7 set the hash width, not the shortest
    // match. MLS 8, zrip's own, hashes and confirms eight bytes: fewer,
    // longer matches make frames that decode faster.
    let confirm: usize = if MLS >= 8 {
        8
    } else if MLS >= 5 {
        4
    } else {
        MLS
    };

    let block_size = block_end - block_start;
    if block_size < 8 {
        return;
    }
    let (step_size, search_strength) = if HASH_LOG == 14 && MLS == 4 {
        (2usize, 8usize)
    } else {
        (
            params.target_length.max(1) as usize + 1,
            params.search_strength as usize,
        )
    };
    let skip_shift = if HASH_LOG == 14 && MLS == 4 {
        5
    } else if params.min_match == 4 && params.target_length == 1 {
        search_strength.min(3)
    } else {
        search_strength
    };
    let ilimit = (block_end - MLS).min(src.len() - 8);
    let max_distance = 1usize << params.window_log;

    let mut rep1 = rep_offsets[0] as usize;
    let mut rep2 = rep_offsets[1] as usize;
    let mut anchor = block_start;
    let mut ip0 = block_start;

    primitives::assert_rep_valid(rep1 as u32, rep2 as u32);

    let hash_log = if HASH_LOG != 0 {
        HASH_LOG
    } else {
        params.hash_log
    };
    let hash_table = primitives::exact_table::<HASH_LOG>(hash_table);

    'start: loop {
        let mut ip1 = ip0 + 1;
        let mut ip2 = ip0 + step_size;

        if unlikely(ip2 + 1 >= ilimit) {
            break;
        }

        // Bytes at ip0 and ip1 for the hashes, reused for the match checks,
        // skipping bounds checks.
        let mut v0 = rd64!(src, ip0);
        let mut v1 = rd64!(src, ip1);
        let mut h0 = hash_val::<HASH_LOG, MLS>(v0, hash_log);
        let mut h1 = hash_val::<HASH_LOG, MLS>(v1, hash_log);
        let mut match_idx = hash_load!(hash_table, h0) as usize;

        loop {
            // Write hash for ip0 (before any checks, matching C zstd order)
            hash_store!(hash_table, h0, ip0 as u32);
            let v2 = primitives::rd64_ahead(src, ip2);

            // Rep check at step-ahead position ip2
            if ip2 >= rep1 && confirms::<MLS>(src, ip2 - rep1, v2) {
                let fill_pos = ip0;
                ip0 = ip2;
                // 1-byte backward extension (C zstd style)
                if ip0 > anchor && ip0 > rep1 && src[ip0 - 1] == src[ip0 - rep1 - 1] {
                    ip0 -= 1;
                }
                hash_store!(hash_table, h1, ip1 as u32);
                let back = ip2 - ip0;
                let mlen = count_match!(src, ip2 + confirm, ip2 - rep1 + confirm, block_end)
                    + confirm
                    + back;
                let lit_len = (ip0 - anchor) as u32;
                sequences.push(Sequence {
                    literal_length: lit_len,
                    offset: rep1 as u32,
                    match_length: mlen as u32,
                });
                ip0 += mlen;
                anchor = ip0;
                if ip0 <= ilimit {
                    insert_hash_mls::<HASH_LOG, MLS>(src, fill_pos + 2, hash_log, hash_table);
                    insert_hash_mls::<HASH_LOG, MLS>(src, ip0 - 2, hash_log, hash_table);
                    rep2_match_loop::<HASH_LOG, MLS>(
                        src,
                        &mut ip0,
                        &mut anchor,
                        &mut rep1,
                        &mut rep2,
                        hash_log,
                        hash_table,
                        sequences,
                        ilimit,
                        block_end,
                    );
                }
                continue 'start;
            }

            // First match check at ip0
            if let Some(match_idx) =
                find_match::<MLS, DICT>(src, ip0, match_idx, v0, max_distance, dict)
            {
                hash_store!(hash_table, h1, ip1 as u32);
                let fill_pos = ip0;
                let back = count_back(src, ip0, match_idx, (ip0 - anchor).min(match_idx));
                let match_start = ip0 - back;
                let offset = (match_start - (match_idx - back)) as u32;
                let mlen = count_match!(src, ip0 + confirm, match_idx + confirm, block_end)
                    + confirm
                    + back;
                let lit_len = (match_start - anchor) as u32;
                sequences.push(Sequence {
                    literal_length: lit_len,
                    offset,
                    match_length: mlen as u32,
                });
                rep2 = rep1;
                rep1 = offset as usize;
                ip0 += mlen - back;
                anchor = ip0;
                if ip0 <= ilimit {
                    insert_hash_mls::<HASH_LOG, MLS>(src, fill_pos + 2, hash_log, hash_table);
                    insert_hash_mls::<HASH_LOG, MLS>(src, ip0 - 2, hash_log, hash_table);
                    rep2_match_loop::<HASH_LOG, MLS>(
                        src,
                        &mut ip0,
                        &mut anchor,
                        &mut rep1,
                        &mut rep2,
                        hash_log,
                        hash_table,
                        sequences,
                        ilimit,
                        block_end,
                    );
                }
                continue 'start;
            }

            // First shift: ip1 becomes ip0, hash ip2 as the new ip1
            match_idx = hash_load!(hash_table, h1) as usize;
            h0 = h1;
            let v2 = rd64_again!(src, ip2, v2);
            h1 = hash_val::<HASH_LOG, MLS>(v2, hash_log);
            v0 = v1;
            v1 = v2;
            ip0 = ip1;
            ip1 = ip2;
            ip2 += 1;

            // Write hash for shifted ip0
            hash_store!(hash_table, h0, ip0 as u32);

            // Second match check at shifted ip0
            if let Some(match_idx) =
                find_match::<MLS, DICT>(src, ip0, match_idx, v0, max_distance, dict)
            {
                if step_size + ((ip0 - anchor) >> search_strength) <= 4 {
                    hash_store!(hash_table, h1, ip1 as u32);
                }
                let fill_pos = ip0;
                let back = count_back(src, ip0, match_idx, (ip0 - anchor).min(match_idx));
                let match_start = ip0 - back;
                let offset = (match_start - (match_idx - back)) as u32;
                let mlen = count_match!(src, ip0 + confirm, match_idx + confirm, block_end)
                    + confirm
                    + back;
                let lit_len = (match_start - anchor) as u32;
                sequences.push(Sequence {
                    literal_length: lit_len,
                    offset,
                    match_length: mlen as u32,
                });
                rep2 = rep1;
                rep1 = offset as usize;
                ip0 += mlen - back;
                anchor = ip0;
                if ip0 <= ilimit {
                    insert_hash_mls::<HASH_LOG, MLS>(src, fill_pos + 2, hash_log, hash_table);
                    insert_hash_mls::<HASH_LOG, MLS>(src, ip0 - 2, hash_log, hash_table);
                    rep2_match_loop::<HASH_LOG, MLS>(
                        src,
                        &mut ip0,
                        &mut anchor,
                        &mut rep1,
                        &mut rep2,
                        hash_log,
                        hash_table,
                        sequences,
                        ilimit,
                        block_end,
                    );
                }
                continue 'start;
            }

            // Second shift with step gap
            match_idx = hash_load!(hash_table, h1) as usize;
            h0 = h1;
            let v2 = rd64!(src, ip2);
            h1 = hash_val::<HASH_LOG, MLS>(v2, hash_log);
            v0 = v1;
            v1 = v2;
            ip0 = ip1;
            ip1 = ip2;
            let step = step_size + ((ip0 - anchor) >> skip_shift);
            ip2 = ip0 + step;

            if unlikely(ip2 + 1 >= ilimit) {
                break;
            }
        }

        break;
    }
}

/// The input table's candidate `idx` for the eight bytes `v` at `ip` if it
/// confirms, else with `DICT` the dictionary table's.
#[inline(always)]
fn find_match<const MLS: usize, const DICT: bool>(
    src: &[u8],
    ip: usize,
    idx: usize,
    v: u64,
    max_distance: usize,
    dict: AttachedDict<'_>,
) -> Option<usize> {
    if in_window(ip, idx, max_distance) && confirms::<MLS>(src, idx, v) {
        return Some(idx);
    }
    if DICT {
        let idx = hash_load!(dict.table, hash_val::<0, MLS>(v, dict.hash_log)) as usize;
        if idx < dict.prefix_len && in_window(ip, idx, max_distance) && confirms::<MLS>(src, idx, v)
        {
            return Some(idx);
        }
    }
    None
}

/// Whether the bytes at `pos` start like `v`, the eight bytes read at the
/// cursor: all eight for MLS 8, else the first four.
#[inline(always)]
fn confirms<const MLS: usize>(src: &[u8], pos: usize, v: u64) -> bool {
    if MLS >= 8 {
        rd64!(src, pos) == v
    } else {
        rd32!(src, pos) == first4(v)
    }
}

/// Whether `idx` is an earlier position at most `max_distance` back from `ip`.
///
/// One wrapping compare covers both bounds: `idx >= ip` wraps to a huge
/// distance.
#[inline(always)]
pub(crate) fn in_window(ip: usize, idx: usize, max_distance: usize) -> bool {
    ip.wrapping_sub(idx).wrapping_sub(1) < max_distance
}

/// Counts equal bytes backward from `a` and `b` (exclusive), up to `max`.
///
/// Compares eight bytes per step and counts the equal prefix with
/// `leading_zeros`, so the common short extension costs one well-predicted
/// branch instead of a loop exit per byte.
#[inline(always)]
pub(crate) fn count_back(src: &[u8], a: usize, b: usize, max: usize) -> usize {
    debug_assert!(b < a && a <= src.len());
    let mut back = 0usize;
    while back < max {
        let pa = a - back;
        let pb = b - back;
        if pb < 8 {
            while back < max && src[a - back - 1] == src[b - back - 1] {
                back += 1;
            }
            return back;
        }
        let diff = rd64!(src, pa - 8) ^ rd64!(src, pb - 8);
        if diff != 0 {
            return (back + (diff.leading_zeros() / 8) as usize).min(max);
        }
        back += 8;
    }
    max
}

/// Post-match rep2 loop (C zstd style): only check rep_offset2, swap on match.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn rep2_match_loop<const HASH_LOG: u32, const MLS: usize>(
    src: &[u8],
    ip: &mut usize,
    anchor: &mut usize,
    rep1: &mut usize,
    rep2: &mut usize,
    hash_log: u32,
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
    ilimit: usize,
    block_end: usize,
) {
    if *rep2 == 0 {
        return;
    }
    while *ip <= ilimit && *ip >= *rep2 {
        let val = rd32!(src, *ip);
        let rval = rd32!(src, *ip - *rep2);
        if val != rval {
            break;
        }
        let rlen = count_match!(src, *ip + 4, *ip - *rep2 + 4, block_end) + 4;
        core::mem::swap(rep1, rep2);
        insert_hash_mls::<HASH_LOG, MLS>(src, *ip, hash_log, hash_table);
        sequences.push(Sequence {
            literal_length: 0,
            offset: *rep1 as u32,
            match_length: rlen as u32,
        });
        *ip += rlen;
        *anchor = *ip;
    }
}

/// Fills `hash_table` with dictionary positions below `prefix_len`, hashed
/// like the match finder at `min_match`.
pub(crate) fn prefill_hash_table(
    combined: &[u8],
    prefix_len: usize,
    hash_log: u32,
    min_match: u32,
    hash_table: &mut [u32],
) {
    match min_match {
        ..=4 => prefill::<4>(combined, prefix_len, hash_log, hash_table),
        5 => prefill::<5>(combined, prefix_len, hash_log, hash_table),
        6 => prefill::<6>(combined, prefix_len, hash_log, hash_table),
        7 => prefill::<7>(combined, prefix_len, hash_log, hash_table),
        _ => prefill::<8>(combined, prefix_len, hash_log, hash_table),
    }
}

fn prefill<const MLS: usize>(
    combined: &[u8],
    prefix_len: usize,
    hash_log: u32,
    hash_table: &mut [u32],
) {
    hash_table.fill(0);
    // Positions whose eight-byte read stays inside the buffer.
    let end = prefix_len.min(combined.len().saturating_sub(7));
    let step = (prefix_len / hash_table.len()).max(1);
    let mut i = 0;
    while i < end {
        hash_store!(
            hash_table,
            hash_pos::<0, MLS>(combined, i, hash_log),
            i as u32
        );
        i += step;
    }
    for i in end.saturating_sub(64)..end {
        hash_store!(
            hash_table,
            hash_pos::<0, MLS>(combined, i, hash_log),
            i as u32
        );
    }
}

pub(crate) fn compress_fast_with_prefix(
    src: &[u8],
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    prefix: &[u8],
) -> Vec<Sequence> {
    if prefix.is_empty() {
        return compress_fast(src, params, rep_offsets);
    }
    let hash_size = 1usize << params.hash_log;
    let mut dict_hash = vec![0u32; hash_size];
    let mut input_hash = vec![0u32; hash_size];
    let mut sequences = Vec::new();
    let mut combined = Vec::new();
    compress_fast_with_prefix_reuse(
        src,
        params,
        rep_offsets,
        prefix,
        &mut dict_hash,
        &mut input_hash,
        &mut sequences,
        &mut combined,
    );
    sequences
}

/// Compresses `src` after `prefix` with the prefix in a frozen table beside
/// the input's. Both tables are `1 << params.hash_log` entries.
#[allow(clippy::too_many_arguments)]
pub(crate) fn compress_fast_with_prefix_reuse(
    src: &[u8],
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    prefix: &[u8],
    dict_hash: &mut [u32],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
    combined: &mut Vec<u8>,
) {
    combined.clear();
    combined.reserve(prefix.len() + src.len());
    combined.extend_from_slice(prefix);
    combined.extend_from_slice(src);

    prefill_hash_table(
        combined,
        prefix.len(),
        params.hash_log,
        params.min_match,
        dict_hash,
    );
    hash_table.fill(0);
    let dict = AttachedDict {
        table: dict_hash,
        hash_log: params.hash_log,
        prefix_len: prefix.len(),
    };
    compress_fast_block_attached(
        combined,
        combined.len(),
        params,
        rep_offsets,
        dict,
        hash_table,
        sequences,
    );
}

#[inline(always)]
fn hash4_const<const HASH_LOG: u32>(value: u32, hash_log: u32) -> usize {
    let hl = if HASH_LOG != 0 { HASH_LOG } else { hash_log };
    ((value.wrapping_mul(PRIME32_1)) >> (32 - hl)) as usize
}

#[inline(always)]
fn hash5_const<const HASH_LOG: u32>(value: u64, hash_log: u32) -> usize {
    let hl = if HASH_LOG != 0 { HASH_LOG } else { hash_log };
    (((value << 24).wrapping_mul(PRIME64_1)) >> (64 - hl)) as usize
}

const PRIME6: u64 = 227_718_039_650_203;

#[inline(always)]
fn hash6_const<const HASH_LOG: u32>(value: u64, hash_log: u32) -> usize {
    let hl = if HASH_LOG != 0 { HASH_LOG } else { hash_log };
    (((value << 16).wrapping_mul(PRIME6)) >> (64 - hl)) as usize
}

const PRIME7: u64 = 58_295_818_150_454_627;

#[inline(always)]
fn hash7_const<const HASH_LOG: u32>(value: u64, hash_log: u32) -> usize {
    let hl = if HASH_LOG != 0 { HASH_LOG } else { hash_log };
    (((value << 8).wrapping_mul(PRIME7)) >> (64 - hl)) as usize
}

#[inline(always)]
fn hash8_const<const HASH_LOG: u32>(value: u64, hash_log: u32) -> usize {
    let hl = if HASH_LOG != 0 { HASH_LOG } else { hash_log };
    (value.wrapping_mul(PRIME64_1) >> (64 - hl)) as usize
}

/// Hash of the eight bytes `v` read at a position.
#[inline(always)]
fn hash_val<const HASH_LOG: u32, const MLS: usize>(v: u64, hash_log: u32) -> usize {
    if MLS >= 8 {
        hash8_const::<HASH_LOG>(v, hash_log)
    } else if MLS >= 7 {
        hash7_const::<HASH_LOG>(v, hash_log)
    } else if MLS == 6 {
        hash6_const::<HASH_LOG>(v, hash_log)
    } else if MLS >= 5 {
        hash5_const::<HASH_LOG>(v, hash_log)
    } else {
        hash4_const::<HASH_LOG>(first4(v), hash_log)
    }
}

#[inline(always)]
fn hash_pos<const HASH_LOG: u32, const MLS: usize>(src: &[u8], pos: usize, hash_log: u32) -> usize {
    if MLS >= 5 {
        hash_val::<HASH_LOG, MLS>(rd64!(src, pos), hash_log)
    } else {
        hash4_const::<HASH_LOG>(rd32!(src, pos), hash_log)
    }
}

#[inline]
fn insert_hash_mls<const HASH_LOG: u32, const MLS: usize>(
    src: &[u8],
    ip: usize,
    hash_log: u32,
    hash_table: &mut [u32],
) {
    if ip + MLS <= src.len() {
        let h = hash_pos::<HASH_LOG, MLS>(src, ip, hash_log);
        hash_store!(hash_table, h, ip as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_ahead_rep_at_offset_start_does_not_underflow_back_extension() {
        let src = b"abababababababab";
        let params = crate::strategy::level_params_for_size(1, src.len()).unwrap();
        let mut hash_table = vec![0u32; 1usize << params.hash_log];
        let mut sequences = Vec::new();

        compress_fast_block(
            src,
            0,
            src.len(),
            &params,
            &[2, 4, 8],
            &mut hash_table,
            &mut sequences,
        );

        assert_ne!(sequences.len(), 0);
    }

    /// Offsets found for `pattern, run of one byte, pattern` by the
    /// `hash_log` 14, MLS 4 specialization. The run hashes to one slot, so
    /// the first pattern's entries survive any distance.
    fn h14_mls4_offsets(distance: usize, window_log: u32) -> Vec<u32> {
        let mut x = 1u32;
        let pattern: Vec<u8> = (0..4096)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 24) as u8
            })
            .collect();
        let mut src = pattern.clone();
        src.resize(distance, 0);
        src.extend_from_slice(&pattern);

        let mut params = crate::strategy::level_params(1).unwrap();
        params.hash_log = 14;
        params.min_match = 4;
        params.window_log = window_log;
        let mut hash_table = vec![0u32; 1usize << 14];
        let mut sequences = Vec::new();
        compress_fast_block(
            &src,
            0,
            src.len(),
            &params,
            &[1, 4, 8],
            &mut hash_table,
            &mut sequences,
        );
        sequences.iter().map(|s| s.offset).collect()
    }

    #[test]
    fn h14_mls4_honors_smaller_window() {
        const DISTANCE: usize = 400 << 10;
        let wide = h14_mls4_offsets(DISTANCE, 19);
        assert!(wide.contains(&(DISTANCE as u32)), "{wide:?}");
        let narrow = h14_mls4_offsets(DISTANCE, 18);
        assert!(narrow.iter().all(|&o| o <= 1 << 18), "{narrow:?}");
    }

    #[test]
    fn h14_mls4_honors_larger_window() {
        const DISTANCE: usize = 600 << 10;
        let offsets = h14_mls4_offsets(DISTANCE, 20);
        assert!(offsets.contains(&(DISTANCE as u32)), "{offsets:?}");
    }
}
