#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::primitives;
use crate::strategy::LevelParams;
use zrip_core::Sequence;
use zrip_core::hash::{PRIME32_1, PRIME64_1};
use zrip_core::hint::unlikely;

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
    sequences.clear();
    let mls = params.min_match as usize;
    macro_rules! dispatch {
        ($hl:expr, $mls:expr) => {
            compress_fast_block_impl::<$hl, $mls>(
                src,
                block_start,
                block_end,
                params,
                rep_offsets,
                hash_table,
                sequences,
            )
        };
    }
    match mls {
        7.. => dispatch!(0, 7),
        5..7 => dispatch!(0, 5),
        _ => dispatch!(0, 4),
    }
}

/// C zstd-style 4-cursor match finder (port of ZSTD_compressBlock_fast_noDict_generic).
///
/// Pipeline: ip0, ip1=ip0+1, ip2=ip0+step, ip3=ip2+1.
/// Each iteration probes two positions (ip0 and shifted ip0), reusing hash
/// computations across shifts.  Rep offset checked at the step-ahead position
/// (ip2) only; rep2 checked in the post-match loop.
fn compress_fast_block_impl<const HASH_LOG: u32, const MLS: usize>(
    src: &[u8],
    block_start: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
) {
    // match_at confirms 5 bytes for MLS>=5, 4 bytes for MLS<5.
    // MLS only controls the hash function width; match confirmation is capped at 5.
    let confirm: usize = if MLS >= 5 { 5 } else { MLS };

    let block_size = block_end - block_start;
    if block_size < 8 {
        return;
    }

    let acceleration = params.target_length.max(1) as usize;
    let step_size = acceleration + 1;
    let search_strength = params.search_strength as usize;
    let ilimit = (block_end - MLS).min(src.len() - 8);
    let max_distance = 1usize << params.window_log;

    let probe_interval = (block_size / 4).max(4096).min(block_size);
    let mut probe_limit = block_start + probe_interval;
    let mut total_match_bytes: usize = 0;

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

    'start: loop {
        let mut ip1 = ip0 + 1;
        let mut ip2 = ip0 + step_size;

        if unlikely(ip2 + 1 >= ilimit) {
            break;
        }

        let mut h0 = hash_pos::<HASH_LOG, MLS>(src, ip0, hash_log);
        let mut match_idx = primitives::hash_load(hash_table, h0) as usize;

        loop {
            // Write hash for ip0 (before any checks, matching C zstd order)
            primitives::hash_store(hash_table, h0, ip0 as u32);

            // Rep check at step-ahead position ip2
            if ip2 >= rep1 {
                let ip2_val = primitives::rd32(src, ip2);
                let rep_val = primitives::rd32(src, ip2 - rep1);
                if ip2_val == rep_val {
                    let fill_pos = ip0;
                    ip0 = ip2;
                    // 1-byte backward extension (C zstd style)
                    if ip0 > anchor
                        && primitives::src_byte(src, ip0 - 1)
                            == primitives::src_byte(src, ip0 - rep1 - 1)
                    {
                        ip0 -= 1;
                    }
                    let h1 = hash_pos::<HASH_LOG, MLS>(src, ip1, hash_log);
                    primitives::hash_store(hash_table, h1, ip1 as u32);
                    let back = ip2 - ip0;
                    let mlen =
                        primitives::count_match(src, ip2 + 4, ip2 - rep1 + 4, block_end) + 4 + back;
                    total_match_bytes += mlen;
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
            }

            // First match check at ip0
            if match_idx < ip0
                && ip0 - match_idx <= max_distance
                && primitives::match_at::<MLS>(src, ip0, match_idx)
            {
                let h1 = hash_pos::<HASH_LOG, MLS>(src, ip1, hash_log);
                primitives::hash_store(hash_table, h1, ip1 as u32);
                let fill_pos = ip0;
                let mut back = 0usize;
                while ip0 > anchor + back
                    && match_idx > back + block_start
                    && primitives::src_byte(src, ip0 - back - 1)
                        == primitives::src_byte(src, match_idx - back - 1)
                {
                    back += 1;
                }
                let match_start = ip0 - back;
                let offset = (match_start - (match_idx - back)) as u32;
                let mlen =
                    primitives::count_match(src, ip0 + confirm, match_idx + confirm, block_end)
                        + confirm
                        + back;
                total_match_bytes += mlen;
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

            // First shift: compute h1 for ip1, reuse as h0
            let h1 = hash_pos::<HASH_LOG, MLS>(src, ip1, hash_log);
            match_idx = primitives::hash_load(hash_table, h1) as usize;
            h0 = h1;
            let h_ip2 = hash_pos::<HASH_LOG, MLS>(src, ip2, hash_log);
            ip0 = ip1;
            ip1 = ip2;
            ip2 += 1;

            #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
            primitives::prefetch_ht(hash_table, h_ip2);

            // Write hash for shifted ip0
            primitives::hash_store(hash_table, h0, ip0 as u32);

            // Second match check at shifted ip0
            if match_idx < ip0
                && ip0 - match_idx <= max_distance
                && primitives::match_at::<MLS>(src, ip0, match_idx)
            {
                if step_size + ((ip0 - anchor) >> search_strength) <= 4 {
                    primitives::hash_store(hash_table, h_ip2, ip1 as u32);
                }
                let fill_pos = ip0;
                let mut back = 0usize;
                while ip0 > anchor + back
                    && match_idx > back + block_start
                    && primitives::src_byte(src, ip0 - back - 1)
                        == primitives::src_byte(src, match_idx - back - 1)
                {
                    back += 1;
                }
                let match_start = ip0 - back;
                let offset = (match_start - (match_idx - back)) as u32;
                let mlen =
                    primitives::count_match(src, ip0 + confirm, match_idx + confirm, block_end)
                        + confirm
                        + back;
                total_match_bytes += mlen;
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
            match_idx = primitives::hash_load(hash_table, h_ip2) as usize;
            h0 = h_ip2;
            #[allow(unused_variables)]
            let h_next = hash_pos::<HASH_LOG, MLS>(src, ip2, hash_log);
            ip0 = ip1;
            ip1 = ip2;
            let step = step_size + ((ip0 - anchor) >> search_strength);
            ip2 = ip0 + step;

            #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
            primitives::prefetch_ht(hash_table, h_next);

            if ip0 >= probe_limit {
                let scanned = ip0 - block_start;
                if total_match_bytes * 6 < scanned {
                    sequences.clear();
                    return;
                }
                probe_limit = probe_limit.saturating_add(probe_interval).min(block_end);
            }

            if unlikely(ip2 + 1 >= ilimit) {
                break;
            }
        }

        break;
    }
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
        let val = primitives::rd32(src, *ip);
        let rval = primitives::rd32(src, *ip - *rep2);
        if val != rval {
            break;
        }
        let rlen = primitives::count_match(src, *ip + 4, *ip - *rep2 + 4, block_end) + 4;
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

pub(crate) fn prefill_hash_table(
    combined: &[u8],
    prefix_len: usize,
    hash_log: u32,
    hash_table: &mut [u32],
) {
    hash_table.fill(0);
    if prefix_len < 4 {
        return;
    }
    let hash_size = hash_table.len();
    let step = (prefix_len / hash_size).max(1);
    let mut i = 0;
    while i + 4 <= prefix_len {
        let h = hash4_const::<0>(primitives::rd32(combined, i), hash_log);
        primitives::hash_store(hash_table, h, i as u32);
        i += step;
    }
    let tail_start = prefix_len.saturating_sub(64);
    for i in tail_start..prefix_len.saturating_sub(3) {
        let h = hash4_const::<0>(primitives::rd32(combined, i), hash_log);
        primitives::hash_store(hash_table, h, i as u32);
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

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "std")]
pub(crate) fn compress_fast_attached(
    combined: &[u8],
    prefix_len: usize,
    block_end: usize,
    params: &LevelParams,
    rep_offsets: &[u32; 3],
    dict_hash: &[u32],
    dict_hash_log: u32,
    hash_table: &mut [u32],
    input_hash_log: u32,
    sequences: &mut Vec<Sequence>,
) {
    sequences.clear();
    let input_len = block_end - prefix_len;
    if input_len < 8 {
        return;
    }

    let mut anchor = prefix_len;
    let mut ip = prefix_len;
    let acceleration = params.target_length.max(1) as usize;
    let step_size = acceleration + 1;
    let search_strength = params.search_strength as usize;
    let mut rep = *rep_offsets;
    let window = 1usize << params.window_log;
    let limit = block_end - 4;

    while ip < limit {
        let rep0 = rep[0] as usize;
        if (rep0 > 0) & (ip >= rep0)
            && primitives::rd32(combined, ip) == primitives::rd32(combined, ip - rep0)
        {
            let clen = combined.len();
            let mut match_len = primitives::count_match(combined, ip + 4, ip - rep0 + 4, clen) + 4;
            let mut back = 0usize;
            while ip - back > anchor
                && ip - back > rep0
                && combined[ip - back - 1] == combined[ip - back - rep0 - 1]
            {
                back += 1;
            }
            ip -= back;
            match_len += back;
            let lit_len = (ip - anchor) as u32;
            sequences.push(Sequence {
                literal_length: lit_len,
                offset: rep0 as u32,
                match_length: match_len as u32,
            });
            ip += match_len;
            anchor = ip;
            insert_hash::<0>(combined, ip, input_hash_log, hash_table);
            rep_match_loop_fast::<0>(
                combined,
                &mut ip,
                &mut anchor,
                &mut rep,
                input_hash_log,
                hash_table,
                sequences,
                limit,
                clen,
            );
            continue;
        }

        let search_start = ip;

        loop {
            let val = primitives::rd32(combined, ip);

            let h_input = hash4_const::<0>(val, input_hash_log);
            let input_match = primitives::hash_load(hash_table, h_input) as usize;
            primitives::hash_store(hash_table, h_input, ip as u32);

            if input_match >= prefix_len
                && input_match < ip
                && ip - input_match <= window
                && val == primitives::rd32(combined, input_match)
            {
                let clen = combined.len();
                let mut match_len =
                    primitives::count_match(combined, ip + 4, input_match + 4, clen) + 4;
                let match_pos = input_match;
                let mut back = 0usize;
                while ip - back > anchor
                    && match_pos > back
                    && combined[ip - back - 1] == combined[match_pos - back - 1]
                {
                    back += 1;
                }
                let match_start = ip - back;
                match_len += back;
                let offset = (match_start - (match_pos - back)) as u32;
                let lit_len = (match_start - anchor) as u32;

                sequences.push(Sequence {
                    literal_length: lit_len,
                    offset,
                    match_length: match_len as u32,
                });

                rep[2] = rep[1];
                rep[1] = rep[0];
                rep[0] = offset;

                ip += match_len - back;
                anchor = ip;

                insert_complementary_fast::<0>(
                    combined,
                    match_start,
                    ip,
                    input_hash_log,
                    hash_table,
                );
                insert_hash::<0>(combined, ip, input_hash_log, hash_table);
                rep_match_loop_fast::<0>(
                    combined,
                    &mut ip,
                    &mut anchor,
                    &mut rep,
                    input_hash_log,
                    hash_table,
                    sequences,
                    limit,
                    clen,
                );

                break;
            }

            let h_dict = hash4_const::<0>(val, dict_hash_log);
            let dict_match = primitives::hash_load(dict_hash, h_dict) as usize;
            if dict_match < prefix_len
                && ip - dict_match <= window
                && val == primitives::rd32(combined, dict_match)
            {
                let clen = combined.len();
                let match_pos = dict_match;
                let mut match_len =
                    primitives::count_match(combined, ip + 4, match_pos + 4, clen) + 4;
                let mut back = 0usize;
                while ip - back > anchor
                    && match_pos > back
                    && combined[ip - back - 1] == combined[match_pos - back - 1]
                {
                    back += 1;
                }
                let match_start = ip - back;
                match_len += back;
                let offset = (match_start - (match_pos - back)) as u32;
                let lit_len = (match_start - anchor) as u32;

                sequences.push(Sequence {
                    literal_length: lit_len,
                    offset,
                    match_length: match_len as u32,
                });

                rep[2] = rep[1];
                rep[1] = rep[0];
                rep[0] = offset;

                ip += match_len - back;
                anchor = ip;

                insert_complementary_fast::<0>(
                    combined,
                    match_start,
                    ip,
                    input_hash_log,
                    hash_table,
                );
                insert_hash::<0>(combined, ip, input_hash_log, hash_table);
                rep_match_loop_fast::<0>(
                    combined,
                    &mut ip,
                    &mut anchor,
                    &mut rep,
                    input_hash_log,
                    hash_table,
                    sequences,
                    limit,
                    clen,
                );

                break;
            }

            let step = step_size + ((ip - search_start) >> search_strength);
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

    prefill_hash_table(combined, prefix.len(), params.hash_log, dict_hash);
    hash_table.copy_from_slice(dict_hash);
    let use_dict_fallback = src.len() <= 1024;

    sequences.clear();
    let plen = prefix.len();
    let mut anchor = plen;
    let mut ip = plen;
    let acceleration = params.target_length.max(1) as usize;
    let step_size = acceleration + 1;
    let search_strength = params.search_strength as usize;
    let mut rep = *rep_offsets;
    let window = 1usize << params.window_log;

    if src.len() < 8 {
        return;
    }

    let limit = combined.len() - 4;

    while ip < limit {
        let rep0 = rep[0] as usize;
        if (rep0 > 0) & (ip >= rep0)
            && primitives::rd32(combined, ip) == primitives::rd32(combined, ip - rep0)
        {
            let clen = combined.len();
            let mut match_len = primitives::count_match(combined, ip + 4, ip - rep0 + 4, clen) + 4;
            let mut back = 0usize;
            while ip - back > anchor
                && ip - back > rep0
                && combined[ip - back - 1] == combined[ip - back - rep0 - 1]
            {
                back += 1;
            }
            ip -= back;
            match_len += back;
            let lit_len = (ip - anchor) as u32;
            sequences.push(Sequence {
                literal_length: lit_len,
                offset: rep0 as u32,
                match_length: match_len as u32,
            });
            ip += match_len;
            anchor = ip;
            insert_hash::<0>(combined, ip, params.hash_log, hash_table);
            rep_match_loop_fast::<0>(
                combined,
                &mut ip,
                &mut anchor,
                &mut rep,
                params.hash_log,
                hash_table,
                sequences,
                limit,
                clen,
            );
            continue;
        }

        let search_start = ip;

        loop {
            let val = primitives::rd32(combined, ip);
            let h = hash4_const::<0>(val, params.hash_log);

            // Probe input hash table (read + write)
            let input_match = primitives::hash_load(hash_table, h) as usize;
            primitives::hash_store(hash_table, h, ip as u32);

            // Primary table lookup (prefix + input entries, evolves during matching)
            if input_match < ip
                && ip - input_match <= window
                && val == primitives::rd32(combined, input_match)
            {
                let clen = combined.len();
                let mut match_len =
                    primitives::count_match(combined, ip + 4, input_match + 4, clen) + 4;
                let match_pos = input_match;
                let mut back = 0usize;
                while ip - back > anchor
                    && match_pos > back
                    && combined[ip - back - 1] == combined[match_pos - back - 1]
                {
                    back += 1;
                }
                let match_start = ip - back;
                match_len += back;
                let offset = (match_start - (match_pos - back)) as u32;
                let lit_len = (match_start - anchor) as u32;

                sequences.push(Sequence {
                    literal_length: lit_len,
                    offset,
                    match_length: match_len as u32,
                });

                rep[2] = rep[1];
                rep[1] = rep[0];
                rep[0] = offset;

                ip += match_len - back;
                anchor = ip;

                insert_complementary_fast::<0>(
                    combined,
                    match_start,
                    ip,
                    params.hash_log,
                    hash_table,
                );
                insert_hash::<0>(combined, ip, params.hash_log, hash_table);
                rep_match_loop_fast::<0>(
                    combined,
                    &mut ip,
                    &mut anchor,
                    &mut rep,
                    params.hash_log,
                    hash_table,
                    sequences,
                    limit,
                    clen,
                );

                break;
            }

            // Fallback: check frozen dict table for prefix matches lost
            // to hash collisions. Only for small inputs where every byte
            // matters; on large inputs the extra sequences hurt encoding.
            if use_dict_fallback {
                let dict_match = primitives::hash_load(dict_hash, h) as usize;
                if dict_match < plen && val == primitives::rd32(combined, dict_match) {
                    let clen = combined.len();
                    let dlen = primitives::count_match(combined, ip + 4, dict_match + 4, clen) + 4;
                    if dlen >= 6 {
                        let match_pos = dict_match;
                        let mut match_len = dlen;
                        let mut back = 0usize;
                        while ip - back > anchor
                            && match_pos > back
                            && combined[ip - back - 1] == combined[match_pos - back - 1]
                        {
                            back += 1;
                        }
                        let match_start = ip - back;
                        match_len += back;
                        let offset = (match_start - (match_pos - back)) as u32;
                        let lit_len = (match_start - anchor) as u32;

                        sequences.push(Sequence {
                            literal_length: lit_len,
                            offset,
                            match_length: match_len as u32,
                        });

                        rep[2] = rep[1];
                        rep[1] = rep[0];
                        rep[0] = offset;

                        ip += match_len - back;
                        anchor = ip;

                        insert_complementary_fast::<0>(
                            combined,
                            match_start,
                            ip,
                            params.hash_log,
                            hash_table,
                        );
                        insert_hash::<0>(combined, ip, params.hash_log, hash_table);
                        rep_match_loop_fast::<0>(
                            combined,
                            &mut ip,
                            &mut anchor,
                            &mut rep,
                            params.hash_log,
                            hash_table,
                            sequences,
                            limit,
                            clen,
                        );

                        break;
                    }
                }
            }

            let step = step_size + ((ip - search_start) >> search_strength);
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

const PRIME7: u64 = 58_295_818_150_454_627;

#[inline(always)]
fn hash7_const<const HASH_LOG: u32>(value: u64, hash_log: u32) -> usize {
    let hl = if HASH_LOG != 0 { HASH_LOG } else { hash_log };
    (((value << 8).wrapping_mul(PRIME7)) >> (64 - hl)) as usize
}

#[inline(always)]
fn hash_pos<const HASH_LOG: u32, const MLS: usize>(src: &[u8], pos: usize, hash_log: u32) -> usize {
    if MLS >= 7 {
        hash7_const::<HASH_LOG>(primitives::rd64(src, pos), hash_log)
    } else if MLS >= 5 {
        hash5_const::<HASH_LOG>(primitives::rd64(src, pos), hash_log)
    } else {
        hash4_const::<HASH_LOG>(primitives::rd32(src, pos), hash_log)
    }
}

#[inline]
fn insert_hash<const HASH_LOG: u32>(src: &[u8], ip: usize, hash_log: u32, hash_table: &mut [u32]) {
    if ip + 4 <= src.len() {
        let h = hash4_const::<HASH_LOG>(primitives::rd32(src, ip), hash_log);
        primitives::hash_store(hash_table, h, ip as u32);
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
        primitives::hash_store(hash_table, h, ip as u32);
    }
}

fn insert_complementary_fast<const HASH_LOG: u32>(
    src: &[u8],
    match_start: usize,
    match_end: usize,
    hash_log: u32,
    hash_table: &mut [u32],
) {
    let pos1 = match_start + 2;
    if pos1 + 4 <= src.len() && pos1 < match_end {
        let h = hash4_const::<HASH_LOG>(primitives::rd32(src, pos1), hash_log);
        primitives::hash_store(hash_table, h, pos1 as u32);
    }
    if match_end >= 2 {
        let pos2 = match_end - 2;
        if pos2 + 4 <= src.len() && pos2 > match_start + 2 {
            let h = hash4_const::<HASH_LOG>(primitives::rd32(src, pos2), hash_log);
            primitives::hash_store(hash_table, h, pos2 as u32);
        }
    }
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn rep_match_loop_fast<const HASH_LOG: u32>(
    src: &[u8],
    ip: &mut usize,
    anchor: &mut usize,
    rep: &mut [u32; 3],
    hash_log: u32,
    hash_table: &mut [u32],
    sequences: &mut Vec<Sequence>,
    limit: usize,
    block_end: usize,
) {
    loop {
        if *ip >= limit {
            break;
        }
        let r0 = rep[0] as usize;
        if (r0 > 0) & (*ip >= r0) && primitives::rd32(src, *ip) == primitives::rd32(src, *ip - r0) {
            let ml = primitives::count_match(src, *ip + 4, *ip - r0 + 4, block_end) + 4;
            sequences.push(Sequence {
                literal_length: 0,
                offset: r0 as u32,
                match_length: ml as u32,
            });
            *ip += ml;
            *anchor = *ip;
            insert_hash::<HASH_LOG>(src, *ip, hash_log, hash_table);
            continue;
        }
        let r1 = rep[1] as usize;
        if (r1 > 0) & (*ip >= r1) && primitives::rd32(src, *ip) == primitives::rd32(src, *ip - r1) {
            rep.swap(0, 1);
            let ml = primitives::count_match(src, *ip + 4, *ip - r1 + 4, block_end) + 4;
            sequences.push(Sequence {
                literal_length: 0,
                offset: r1 as u32,
                match_length: ml as u32,
            });
            *ip += ml;
            *anchor = *ip;
            insert_hash::<HASH_LOG>(src, *ip, hash_log, hash_table);
            continue;
        }
        break;
    }
}
