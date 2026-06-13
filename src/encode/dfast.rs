#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::decode::sequences::Sequence;
use crate::encode::strategy::LevelParams;
use crate::hash::PRIME32_1;
use crate::hash::PRIME64_1;

pub(crate) fn compress_dfast(
    src: &[u8],
    params: &LevelParams,
    rep_offsets: &[u32; 3],
) -> Vec<Sequence> {
    let hash_size = 1usize << params.hash_log;
    let mut hash_short = vec![0u32; hash_size];
    let mut hash_long = vec![0u32; hash_size];
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
    match params.hash_log {
        14 => compress_dfast_block_impl::<14>(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_short,
            hash_long,
            sequences,
        ),
        16 => compress_dfast_block_impl::<16>(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_short,
            hash_long,
            sequences,
        ),
        18 => compress_dfast_block_impl::<18>(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_short,
            hash_long,
            sequences,
        ),
        _ => compress_dfast_block_impl::<0>(
            src,
            block_start,
            block_end,
            params,
            rep_offsets,
            hash_short,
            hash_long,
            sequences,
        ),
    }
}

fn compress_dfast_block_impl<const HASH_LOG: u32>(
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
    let mut anchor = block_start;
    let mut ip = block_start;
    let acceleration = params.target_length.max(1) as usize;
    let search_strength = params.search_strength as usize;
    let mut rep = *rep_offsets;
    let block_size = block_end - block_start;

    if block_size < 16 {
        return;
    }

    let limit = block_end - 8;
    let max_distance = 1usize << params.window_log;
    let hash_log = if HASH_LOG != 0 {
        HASH_LOG
    } else {
        params.hash_log
    };

    while ip < limit {
        let rep0 = rep[0] as usize;
        if (rep0 > 0) & (ip >= rep0) && rd32(src, ip) == rd32(src, ip - rep0) {
            let ml = count_match(src, ip + 4, ip - rep0 + 4, block_end) + 4;
            let mut back = 0usize;
            while ip - back > anchor
                && ip - back > rep0
                && src[ip - back - 1] == src[ip - back - rep0 - 1]
            {
                back += 1;
            }
            ip -= back;
            let lit_len = (ip - anchor) as u32;
            sequences.push(Sequence {
                literal_length: lit_len,
                offset: rep0 as u32,
                match_length: (ml + back) as u32,
            });
            ip += ml + back;
            anchor = ip;
            update_hashes(src, ip, hash_log, hash_short, hash_long);
            rep_match_loop(
                src,
                &mut ip,
                &mut anchor,
                &mut rep,
                hash_log,
                hash_short,
                hash_long,
                sequences,
                limit,
                block_end,
            );
            continue;
        }

        let search_start = ip;

        loop {
            let val4 = rd32(src, ip);
            let val8 = rd64(src, ip);

            let hs = h4(val4, hash_log) as usize;
            let hl = h8(val8, hash_log) as usize;

            let match_short_pos = hl32(hash_short, hs) as usize;
            let match_long_pos = hl32(hash_long, hl) as usize;

            hs32(hash_short, hs, ip as u32);
            hs32(hash_long, hl, ip as u32);

            if match_long_pos < ip
                && ip - match_long_pos <= max_distance
                && val8 == rd64(src, match_long_pos)
            {
                let match_len = count_match(src, ip + 8, match_long_pos + 8, block_end) + 8;
                let mut back = 0usize;
                while ip - back > anchor
                    && match_long_pos > back
                    && src[ip - back - 1] == src[match_long_pos - back - 1]
                {
                    back += 1;
                }
                let match_start = ip - back;
                emit_match(
                    match_start,
                    match_long_pos - back,
                    match_len + back,
                    anchor,
                    sequences,
                    &mut rep,
                );
                ip += match_len;
                anchor = ip;
                insert_complementary(src, match_start, ip, hash_log, hash_short, hash_long);
                update_hashes(src, ip, hash_log, hash_short, hash_long);
                rep_match_loop(
                    src,
                    &mut ip,
                    &mut anchor,
                    &mut rep,
                    hash_log,
                    hash_short,
                    hash_long,
                    sequences,
                    limit,
                    block_end,
                );
                break;
            }

            if match_short_pos < ip
                && ip - match_short_pos <= max_distance
                && val4 == rd32(src, match_short_pos)
            {
                if ip + 1 < limit {
                    let val8_next = rd64(src, ip + 1);
                    let hl_next = h8(val8_next, hash_log) as usize;
                    let match_long_next = hl32(hash_long, hl_next) as usize;

                    if match_long_next < ip + 1
                        && ip + 1 - match_long_next <= max_distance
                        && val8_next == rd64(src, match_long_next)
                    {
                        let long_len = count_match(src, ip + 9, match_long_next + 8, block_end) + 8;
                        let short_len =
                            count_match(src, ip + 4, match_short_pos + 4, block_end) + 4;

                        if long_len > short_len + 1 {
                            ip += 1;
                            hs32(hash_long, hl_next, ip as u32);
                            let mut back = 0usize;
                            while ip - back > anchor
                                && match_long_next > back
                                && src[ip - back - 1] == src[match_long_next - back - 1]
                            {
                                back += 1;
                            }
                            let match_start = ip - back;
                            emit_match(
                                match_start,
                                match_long_next - back,
                                long_len + back,
                                anchor,
                                sequences,
                                &mut rep,
                            );
                            ip += long_len;
                            anchor = ip;
                            insert_complementary(
                                src,
                                match_start,
                                ip,
                                hash_log,
                                hash_short,
                                hash_long,
                            );
                            update_hashes(src, ip, hash_log, hash_short, hash_long);
                            rep_match_loop(
                                src,
                                &mut ip,
                                &mut anchor,
                                &mut rep,
                                hash_log,
                                hash_short,
                                hash_long,
                                sequences,
                                limit,
                                block_end,
                            );
                            break;
                        }
                    }
                }

                let mut match_len = count_match(src, ip + 4, match_short_pos + 4, block_end) + 4;
                let mut back = 0usize;
                while ip - back > anchor
                    && match_short_pos > back
                    && src[ip - back - 1] == src[match_short_pos - back - 1]
                {
                    back += 1;
                }
                let match_start = ip - back;
                match_len += back;
                emit_match(
                    match_start,
                    match_short_pos - back,
                    match_len,
                    anchor,
                    sequences,
                    &mut rep,
                );
                ip += match_len - back;
                anchor = ip;
                insert_complementary(src, match_start, ip, hash_log, hash_short, hash_long);
                update_hashes(src, ip, hash_log, hash_short, hash_long);
                rep_match_loop(
                    src,
                    &mut ip,
                    &mut anchor,
                    &mut rep,
                    hash_log,
                    hash_short,
                    hash_long,
                    sequences,
                    limit,
                    block_end,
                );
                break;
            }

            let step = acceleration + ((ip - search_start) >> search_strength);
            ip += step;

            if ip >= limit {
                break;
            }
        }

        if ip >= limit {
            break;
        }
    }
}

pub(crate) fn prefill_hash_tables(
    combined: &[u8],
    prefix_len: usize,
    hash_log: u32,
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
        let hs = h4(rd32(combined, i), hash_log) as usize;
        hs32(hash_short, hs, i as u32);
        let hl = h8(rd64(combined, i), hash_log) as usize;
        hs32(hash_long, hl, i as u32);
        i += step;
    }
    let tail_start = prefix_len.saturating_sub(64);
    for i in tail_start..prefix_len.saturating_sub(7) {
        let hs = h4(rd32(combined, i), hash_log) as usize;
        hs32(hash_short, hs, i as u32);
        let hl = h8(rd64(combined, i), hash_log) as usize;
        hs32(hash_long, hl, i as u32);
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
    let hash_size = 1usize << params.hash_log;
    let mut hash_short = vec![0u32; hash_size];
    let mut hash_long = vec![0u32; hash_size];
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
    prefill_hash_tables(combined, plen, params.hash_log, hash_short, hash_long);

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
fn rep_match_loop(
    src: &[u8],
    ip: &mut usize,
    anchor: &mut usize,
    rep: &mut [u32; 3],
    hash_log: u32,
    hash_short: &mut [u32],
    hash_long: &mut [u32],
    sequences: &mut Vec<Sequence>,
    limit: usize,
    block_end: usize,
) {
    loop {
        if *ip >= limit {
            break;
        }
        let r0 = rep[0] as usize;
        if (r0 > 0) & (*ip >= r0) && rd32(src, *ip) == rd32(src, *ip - r0) {
            let ml = count_match(src, *ip + 4, *ip - r0 + 4, block_end) + 4;
            sequences.push(Sequence {
                literal_length: 0,
                offset: r0 as u32,
                match_length: ml as u32,
            });
            *ip += ml;
            *anchor = *ip;
            update_hashes(src, *ip, hash_log, hash_short, hash_long);
            continue;
        }
        let r1 = rep[1] as usize;
        if (r1 > 0) & (*ip >= r1) && rd32(src, *ip) == rd32(src, *ip - r1) {
            rep.swap(0, 1);
            let ml = count_match(src, *ip + 4, *ip - r1 + 4, block_end) + 4;
            sequences.push(Sequence {
                literal_length: 0,
                offset: r1 as u32,
                match_length: ml as u32,
            });
            *ip += ml;
            *anchor = *ip;
            update_hashes(src, *ip, hash_log, hash_short, hash_long);
            continue;
        }
        break;
    }
}

#[inline(always)]
fn emit_match(
    ip: usize,
    match_pos: usize,
    match_len: usize,
    anchor: usize,
    sequences: &mut Vec<Sequence>,
    rep: &mut [u32; 3],
) {
    let offset = (ip - match_pos) as u32;
    let lit_len = (ip - anchor) as u32;

    sequences.push(Sequence {
        literal_length: lit_len,
        offset,
        match_length: match_len as u32,
    });

    rep[2] = rep[1];
    rep[1] = rep[0];
    rep[0] = offset;
}

#[inline(always)]
fn insert_complementary(
    src: &[u8],
    match_start: usize,
    match_end: usize,
    hash_log: u32,
    hash_short: &mut [u32],
    hash_long: &mut [u32],
) {
    let pos1 = match_start + 2;
    if pos1 + 8 <= src.len() && pos1 < match_end {
        let hs = h4(rd32(src, pos1), hash_log) as usize;
        hs32(hash_short, hs, pos1 as u32);
        let hl = h8(rd64(src, pos1), hash_log) as usize;
        hs32(hash_long, hl, pos1 as u32);
    }
    if match_end >= 2 {
        let pos2 = match_end - 2;
        if pos2 + 8 <= src.len() && pos2 > match_start + 2 {
            let hs = h4(rd32(src, pos2), hash_log) as usize;
            hs32(hash_short, hs, pos2 as u32);
            let hl = h8(rd64(src, pos2), hash_log) as usize;
            hs32(hash_long, hl, pos2 as u32);
        }
    }
}

#[inline(always)]
fn update_hashes(
    src: &[u8],
    ip: usize,
    hash_log: u32,
    hash_short: &mut [u32],
    hash_long: &mut [u32],
) {
    if ip + 8 <= src.len() {
        let hs = h4(rd32(src, ip), hash_log) as usize;
        hs32(hash_short, hs, ip as u32);
        let hl = h8(rd64(src, ip), hash_log) as usize;
        hs32(hash_long, hl, ip as u32);
    }
}

#[inline(always)]
fn h4(val: u32, hash_log: u32) -> usize {
    (val.wrapping_mul(PRIME32_1) >> (32 - hash_log)) as usize
}

#[inline(always)]
fn h8(val: u64, hash_log: u32) -> usize {
    (val.wrapping_mul(PRIME64_1) >> (64 - hash_log)) as usize
}

#[inline(always)]
fn rd32(src: &[u8], pos: usize) -> u32 {
    debug_assert!(pos + 4 <= src.len());
    unsafe { (src.as_ptr().add(pos) as *const u32).read_unaligned() }
}

#[inline(always)]
fn rd64(src: &[u8], pos: usize) -> u64 {
    debug_assert!(pos + 8 <= src.len());
    unsafe { (src.as_ptr().add(pos) as *const u64).read_unaligned() }
}

#[inline(always)]
fn hl32(table: &[u32], idx: usize) -> u32 {
    debug_assert!(idx < table.len());
    unsafe { *table.get_unchecked(idx) }
}

#[inline(always)]
fn hs32(table: &mut [u32], idx: usize, val: u32) {
    debug_assert!(idx < table.len());
    unsafe {
        *table.get_unchecked_mut(idx) = val;
    }
}

#[inline(always)]
fn count_match(src: &[u8], mut p1: usize, mut p2: usize, limit: usize) -> usize {
    debug_assert!(p1 <= limit && limit <= src.len());
    debug_assert!(p2 <= src.len());
    let start = p1;
    let max_len = (limit - p1).min(src.len() - p2);
    let end8 = start + (max_len & !7);
    while p1 < end8 {
        let a = rd64(src, p1);
        let b = rd64(src, p2);
        let xor = a ^ b;
        if xor != 0 {
            return p1 - start + (xor.trailing_zeros() as usize / 8);
        }
        p1 += 8;
        p2 += 8;
    }
    while p1 < start + max_len {
        if unsafe { *src.get_unchecked(p1) != *src.get_unchecked(p2) } {
            break;
        }
        p1 += 1;
        p2 += 1;
    }
    p1 - start
}
