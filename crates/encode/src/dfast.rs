#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::primitives;
use crate::strategy::LevelParams;
use zrip_core::Sequence;
use zrip_core::hash::PRIME64_1;

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
    macro_rules! dispatch {
        ($hl:expr, $sl:expr) => {
            compress_dfast_block_impl::<$hl, $sl>(
                src,
                block_start,
                block_end,
                params,
                rep_offsets,
                hash_short,
                hash_long,
                sequences,
            )
        };
    }
    dispatch!(0, 0);
}

/// 4-cursor DFast match finder with prefetch pipeline.
///
/// Port of C zstd's 4-cursor pattern from ZSTD_compressBlock_fast to dual
/// hash tables.  Pipeline: ip0, ip1=ip0+1, ip2=ip0+step, ip3=ip2+1.  Each
/// iteration probes two positions, reusing hash computations across shifts
/// and prefetching both hash_short and hash_long for the next position.
#[allow(unused_assignments, unused_variables, clippy::too_many_arguments)]
fn compress_dfast_block_impl<const HASH_LOG: u32, const SHORT_LOG: u32>(
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
    if block_size < 16 {
        return;
    }

    let acceleration = params.target_length.max(1) as usize;
    let step_size = acceleration + 1;
    let search_strength = params.search_strength as usize;
    let search_log = params.search_log;
    let ilimit = block_end - 8;
    let max_distance = 1usize << params.window_log;

    let probe_interval = (block_size / 4).max(4096).min(block_size);
    let mut probe_limit = block_start + probe_interval;
    let mut total_match_bytes: usize = 0;

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

    let mut rep0 = rep_offsets[0];
    let mut rep1 = rep_offsets[1];
    let mut rep2 = rep_offsets[2];
    let mut anchor = block_start;
    let mut ip0 = block_start;

    primitives::assert_rep_valid(rep0, rep1);

    macro_rules! update_hashes_inline {
        ($pos:expr) => {{
            let pos = $pos;
            if pos + 8 <= src.len() {
                let hs = h5(primitives::rd64(src, pos), short_log);
                primitives::hash_store(hash_short, hs, pos as u32);
                let hl = h8(primitives::rd64(src, pos), hash_log);
                primitives::hash_store(hash_long, hl, pos as u32);
            }
        }};
    }

    macro_rules! insert_comp_inline {
        ($match_start:expr, $match_end:expr) => {{
            let ms = $match_start;
            let me = $match_end;
            let step = 1usize << search_log;
            let safe_end = me.min(src.len().saturating_sub(7));
            let cap_end = safe_end.min(ms + 2 + step * 4);
            let mut pos = ms + 2;
            while pos < cap_end {
                let hs = h5(primitives::rd64(src, pos), short_log);
                primitives::hash_store(hash_short, hs, pos as u32);
                let hl = h8(primitives::rd64(src, pos), hash_log);
                primitives::hash_store(hash_long, hl, pos as u32);
                pos += step;
            }
            if me >= 2 {
                let tail = me - 2;
                if tail < safe_end && tail >= cap_end {
                    let hs = h5(primitives::rd64(src, tail), short_log);
                    primitives::hash_store(hash_short, hs, tail as u32);
                    let hl = h8(primitives::rd64(src, tail), hash_log);
                    primitives::hash_store(hash_long, hl, tail as u32);
                }
            }
        }};
    }

    macro_rules! rep_match_loop_inline {
        () => {{
            loop {
                if ip0 >= ilimit {
                    break;
                }
                let r1 = rep1 as usize;
                if (r1 > 0) & (ip0 >= r1)
                    && primitives::rd32(src, ip0) == primitives::rd32(src, ip0 - r1)
                {
                    let ml = primitives::count_match(src, ip0 + 4, ip0 - r1 + 4, block_end) + 4;
                    core::mem::swap(&mut rep0, &mut rep1);
                    total_match_bytes += ml;
                    sequences.push(Sequence {
                        literal_length: 0,
                        offset: r1 as u32,
                        match_length: ml as u32,
                    });
                    ip0 += ml;
                    anchor = ip0;
                    update_hashes_inline!(ip0);
                    continue;
                }
                break;
            }
        }};
    }

    'start: loop {
        let mut ip1 = ip0 + 1;
        let mut ip2 = ip0 + step_size;
        let mut ip3 = ip2 + 1;

        if ip3 >= ilimit {
            break;
        }

        let mut hs0 = h5(primitives::rd64(src, ip0), short_log);
        let mut hl0 = h8(primitives::rd64(src, ip0), hash_log);
        let mut hs1 = h5(primitives::rd64(src, ip1), short_log);
        let mut hl1 = h8(primitives::rd64(src, ip1), hash_log);

        let mut match_short = primitives::hash_load(hash_short, hs0) as usize;
        let mut match_long = primitives::hash_load(hash_long, hl0) as usize;

        loop {
            // --- Store hashes for ip0 ---
            primitives::hash_store(hash_short, hs0, ip0 as u32);
            primitives::hash_store(hash_long, hl0, ip0 as u32);

            // --- Rep check at step-ahead position ip2 ---
            {
                let r0 = rep0 as usize;
                if ip2 >= r0 {
                    let v = primitives::rd32(src, ip2);
                    if v == primitives::rd32(src, ip2 - r0) {
                        let fill_pos = ip0;
                        primitives::hash_store(hash_short, hs1, ip1 as u32);
                        primitives::hash_store(hash_long, hl1, ip1 as u32);
                        ip0 = ip2;
                        if ip0 > anchor
                            && primitives::src_byte(src, ip0 - 1)
                                == primitives::src_byte(src, ip0 - r0 - 1)
                        {
                            ip0 -= 1;
                        }
                        let back = ip2 - ip0;
                        let mlen = primitives::count_match(src, ip2 + 4, ip2 - r0 + 4, block_end)
                            + 4
                            + back;
                        total_match_bytes += mlen;
                        sequences.push(Sequence {
                            literal_length: (ip0 - anchor) as u32,
                            offset: r0 as u32,
                            match_length: mlen as u32,
                        });
                        ip0 += mlen;
                        anchor = ip0;
                        if ip0 <= ilimit {
                            update_hashes_inline!(fill_pos + 2);
                            update_hashes_inline!(ip0 - 2);
                            rep_match_loop_inline!();
                        }
                        continue 'start;
                    }
                }
            }

            // --- Long match check at ip0 ---
            if match_long < ip0
                && ip0 - match_long <= max_distance
                && primitives::rd64(src, ip0) == primitives::rd64(src, match_long)
            {
                primitives::hash_store(hash_short, hs1, ip1 as u32);
                primitives::hash_store(hash_long, hl1, ip1 as u32);
                let mut back = 0usize;
                while ip0 > anchor + back
                    && match_long > back + block_start
                    && primitives::src_byte(src, ip0 - back - 1)
                        == primitives::src_byte(src, match_long - back - 1)
                {
                    back += 1;
                }
                let match_start = ip0 - back;
                let mlen =
                    primitives::count_match(src, ip0 + 8, match_long + 8, block_end) + 8 + back;
                total_match_bytes += mlen;
                let offset = (match_start - (match_long - back)) as u32;
                sequences.push(Sequence {
                    literal_length: (match_start - anchor) as u32,
                    offset,
                    match_length: mlen as u32,
                });
                rep2 = rep1;
                rep1 = rep0;
                rep0 = offset;
                ip0 += mlen - back;
                anchor = ip0;
                if ip0 <= ilimit {
                    insert_comp_inline!(match_start, ip0);
                    update_hashes_inline!(ip0);
                    rep_match_loop_inline!();
                }
                continue 'start;
            }

            // --- Short match check at ip0 ---
            if match_short < ip0
                && ip0 - match_short <= max_distance
                && primitives::rd32(src, ip0) == primitives::rd32(src, match_short)
            {
                // ip+1 lookahead: prefer long match at ip1 if significantly better
                if ip1 < ilimit {
                    let ml_next = primitives::hash_load(hash_long, hl1) as usize;
                    if ml_next < ip1
                        && ip1 - ml_next <= max_distance
                        && primitives::rd64(src, ip1) == primitives::rd64(src, ml_next)
                    {
                        let long_len =
                            primitives::count_match(src, ip1 + 8, ml_next + 8, block_end) + 8;
                        let short_len =
                            primitives::count_match(src, ip0 + 4, match_short + 4, block_end) + 4;

                        if long_len > short_len + 1 {
                            primitives::hash_store(hash_short, hs1, ip1 as u32);
                            primitives::hash_store(hash_long, hl1, ip1 as u32);
                            ip0 = ip1;
                            let mut back = 0usize;
                            while ip0 > anchor + back
                                && ml_next > back + block_start
                                && primitives::src_byte(src, ip0 - back - 1)
                                    == primitives::src_byte(src, ml_next - back - 1)
                            {
                                back += 1;
                            }
                            let match_start = ip0 - back;
                            total_match_bytes += long_len + back;
                            let offset = (match_start - (ml_next - back)) as u32;
                            sequences.push(Sequence {
                                literal_length: (match_start - anchor) as u32,
                                offset,
                                match_length: (long_len + back) as u32,
                            });
                            rep2 = rep1;
                            rep1 = rep0;
                            rep0 = offset;
                            ip0 += long_len;
                            anchor = ip0;
                            if ip0 <= ilimit {
                                insert_comp_inline!(match_start, ip0);
                                update_hashes_inline!(ip0);
                                rep_match_loop_inline!();
                            }
                            continue 'start;
                        }
                    }
                }

                // Take short match at ip0
                primitives::hash_store(hash_short, hs1, ip1 as u32);
                primitives::hash_store(hash_long, hl1, ip1 as u32);
                let mut mlen =
                    primitives::count_match(src, ip0 + 4, match_short + 4, block_end) + 4;
                let mut back = 0usize;
                while ip0 > anchor + back
                    && match_short > back + block_start
                    && primitives::src_byte(src, ip0 - back - 1)
                        == primitives::src_byte(src, match_short - back - 1)
                {
                    back += 1;
                }
                let match_start = ip0 - back;
                mlen += back;
                total_match_bytes += mlen;
                let offset = (match_start - (match_short - back)) as u32;
                sequences.push(Sequence {
                    literal_length: (match_start - anchor) as u32,
                    offset,
                    match_length: mlen as u32,
                });
                rep2 = rep1;
                rep1 = rep0;
                rep0 = offset;
                ip0 += mlen - back;
                anchor = ip0;
                if ip0 <= ilimit {
                    insert_comp_inline!(match_start, ip0);
                    update_hashes_inline!(ip0);
                    rep_match_loop_inline!();
                }
                continue 'start;
            }

            // === First shift ===
            match_short = primitives::hash_load(hash_short, hs1) as usize;
            match_long = primitives::hash_load(hash_long, hl1) as usize;
            hs0 = hs1;
            hl0 = hl1;

            hs1 = h5(primitives::rd64(src, ip2), short_log);
            hl1 = h8(primitives::rd64(src, ip2), hash_log);
            ip0 = ip1;
            ip1 = ip2;
            ip2 = ip3;

            #[cfg(target_arch = "x86_64")]
            {
                primitives::prefetch_ht(hash_short, hs1);
                primitives::prefetch_ht(hash_long, hl1);
            }

            // --- Store hashes for shifted ip0 ---
            primitives::hash_store(hash_short, hs0, ip0 as u32);
            primitives::hash_store(hash_long, hl0, ip0 as u32);

            // --- Long match check at shifted ip0 ---
            if match_long < ip0
                && ip0 - match_long <= max_distance
                && primitives::rd64(src, ip0) == primitives::rd64(src, match_long)
            {
                if step_size + ((ip0 - anchor) >> search_strength) <= 4 {
                    primitives::hash_store(hash_short, hs1, ip1 as u32);
                    primitives::hash_store(hash_long, hl1, ip1 as u32);
                }
                let mut back = 0usize;
                while ip0 > anchor + back
                    && match_long > back + block_start
                    && primitives::src_byte(src, ip0 - back - 1)
                        == primitives::src_byte(src, match_long - back - 1)
                {
                    back += 1;
                }
                let match_start = ip0 - back;
                let mlen =
                    primitives::count_match(src, ip0 + 8, match_long + 8, block_end) + 8 + back;
                total_match_bytes += mlen;
                let offset = (match_start - (match_long - back)) as u32;
                sequences.push(Sequence {
                    literal_length: (match_start - anchor) as u32,
                    offset,
                    match_length: mlen as u32,
                });
                rep2 = rep1;
                rep1 = rep0;
                rep0 = offset;
                ip0 += mlen - back;
                anchor = ip0;
                if ip0 <= ilimit {
                    insert_comp_inline!(match_start, ip0);
                    update_hashes_inline!(ip0);
                    rep_match_loop_inline!();
                }
                continue 'start;
            }

            // --- Short match check at shifted ip0 ---
            if match_short < ip0
                && ip0 - match_short <= max_distance
                && primitives::rd32(src, ip0) == primitives::rd32(src, match_short)
            {
                // ip+1 lookahead (hash_long[hl1] was prefetched in the shift)
                if ip1 < ilimit {
                    let ml_next = primitives::hash_load(hash_long, hl1) as usize;
                    if ml_next < ip1
                        && ip1 - ml_next <= max_distance
                        && primitives::rd64(src, ip1) == primitives::rd64(src, ml_next)
                    {
                        let long_len =
                            primitives::count_match(src, ip1 + 8, ml_next + 8, block_end) + 8;
                        let short_len =
                            primitives::count_match(src, ip0 + 4, match_short + 4, block_end) + 4;

                        if long_len > short_len + 1 {
                            if step_size + ((ip0 - anchor) >> search_strength) <= 4 {
                                primitives::hash_store(hash_short, hs1, ip1 as u32);
                                primitives::hash_store(hash_long, hl1, ip1 as u32);
                            }
                            ip0 = ip1;
                            let mut back = 0usize;
                            while ip0 > anchor + back
                                && ml_next > back + block_start
                                && primitives::src_byte(src, ip0 - back - 1)
                                    == primitives::src_byte(src, ml_next - back - 1)
                            {
                                back += 1;
                            }
                            let match_start = ip0 - back;
                            total_match_bytes += long_len + back;
                            let offset = (match_start - (ml_next - back)) as u32;
                            sequences.push(Sequence {
                                literal_length: (match_start - anchor) as u32,
                                offset,
                                match_length: (long_len + back) as u32,
                            });
                            rep2 = rep1;
                            rep1 = rep0;
                            rep0 = offset;
                            ip0 += long_len;
                            anchor = ip0;
                            if ip0 <= ilimit {
                                insert_comp_inline!(match_start, ip0);
                                update_hashes_inline!(ip0);
                                rep_match_loop_inline!();
                            }
                            continue 'start;
                        }
                    }
                }

                // Take short match
                if step_size + ((ip0 - anchor) >> search_strength) <= 4 {
                    primitives::hash_store(hash_short, hs1, ip1 as u32);
                    primitives::hash_store(hash_long, hl1, ip1 as u32);
                }
                let mut mlen =
                    primitives::count_match(src, ip0 + 4, match_short + 4, block_end) + 4;
                let mut back = 0usize;
                while ip0 > anchor + back
                    && match_short > back + block_start
                    && primitives::src_byte(src, ip0 - back - 1)
                        == primitives::src_byte(src, match_short - back - 1)
                {
                    back += 1;
                }
                let match_start = ip0 - back;
                mlen += back;
                total_match_bytes += mlen;
                let offset = (match_start - (match_short - back)) as u32;
                sequences.push(Sequence {
                    literal_length: (match_start - anchor) as u32,
                    offset,
                    match_length: mlen as u32,
                });
                rep2 = rep1;
                rep1 = rep0;
                rep0 = offset;
                ip0 += mlen - back;
                anchor = ip0;
                if ip0 <= ilimit {
                    insert_comp_inline!(match_start, ip0);
                    update_hashes_inline!(ip0);
                    rep_match_loop_inline!();
                }
                continue 'start;
            }

            // === Second shift with step gap ===
            match_short = primitives::hash_load(hash_short, hs1) as usize;
            match_long = primitives::hash_load(hash_long, hl1) as usize;
            hs0 = hs1;
            hl0 = hl1;

            hs1 = h5(primitives::rd64(src, ip2), short_log);
            hl1 = h8(primitives::rd64(src, ip2), hash_log);
            ip0 = ip1;
            ip1 = ip2;
            let step = step_size + ((ip0 - anchor) >> search_strength);
            ip2 = ip0 + step;
            ip3 = ip1 + step;

            #[cfg(target_arch = "x86_64")]
            {
                primitives::prefetch_ht(hash_short, hs1);
                primitives::prefetch_ht(hash_long, hl1);
            }

            if ip0 >= probe_limit {
                let scanned = ip0 - block_start;
                if total_match_bytes * 6 < scanned {
                    sequences.clear();
                    return;
                }
                probe_limit = probe_limit.saturating_add(probe_interval).min(block_end);
            }

            if ip3 >= ilimit {
                break;
            }
        }
        break;
    }
}

pub(crate) fn prefill_hash_tables(
    combined: &[u8],
    prefix_len: usize,
    hash_log: u32,
    short_log: u32,
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
        let hs = h5(primitives::rd64(combined, i), short_log);
        primitives::hash_store(hash_short, hs, i as u32);
        let hl = h8(primitives::rd64(combined, i), hash_log);
        primitives::hash_store(hash_long, hl, i as u32);
        i += step;
    }
    let tail_start = prefix_len.saturating_sub(64);
    for i in tail_start..prefix_len.saturating_sub(7) {
        let hs = h5(primitives::rd64(combined, i), short_log);
        primitives::hash_store(hash_short, hs, i as u32);
        let hl = h8(primitives::rd64(combined, i), hash_log);
        primitives::hash_store(hash_long, hl, i as u32);
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
fn h5(val: u64, hash_log: u32) -> usize {
    ((val << 24).wrapping_mul(PRIME64_1) >> (64 - hash_log)) as usize
}

#[inline(always)]
fn h8(val: u64, hash_log: u32) -> usize {
    (val.wrapping_mul(PRIME64_1) >> (64 - hash_log)) as usize
}
