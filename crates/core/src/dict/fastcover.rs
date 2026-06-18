#![forbid(unsafe_code)]

use alloc::vec;
use alloc::vec::Vec;

pub struct FastCoverParams {
    pub k: usize,
    pub d: usize,
    pub accel: usize,
}

impl Default for FastCoverParams {
    fn default() -> Self {
        Self {
            k: 2048,
            d: 8,
            accel: 1,
        }
    }
}

pub fn select_segments(samples: &[&[u8]], dict_size: usize, params: &FastCoverParams) -> Vec<u8> {
    let d = params.d;
    let k = params.k;
    let accel = params.accel.max(1);

    let mut concat = Vec::new();
    let mut offsets = Vec::new();
    for &sample in samples {
        offsets.push(concat.len());
        concat.extend_from_slice(sample);
    }
    offsets.push(concat.len());

    if concat.len() < d || concat.len() < k {
        return concat[..dict_size.min(concat.len())].to_vec();
    }

    let num_dmers = concat.len() - d + 1;
    let freq_table_bits = (d * 4).min(24);
    let freq_table_size = 1usize << freq_table_bits;
    let freq_mask = freq_table_size - 1;
    let mut freqs = vec![0u32; freq_table_size];

    // Precompute hash indices for all d-mer positions
    let mut hashes = vec![0u32; num_dmers];
    for i in 0..num_dmers {
        hashes[i] = (hash_dmer(&concat[i..i + d]) & freq_mask) as u32;
    }

    // Count d-mer frequencies across distinct samples
    for s in 0..samples.len() {
        let start = offsets[s];
        let end = offsets[s + 1];
        if end - start < d {
            continue;
        }
        let mut i = start;
        while i + d <= end {
            freqs[hashes[i] as usize] += 1;
            i += accel;
        }
    }

    let mut best_segments: Vec<(usize, u64)> = Vec::new();
    let content_budget = dict_size;
    let mut collected = 0usize;

    let mut used = vec![false; concat.len()];
    let seg_dmers = k - d + 1;

    // Prefix sum array: prefix[i] = sum of freqs[hashes[j]] for j in 0..i
    let mut prefix = vec![0u64; num_dmers + 1];

    while collected < content_budget {
        // Rebuild prefix sums (frequencies change each round)
        prefix[0] = 0;
        for i in 0..num_dmers {
            prefix[i + 1] = prefix[i] + freqs[hashes[i] as usize] as u64;
        }

        let mut best_pos = 0;
        let mut best_score = 0u64;

        let step = (num_dmers / 512).max(1);
        let mut pos = 0;
        while pos + k <= concat.len() {
            if !used[pos] {
                let score = prefix[pos + seg_dmers] - prefix[pos];
                if score > best_score {
                    best_score = score;
                    best_pos = pos;
                }
            }
            pos += step;
        }

        if best_score == 0 {
            break;
        }

        best_segments.push((best_pos, best_score));

        for i in best_pos..best_pos + seg_dmers {
            freqs[hashes[i] as usize] = 0;
        }
        for u in &mut used[best_pos..best_pos + k] {
            *u = true;
        }

        collected += k;
    }

    let mut content = Vec::with_capacity(content_budget);
    for &(pos, _) in best_segments.iter().rev() {
        let end = (pos + k).min(concat.len());
        content.extend_from_slice(&concat[pos..end]);
        if content.len() >= content_budget {
            break;
        }
    }
    content.truncate(content_budget);

    if content.len() < content_budget {
        let pad_start = concat.len().saturating_sub(content_budget - content.len());
        content.extend_from_slice(&concat[pad_start..]);
        content.truncate(content_budget);
    }

    content
}

#[inline(always)]
fn hash_dmer(dmer: &[u8]) -> usize {
    let len = dmer.len();
    if len <= 8 {
        let v = read_le_partial(dmer);
        v.wrapping_mul(0x9E37_79B9_7F4A_7C15) as usize
    } else {
        let mut h = read_le_u64(dmer).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut off = 8;
        while off + 8 <= len {
            h = h.rotate_left(11) ^ read_le_u64(&dmer[off..]).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            off += 8;
        }
        if off < len {
            h = h.rotate_left(11)
                ^ read_le_partial(&dmer[off..]).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
        h as usize
    }
}

#[inline(always)]
fn read_le_u64(data: &[u8]) -> u64 {
    u64::from_le_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ])
}

#[inline(always)]
fn read_le_partial(data: &[u8]) -> u64 {
    let mut v = 0u64;
    let mut i = 0;
    while i < data.len() {
        v |= (data[i] as u64) << (i * 8);
        i += 1;
    }
    v
}
