//! Dictionary content selection after C zstd's FastCOVER. Simpler: no
//! parameter search, and candidate segments are sampled.

#![forbid(unsafe_code)]

use alloc::vec;
use alloc::vec::Vec;

use crate::dict::MIN_DICT_SIZE;

/// FastCOVER parameters.
pub struct FastCoverParams {
    /// Segment size in bytes.
    pub k: usize,
    /// d-mer size in bytes. The frequency table has `2^min(4 * d, 24)`
    /// counters.
    pub d: usize,
    /// Step between counted d-mer positions; 0 acts as 1.
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

/// Picks up to `dict_size` bytes of content from `samples`: the `k`-byte
/// segments whose d-mers are most frequent, best last. Pads with the end
/// of the samples when segments run out. Samples shorter than `k` in
/// total come back as is, cut to `dict_size`. Sizes under
/// [`MIN_DICT_SIZE`] count as `MIN_DICT_SIZE`, as in
/// [`finalize_dictionary`](super::finalize::finalize_dictionary), which
/// keeps the end of the content that fits beside its tables.
pub fn select_segments(samples: &[&[u8]], dict_size: usize, params: &FastCoverParams) -> Vec<u8> {
    let dict_size = dict_size.max(MIN_DICT_SIZE);
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
    // 4 bits per d-mer byte, at most 2^24 counters (64 MiB).
    let freq_table_bits = (d * 4).min(24);
    let freq_table_size = 1usize << freq_table_bits;
    let freq_mask = freq_table_size - 1;
    let mut freqs = vec![0u32; freq_table_size];

    // Precompute hash indices for all d-mer positions
    let mut hashes = vec![0u32; num_dmers];
    for i in 0..num_dmers {
        hashes[i] = (hash_dmer(&concat[i..i + d]) & freq_mask) as u32;
    }

    // Count d-mer frequencies within each sample; d-mers that span two
    // samples are skipped
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

        // Score about 512 evenly spaced start positions per round.
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

        // Zero the chosen d-mers so later picks favor other content.
        for i in best_pos..best_pos + seg_dmers {
            freqs[hashes[i] as usize] = 0;
        }
        for u in &mut used[best_pos..best_pos + k] {
            *u = true;
        }

        collected += k;
    }

    // As C zstd: fill from the end, best pick first, so the best segment
    // ends nearest the data, and cut the last pick to the room left.
    let mut content = vec![0u8; content_budget];
    let mut tail = content_budget;
    for &(pos, _) in &best_segments {
        if tail == 0 {
            break;
        }
        let end = (pos + k).min(concat.len());
        let size = (end - pos).min(tail);
        tail -= size;
        content[tail..tail + size].copy_from_slice(&concat[pos..pos + size]);
    }
    if tail > 0 {
        // Too few segments: the samples' last bytes fill the front.
        let pad = tail.min(concat.len());
        content.drain(..tail - pad);
        let pad_from = concat.len() - pad;
        content[..pad].copy_from_slice(&concat[pad_from..]);
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
