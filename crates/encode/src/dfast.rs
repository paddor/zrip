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

macro_rules! rd64_upto {
    ($src:expr, $pos:expr, $limit:expr) => {
        primitives::rd64_upto($src, $pos, $limit)
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

pub(crate) fn compress_dfast(
    src: &[u8],
    params: &LevelParams,
    rep_offsets: &[u32; 3],
) -> Vec<Sequence> {
    let short_size = 1usize << params.chain_log;
    let long_size = 1usize << params.hash_log;
    let mut hash_short = vec![0u32; short_size];
    let mut hash_long = vec![0u32; long_size];
    let mut sequences = Vec::new();
    compress_dfast_block(
        src,
        0,
        src.len(),
        params,
        rep_offsets,
        &mut hash_short,
        &mut hash_long,
        &mut sequences,
    );
    sequences
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn compress_dfast_block(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_short: &mut [u32],
    hash_long: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    crate::rebase::debug_assert_fits(block_end, params.window_log);
    // Constant table sizes let the compiler drop the bounds checks of table
    // accesses; other sizes take the generic path.
    macro_rules! dispatch {
        ($(($mls:literal, $hl:literal, $sl:literal)),*) => {
            match (short_mls(params.min_match), params.hash_log, params.chain_log) {
                $(($mls, $hl, $sl) => dfast_block::<$hl, $sl, $mls>(
                    src, block_start, block_end, params, rep_offsets, hash_short, hash_long, sequences,
                ),)*
                (4, _, _) => dfast_block::<0, 0, 4>(
                    src, block_start, block_end, params, rep_offsets, hash_short, hash_long, sequences,
                ),
                _ => dfast_block::<0, 0, 5>(
                    src, block_start, block_end, params, rep_offsets, hash_short, hash_long, sequences,
                ),
            }
        };
    }
    dispatch!(
        (5, 15, 15),
        (5, 16, 16),
        (5, 17, 16),
        (5, 17, 17),
        (5, 18, 18)
    );
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn dfast_block<const HASH_LOG: u32, const SHORT_LOG: u32, const MLS: u32>(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_short: &mut [u32],
    hash_long: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    simd_body!(compress_dfast_block_impl::<HASH_LOG, SHORT_LOG, MLS>(
        src,
        block_start,
        block_end,
        params,
        rep_offsets,
        hash_short,
        hash_long,
        sequences,
    ));
}

/// The four bytes after the first of eight bytes read with `rd64`.
#[inline(always)]
fn second4(v: u64) -> u32 {
    if cfg!(target_endian = "little") {
        (v >> 8) as u32
    } else {
        (v >> 24) as u32
    }
}

/// DFast match finder, after C zstd's `ZSTD_compressBlock_doubleFast`.
///
/// Each position probes the long table (8-byte hash) and the short table
/// (`MLS`-byte hash) and checks rep 0 one byte ahead. A short match also
/// tries the long table at the next position. The step between positions
/// grows by one every `1 << search_strength` bytes without a match.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn compress_dfast_block_impl<const HASH_LOG: u32, const SHORT_LOG: u32, const MLS: u32>(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_short: &mut [u32],
    hash_long: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    sequences.clear();
    let block_size = block_end - block_start;
    // The length check lets the compiler see `ilimit + 8 <= src.len()`.
    if block_size < 16 || src.len() < 16 {
        return;
    }
    let hash_log = if HASH_LOG != 0 {
        HASH_LOG
    } else {
        params.hash_log
    };
    let short_log = if SHORT_LOG != 0 {
        SHORT_LOG
    } else {
        params.chain_log
    };
    let hash_long = primitives::exact_table::<HASH_LOG>(hash_long);
    let hash_short = primitives::exact_table::<SHORT_LOG>(hash_short);
    let ilimit = (block_end - 8).min(src.len() - 8);
    let max_distance = 1usize << params.window_log;
    let step_incr = 1usize << params.search_strength;

    primitives::assert_rep_valid(rep_offsets[0], rep_offsets[1]);
    let mut rep0 = rep_offsets[0] as usize;
    let mut rep1 = rep_offsets[1] as usize;
    let mut anchor = block_start;
    let mut ip = block_start + usize::from(block_start == 0);

    // Table entries always lie behind the current position, so a candidate
    // is usable when it is at least `low`, the window start for the block
    // end, as in C zstd.
    let low = block_end.saturating_sub(max_distance);
    let in_window = |m: usize| m >= low;

    'outer: loop {
        let mut step = 1usize;
        let mut next_step = ip + step_incr;
        let mut ip1 = ip + step;
        if ip1 > ilimit {
            break;
        }
        let mut v0 = rd64_upto!(src, ip, ilimit);
        let mut hl0 = h8(v0, hash_log);
        let mut idxl0 = hash_load!(hash_long, hl0) as usize;

        loop {
            let curr = ip;
            let hs0 = short_hash(v0, MLS, short_log);
            let idxs0 = hash_load!(hash_short, hs0) as usize;
            hash_store!(hash_long, hl0, ip as u32);
            hash_store!(hash_short, hs0, ip as u32);

            // Rep 0 at ip + 1. A rep reaching before the input fails the
            // first test and reads a clamped position.
            let rep_pos = (ip + 1).wrapping_sub(rep0);
            if (ip + 1 >= rep0) & (rd32!(src, rep_pos.min(ilimit)) == second4(v0)) {
                let ml = count_match!(src, ip + 5, rep_pos + 4, block_end) + 4;
                ip += 1;
                sequences.push(Sequence {
                    literal_length: (ip - anchor) as u32,
                    offset: rep0 as u32,
                    match_length: ml as u32,
                });
                ip += ml;
                anchor = ip;
                after_match(
                    src,
                    curr,
                    &mut ip,
                    &mut anchor,
                    ilimit,
                    block_end,
                    &mut rep0,
                    &mut rep1,
                    hash_short,
                    hash_long,
                    hash_log,
                    short_log,
                    sequences,
                    MLS,
                );
                continue 'outer;
            }

            let v1 = rd64_upto!(src, ip1, ilimit);
            let hl1 = h8(v1, hash_log);

            // Long match at ip.
            let found = if in_window(idxl0) & (rd64!(src, idxl0.min(ilimit)) == v0) {
                let ml = count_match!(src, ip + 8, idxl0 + 8, block_end) + 8;
                Some((ip, idxl0, ml))
            } else {
                None
            };

            let idxl1 = hash_load!(hash_long, hl1) as usize;

            // Short match at ip, then a long match at ip1 if it is longer.
            let found = found.or_else(|| {
                if in_window(idxs0) & (rd32!(src, idxs0.min(ilimit)) == first4(v0)) {
                    let ml = count_match!(src, ip + 4, idxs0 + 4, block_end) + 4;
                    if in_window(idxl1) & (rd64!(src, idxl1.min(ilimit)) == v1) {
                        let l1 = count_match!(src, ip1 + 8, idxl1 + 8, block_end) + 8;
                        if l1 > ml {
                            return Some((ip1, idxl1, l1));
                        }
                    }
                    Some((ip, idxs0, ml))
                } else {
                    None
                }
            });

            if let Some((mip, m, ml)) = found {
                debug_assert!(m < mip, "table entry ahead of the search");
                let back = crate::fast::count_back(src, mip, m, (mip - anchor).min(m - low));
                let match_start = mip - back;
                let offset = mip - m;
                rep1 = rep0;
                rep0 = offset;
                // Safe while ip1 stays inside the match, which a short
                // match of four bytes guarantees for steps below four.
                if step < 4 {
                    hash_store!(hash_long, hl1, ip1 as u32);
                }
                sequences.push(Sequence {
                    literal_length: (match_start - anchor) as u32,
                    offset: offset as u32,
                    match_length: (ml + back) as u32,
                });
                ip = mip + ml;
                anchor = ip;
                after_match(
                    src,
                    curr,
                    &mut ip,
                    &mut anchor,
                    ilimit,
                    block_end,
                    &mut rep0,
                    &mut rep1,
                    hash_short,
                    hash_long,
                    hash_log,
                    short_log,
                    sequences,
                    MLS,
                );
                continue 'outer;
            }

            if unlikely(ip1 >= next_step) {
                step += 1;
                next_step += step_incr;
            }
            ip = ip1;
            ip1 += step;
            hl0 = hl1;
            idxl0 = idxl1;
            v0 = v1;
            if ip1 > ilimit {
                break 'outer;
            }
        }
    }
}

/// Complementary insertion and immediate rep 1 matches after a match that
/// ended at `ip`, found while searching at `curr`.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn after_match(
    src: &[u8],
    curr: usize,
    ip: &mut usize,
    anchor: &mut usize,
    ilimit: usize,
    block_end: usize,
    rep0: &mut usize,
    rep1: &mut usize,
    hash_short: &mut [u32],
    hash_long: &mut [u32],
    hash_log: u32,
    short_log: u32,
    sequences: &mut Vec<Sequence>,
    mls: u32,
) {
    if *ip > ilimit {
        return;
    }
    let short = |v: u64| short_hash(v, mls, short_log);
    let i = curr + 2;
    let vi = rd64_upto!(src, i, ilimit);
    hash_store!(hash_long, h8(vi, hash_log), i as u32);
    hash_store!(hash_short, short(vi), i as u32);
    hash_store!(
        hash_long,
        h8(rd64_upto!(src, *ip - 2, ilimit), hash_log),
        (*ip - 2) as u32
    );
    hash_store!(
        hash_short,
        short(rd64_upto!(src, *ip - 1, ilimit)),
        (*ip - 1) as u32
    );
    while *ip <= ilimit {
        let p = *ip;
        let r = *rep1;
        let v = rd64_upto!(src, p, ilimit);
        if !((p >= r) & (rd32!(src, p.wrapping_sub(r).min(ilimit)) == first4(v))) {
            break;
        }
        let rl = count_match!(src, p + 4, p - r + 4, block_end) + 4;
        core::mem::swap(rep0, rep1);
        hash_store!(hash_short, short(v), p as u32);
        hash_store!(hash_long, h8(v, hash_log), p as u32);
        sequences.push(Sequence {
            literal_length: 0,
            offset: r as u32,
            match_length: rl as u32,
        });
        *ip = p + rl;
        *anchor = *ip;
    }
}

pub(crate) fn prefill_hash_tables(
    combined: &[u8],
    prefix_len: usize,
    hash_log: u32,
    short_log: u32,
    min_match: u32,
    hash_short: &mut [u32],
    hash_long: &mut [u32],
) {
    hash_short.fill(0);
    hash_long.fill(0);
    if prefix_len < 8 {
        return;
    }
    let hash_size = hash_short.len();
    let step = (prefix_len / hash_size).max(1);
    let mut i = 0;
    while i + 8 <= prefix_len {
        let hs = short_hash(rd64!(combined, i), short_mls(min_match), short_log);
        hash_store!(hash_short, hs, i as u32);
        let hl = h8(rd64!(combined, i), hash_log);
        hash_store!(hash_long, hl, i as u32);
        i += step;
    }
    let tail_start = prefix_len.saturating_sub(64);
    for i in tail_start..prefix_len.saturating_sub(7) {
        let hs = short_hash(rd64!(combined, i), short_mls(min_match), short_log);
        hash_store!(hash_short, hs, i as u32);
        let hl = h8(rd64!(combined, i), hash_log);
        hash_store!(hash_long, hl, i as u32);
    }
}

pub(crate) fn compress_dfast_with_prefix(
    src: &[u8],
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    prefix: &[u8],
) -> Vec<Sequence> {
    if prefix.is_empty() {
        return compress_dfast(src, params, rep_offsets);
    }
    let short_size = 1usize << params.chain_log;
    let long_size = 1usize << params.hash_log;
    let mut hash_short = vec![0u32; short_size];
    let mut hash_long = vec![0u32; long_size];
    let mut sequences = Vec::new();
    let mut combined = Vec::new();
    compress_dfast_with_prefix_reuse(
        src,
        params,
        rep_offsets,
        prefix,
        &mut hash_short,
        &mut hash_long,
        &mut sequences,
        &mut combined,
    );
    sequences
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn compress_dfast_with_prefix_reuse(
    src: &[u8],
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    prefix: &[u8],
    hash_short: &mut [u32],
    hash_long: &mut [u32],
    sequences: &mut Vec<Sequence>,
    combined: &mut Vec<u8>,
) {
    combined.clear();
    combined.reserve(prefix.len() + src.len());
    combined.extend_from_slice(prefix);
    combined.extend_from_slice(src);

    let plen = prefix.len();
    prefill_hash_tables(
        combined,
        plen,
        params.hash_log,
        params.chain_log,
        params.min_match,
        hash_short,
        hash_long,
    );

    compress_dfast_block(
        combined,
        plen,
        combined.len(),
        params,
        rep_offsets,
        hash_short,
        hash_long,
        sequences,
    );
}

#[inline(always)]
fn h4(val: u32, hash_log: u32) -> usize {
    (val.wrapping_mul(PRIME32_1) >> (32 - hash_log)) as usize
}

/// Bytes hashed for the short table: `min_match`, limited to 4 or 5.
fn short_mls(min_match: u32) -> u32 {
    min_match.clamp(4, 5)
}

/// Short-table hash of the first `mls` of the eight bytes `v` read at a
/// position.
#[inline(always)]
fn short_hash(v: u64, mls: u32, short_log: u32) -> usize {
    if mls <= 4 {
        h4(first4(v), short_log)
    } else {
        ((v << (64 - 8 * mls)).wrapping_mul(PRIME64_1) >> (64 - short_log)) as usize
    }
}

#[inline(always)]
fn h8(val: u64, hash_log: u32) -> usize {
    (val.wrapping_mul(PRIME64_1) >> (64 - hash_log)) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_ahead_rep_at_offset_start_does_not_underflow_back_extension() {
        let src = b"abababababababab";
        let params = crate::strategy::level_params_for_size(3, src.len()).unwrap();
        let mut hash_short = vec![0u32; 1usize << params.chain_log];
        let mut hash_long = vec![0u32; 1usize << params.hash_log];
        let mut sequences = Vec::new();

        compress_dfast_block(
            src,
            0,
            src.len(),
            &params,
            &[2, 4, 8],
            &mut hash_short,
            &mut hash_long,
            &mut sequences,
        );

        assert_ne!(sequences.len(), 0);
    }
}
