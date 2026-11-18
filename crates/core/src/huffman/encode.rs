#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use super::primitives;
use crate::huffman::{MAX_BITS, MAX_SYMBOL_VALUE};

#[derive(Clone)]
pub struct HuffmanEncodeTable {
    num_bits: [u8; MAX_SYMBOL_VALUE + 1],
    /// `code | num_bits << 16` per symbol, so encoding needs one load.
    packed: [u32; MAX_SYMBOL_VALUE + 1],
    /// Serialized weight description, built once with the table.
    description: Vec<u8>,
    table_log: u8,
}

#[cfg(feature = "alloc")]
impl HuffmanEncodeTable {
    pub fn from_data(data: &[u8]) -> Option<Self> {
        if data.is_empty() {
            return None;
        }
        Self::from_histogram(&byte_histogram(data))
    }

    /// Builds a table from byte counts, as returned by [`byte_histogram`].
    pub fn from_histogram(freqs: &[u32; MAX_SYMBOL_VALUE + 1]) -> Option<Self> {
        let max_sym = freqs.iter().rposition(|&f| f > 0)?;

        // The symbols that occur, in symbol order. Every later step works on
        // this list instead of all 256 counts.
        let mut syms = [0u8; MAX_SYMBOL_VALUE + 1];
        let mut n = 0usize;
        for (s, &f) in freqs[..=max_sym].iter().enumerate() {
            syms[n & MAX_SYMBOL_VALUE] = s as u8;
            n += usize::from(f > 0);
        }
        let syms = &syms[..n];
        if n < 2 {
            return None;
        }

        // Beyond 128 explicit weights only the FSE-compressed form exists, and
        // it must fit in 127 bytes. If it does not, flatten the counts: fewer
        // distinct weights compress better.
        let mut counts = *freqs;
        loop {
            let (num_bits, table_log) = code_lengths(&counts, syms);
            let mut weights = [0u8; MAX_SYMBOL_VALUE + 1];
            for &s in syms {
                weights[s as usize] = table_log + 1 - num_bits[s as usize];
            }
            // The explicit weights cover every symbol below the last one.
            let mut hist = [0u32; WEIGHT_HIST_LEN];
            for &s in &syms[..n - 1] {
                hist[usize::from(weights[s as usize]) % WEIGHT_HIST_LEN] += 1;
            }
            hist[0] = (max_sym + 1 - n) as u32;
            if let Some(description) = describe_weights(&weights[..max_sym], &hist) {
                let codes = build_encode_codes(&weights, syms, table_log);
                return Some(Self {
                    packed: pack_codes(&codes, &num_bits),
                    description,
                    num_bits,
                    table_log,
                });
            }
            if syms.iter().all(|&s| counts[s as usize] <= 1) {
                return None;
            }
            // (c + 1) / 2 keeps every symbol present and reaches 1 for all,
            // so the loop ends.
            for &s in syms {
                counts[s as usize] = counts[s as usize].div_ceil(2);
            }
        }
    }

    pub fn from_decode_table(
        decode_table: &[super::HuffmanDecodeEntry],
        table_log: u8,
    ) -> Option<Self> {
        let table_size = 1usize << table_log;
        if decode_table.len() < table_size {
            return None;
        }

        let mut num_bits_per_sym = [0u8; MAX_SYMBOL_VALUE + 1];
        let mut max_sym = 0u8;
        let mut seen = [false; MAX_SYMBOL_VALUE + 1];
        for entry in &decode_table[..table_size] {
            let s = entry.symbol;
            if !seen[s as usize] {
                num_bits_per_sym[s as usize] = entry.num_bits;
                seen[s as usize] = true;
                if s > max_sym {
                    max_sym = s;
                }
            }
        }

        if max_sym as usize > 128 {
            return None;
        }

        let mut syms = [0u8; MAX_SYMBOL_VALUE + 1];
        let mut n = 0usize;
        let mut weights = [0u8; MAX_SYMBOL_VALUE + 1];
        let mut num_bits = [0u8; MAX_SYMBOL_VALUE + 1];
        for s in 0..=max_sym {
            if seen[s as usize] {
                syms[n] = s;
                n += 1;
                num_bits[s as usize] = num_bits_per_sym[s as usize];
                weights[s as usize] = table_log + 1 - num_bits_per_sym[s as usize];
            }
        }
        if n < 2 {
            return None;
        }

        let codes = build_encode_codes(&weights, &syms[..n], table_log);
        let explicit = &weights[..max_sym as usize];
        let description =
            describe_weights(explicit, &weight_histogram(explicit)).unwrap_or_default();

        Some(Self {
            packed: pack_codes(&codes, &num_bits),
            description,
            num_bits,
            table_log,
        })
    }

    pub fn table_log(&self) -> u8 {
        self.table_log
    }

    /// Encoded size in bits of data with these byte counts, or `None` when
    /// the table has no code for a byte that occurs.
    pub fn estimate_bits(&self, freqs: &[u32; MAX_SYMBOL_VALUE + 1]) -> Option<u64> {
        let mut bits = 0u64;
        for (&f, &n) in freqs.iter().zip(&self.num_bits) {
            if f > 0 && n == 0 {
                return None;
            }
            bits += u64::from(f) * u64::from(n);
        }
        Some(bits)
    }

    pub fn can_encode(&self, data: &[u8]) -> bool {
        for &b in data {
            if self.num_bits[b as usize] == 0 {
                return false;
            }
        }
        true
    }

    pub fn serialize_weights(&self) -> Vec<u8> {
        self.weights_description().to_vec()
    }

    /// Explicit weights (all but the implied last one), derived from the
    /// code lengths.
    #[cfg(test)]
    fn explicit_weights(&self) -> Vec<u8> {
        let last = self.num_bits.iter().rposition(|&n| n > 0).unwrap_or(0);
        self.num_bits[..last]
            .iter()
            .map(|&n| if n == 0 { 0 } else { self.table_log + 1 - n })
            .collect()
    }

    /// The serialized Huffman tree description (weights header).
    pub fn weights_description(&self) -> &[u8] {
        debug_assert_ne!(self.description.len(), 0);
        &self.description
    }

    pub fn encode_single_stream(&self, data: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(data.len() + 8);
        self.encode_single_stream_into(data, &mut buf);
        buf
    }

    pub fn encode_single_stream_into(&self, data: &[u8], buf: &mut Vec<u8>) {
        // Codes are at most MAX_BITS (11) long: four codes plus at most seven
        // pending bits fit in the 64-bit container, so every group of four
        // symbols ends in an unconditional flush.
        const _: () = assert!(4 * MAX_BITS as u32 + 7 <= 64);

        let mut bitstream = primitives::BitstreamScratch::new(buf, data.len() + 16);
        let packed = &self.packed;
        let mut bits: u64 = 0;
        let mut bits_used: u32 = 0;
        let mut wpos: usize = 0;

        macro_rules! put {
            ($b:expr) => {
                let e = packed[$b as usize];
                bits |= u64::from(e & 0xFFFF) << bits_used;
                bits_used += e >> 16;
            };
        }
        macro_rules! flush_bits {
            () => {
                bitstream.flush(wpos, bits);
                let nb = (bits_used >> 3) as usize;
                wpos += nb;
                bits >>= nb << 3;
                bits_used &= 7;
            };
        }

        // Symbols are written last to first. rchunks_exact walks groups from
        // the end; the leftover head is encoded after them, in reverse.
        let chunks = data.rchunks_exact(4);
        let head = chunks.remainder();
        for c in chunks {
            put!(c[3]);
            put!(c[2]);
            put!(c[1]);
            put!(c[0]);
            flush_bits!();
        }
        for &b in head.iter().rev() {
            put!(b);
        }
        flush_bits!();

        bits |= 1u64 << bits_used;
        bits_used += 1;
        while bits_used > 0 {
            bitstream.write_byte(wpos, bits as u8);
            wpos += 1;
            bits >>= 8;
            bits_used = bits_used.saturating_sub(8);
        }
        bitstream.finish(wpos);
    }

    pub fn encode_4_streams(&self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_4_streams_into(data, &mut out, &mut Vec::new());
        out
    }

    pub fn encode_4_streams_into(&self, data: &[u8], out: &mut Vec<u8>, stream_buf: &mut Vec<u8>) {
        let seg = data.len().div_ceil(4);
        let s1 = &data[..seg.min(data.len())];
        let s2 = &data[seg.min(data.len())..(seg * 2).min(data.len())];
        let s3 = &data[(seg * 2).min(data.len())..(seg * 3).min(data.len())];
        let s4 = &data[(seg * 3).min(data.len())..];

        out.clear();
        out.extend_from_slice(&[0u8; 6]);

        self.encode_single_stream_into(s1, stream_buf);
        let e1_len = stream_buf.len();
        out.extend_from_slice(stream_buf);

        self.encode_single_stream_into(s2, stream_buf);
        let e2_len = stream_buf.len();
        out.extend_from_slice(stream_buf);

        self.encode_single_stream_into(s3, stream_buf);
        let e3_len = stream_buf.len();
        out.extend_from_slice(stream_buf);

        self.encode_single_stream_into(s4, stream_buf);
        out.extend_from_slice(stream_buf);

        out[0..2].copy_from_slice(&(e1_len as u16).to_le_bytes());
        out[2..4].copy_from_slice(&(e2_len as u16).to_le_bytes());
        out[4..6].copy_from_slice(&(e3_len as u16).to_le_bytes());
    }

    pub fn compressed_size_single(&self, data: &[u8]) -> usize {
        let total_bits: usize = data
            .iter()
            .map(|&b| self.num_bits[b as usize] as usize)
            .sum();
        (total_bits + 8) / 8
    }
}

/// Code lengths of at most [`MAX_BITS`] for the symbols in `syms` (at
/// least two, in symbol order) with these counts, as a per-symbol array and
/// the longest length.
fn code_lengths(
    counts: &[u32; MAX_SYMBOL_VALUE + 1],
    syms: &[u8],
) -> ([u8; MAX_SYMBOL_VALUE + 1], u8) {
    let n = syms.len();
    debug_assert!((2..=MAX_SYMBOL_VALUE + 1).contains(&n));
    let mut freq = [0u32; MAX_SYMBOL_VALUE + 1];
    for (f, &s) in freq.iter_mut().zip(syms) {
        *f = counts[s as usize];
    }
    // Block literal counts stay far below 2^23, so the sum of up to 256 of
    // them fits a u32. Larger counts are scaled down to that range.
    let max = freq[..n].iter().copied().max().unwrap_or(0);
    let scale = (u32::BITS - max.leading_zeros()).saturating_sub(23);
    if scale > 0 {
        for f in &mut freq[..n] {
            *f = (*f >> scale).max(1);
        }
    }

    // An optimal tree can be deeper than MAX_BITS. Flatten the frequencies
    // and rebuild until it fits, as bzip2 does. Each pass halves the spread
    // between counts, so the depth converges to ceil(log2(active)) <= 8.
    loop {
        let (num_bits, max_bits) = huffman_bit_lengths(&freq[..n], syms);
        if max_bits <= MAX_BITS {
            return (num_bits, max_bits);
        }
        for f in &mut freq[..n] {
            *f = 1 + *f / 2;
        }
    }
}

/// Builds unrestricted Huffman code lengths for the symbols in `syms` (in
/// symbol order) with counts `freq`, as a per-symbol array and the longest
/// length.
fn huffman_bit_lengths(freq: &[u32], syms: &[u8]) -> ([u8; MAX_SYMBOL_VALUE + 1], u8) {
    // Two-queue construction: leaves sorted by (frequency, symbol), internal
    // nodes created in nondecreasing frequency order. Preferring the leaf on
    // equal frequency picks the same node as a min-heap keyed by
    // (frequency, id), because leaf ids precede internal ids.
    const MAX_NODES: usize = 2 * (MAX_SYMBOL_VALUE + 1);
    const NODE_MASK: usize = MAX_NODES - 1;
    let n = syms.len();
    debug_assert!((2..=MAX_SYMBOL_VALUE + 1).contains(&n));

    // Sort packed (frequency, symbol) keys; frequencies stay below 2^23.
    // Counting sort by frequency, with every count of 255 or more in the
    // last bucket: the buckets keep symbol order, so only that last one
    // needs a comparison sort, and only the few frequent symbols land there.
    const TOP: usize = 255;
    let mut starts = [0u16; TOP + 1];
    for &f in freq {
        starts[(f as usize).min(TOP)] += 1;
    }
    let mut at = 0u16;
    for start in &mut starts {
        let count = *start;
        *start = at;
        at += count;
    }
    let mut keys = [0u32; MAX_SYMBOL_VALUE + 1];
    for (&f, &s) in freq.iter().zip(syms) {
        let bucket = &mut starts[(f as usize).min(TOP)];
        keys[usize::from(*bucket) & MAX_SYMBOL_VALUE] = (f << 8) | u32::from(s);
        *bucket += 1;
    }
    let keys = &mut keys[..n];
    // Each start now holds its bucket's end, so this is where the last
    // bucket begins.
    let frequent = usize::from(starts[TOP - 1]);
    keys[frequent..].sort_unstable();

    // Both queues end in a sentinel above every sum (at most 2^31), so a
    // pick needs no emptiness check: the sorted leaves, and the internal
    // nodes in creation order, each set once its children are picked. Leaf
    // `i` has id `i`, internal node `j` id `n + j`.
    let mut leaves = [u32::MAX; MAX_NODES];
    for (l, &k) in leaves.iter_mut().zip(keys.iter()) {
        *l = k >> 8;
    }
    let mut sums = [u32::MAX; MAX_NODES];
    let mut parent = [0u16; MAX_NODES];
    let (mut leaf, mut internal) = (0usize, 0usize);
    for j in 0..n - 1 {
        let mut pick = || {
            let (l, i) = (leaves[leaf & NODE_MASK], sums[internal & NODE_MASK]);
            let take_leaf = l <= i;
            let id = if take_leaf { leaf } else { n + internal };
            leaf += usize::from(take_leaf);
            internal += usize::from(!take_leaf);
            (id, if take_leaf { l } else { i })
        };
        let (a, fa) = pick();
        let (b, fb) = pick();
        sums[j] = fa + fb;
        parent[a & NODE_MASK] = (n + j) as u16;
        parent[b & NODE_MASK] = (n + j) as u16;
    }

    // Parents always follow their children, so one backward pass from the
    // root assigns every depth.
    let root = 2 * n - 2;
    let mut depth = [0u8; MAX_NODES];
    for k in (0..root).rev() {
        depth[k] = depth[usize::from(parent[k]) & NODE_MASK] + 1;
    }

    let mut bit_lengths = [0u8; MAX_SYMBOL_VALUE + 1];
    for (&k, &d) in keys.iter().zip(&depth) {
        bit_lengths[(k & 0xFF) as usize] = d;
    }
    let max_bits = depth[..n].iter().copied().max().unwrap_or(0);
    (bit_lengths, max_bits)
}

/// Serializes explicit Huffman weights, preferring the FSE-compressed form
/// when it is smaller, as C zstd's `HUF_writeCTable` does. Returns `None` when
/// more than 128 weights cannot be FSE-compressed.
fn describe_weights(explicit: &[u8], hist: &[u32; WEIGHT_HIST_LEN]) -> Option<Vec<u8>> {
    let num_symbols = explicit.len();
    let direct_len = 1 + num_symbols.div_ceil(2);
    if let Some(compressed) = compress_weights(explicit, hist)
        && (num_symbols > 128 || 1 + compressed.len() < direct_len)
    {
        let mut out = Vec::with_capacity(1 + compressed.len());
        out.push(compressed.len() as u8);
        out.extend_from_slice(&compressed);
        return Some(out);
    }
    if num_symbols > 128 {
        return None;
    }
    let mut out = Vec::with_capacity(direct_len);
    out.push((num_symbols + 127) as u8);
    for pair in explicit.chunks(2) {
        out.push((pair[0] << 4) | pair.get(1).copied().unwrap_or(0));
    }
    Some(out)
}

/// FSE-compresses Huffman weights as in C zstd's `HUF_compressWeights`.
///
/// Two states share one table: the first encodes even-indexed weights, the
/// second odd-indexed ones. Returns `None` when the weights use a single
/// value or the result does not fit the one-byte size header (< 128).
fn compress_weights(weights: &[u8], hist: &[u32; WEIGHT_HIST_LEN]) -> Option<Vec<u8>> {
    // C zstd's FSE_optimalTableLog settles on the minimum log, 5, for every
    // weight count the format allows.
    compress_weights_with_log(weights, hist, 5)
}

/// Entries in a histogram of weights: 0 through `MAX_BITS + 1`.
const WEIGHT_HIST_LEN: usize = MAX_BITS as usize + 2;

/// Counts each weight value.
fn weight_histogram(weights: &[u8]) -> [u32; WEIGHT_HIST_LEN] {
    let mut hist = [0u32; WEIGHT_HIST_LEN];
    for &w in weights {
        hist[usize::from(w) % WEIGHT_HIST_LEN] += 1;
    }
    hist
}

fn compress_weights_with_log(
    weights: &[u8],
    hist: &[u32; WEIGHT_HIST_LEN],
    accuracy_log: u8,
) -> Option<Vec<u8>> {
    use crate::fse::encode::{FseEncodeState, FseEncodeTable};
    use crate::fse::table_builder::{normalize_counts, serialize_fse_table_description};

    let n = weights.len();
    if n < 2 {
        return None;
    }
    debug_assert_eq!(hist, &weight_histogram(weights));
    let max_w = hist.iter().rposition(|&c| c > 0)?;
    if hist[..=max_w].iter().filter(|&&c| c > 0).count() < 2 {
        return None;
    }

    let dist = normalize_counts(&hist[..=max_w], accuracy_log);
    let table = FseEncodeTable::from_distribution(&dist, accuracy_log);
    let mut out = serialize_fse_table_description(&dist, accuracy_log);

    // Encode backward so the decoder reads weights front to back, starting
    // with the first state. Bits go through a local 64-bit accumulator; each
    // step adds at most accuracy_log bits.
    let mut acc: u64 = 0;
    let mut used: u32 = 0;
    macro_rules! put {
        ($val:expr, $nb:expr) => {
            acc |= u64::from($val) << used;
            used += $nb;
            if used >= 32 {
                out.extend_from_slice(&(acc as u32).to_le_bytes());
                acc >>= 32;
                used -= 32;
            }
        };
    }
    macro_rules! encode {
        ($state:ident, $sym:expr) => {{
            let tt = table.symbol_tt[$sym as usize];
            let nb = $state.wrapping_add(tt.delta_nb_bits) >> 16;
            put!($state & ((1u32 << nb) - 1), nb);
            $state = u32::from(
                table.state_table[(($state >> nb) as i32 + tt.delta_find_state) as usize],
            );
        }};
    }
    let init = |sym: u8| FseEncodeState::init(&table, sym).state();

    let mut i = n;
    let (mut s1, mut s2);
    if n % 2 == 1 {
        s1 = init(weights[n - 1]);
        s2 = init(weights[n - 2]);
        i -= 3;
        encode!(s1, weights[i]);
    } else {
        s2 = init(weights[n - 1]);
        s1 = init(weights[n - 2]);
        i -= 2;
    }
    while i > 0 {
        encode!(s2, weights[i - 1]);
        encode!(s1, weights[i - 2]);
        i -= 2;
    }
    let mask = (1u32 << accuracy_log) - 1;
    put!(s2 & mask, u32::from(accuracy_log));
    put!(s1 & mask, u32::from(accuracy_log));
    put!(1u32, 1);
    while used > 0 {
        out.push(acc as u8);
        acc >>= 8;
        used = used.saturating_sub(8);
    }

    (out.len() < 128).then_some(out)
}

/// Counts byte frequencies into four interleaved tables, so runs of one byte
/// value do not serialize on a single counter.
pub fn byte_histogram(data: &[u8]) -> [u32; MAX_SYMBOL_VALUE + 1] {
    let mut t = [[0u32; MAX_SYMBOL_VALUE + 1]; 4];
    let (chunks, tail) = data.as_chunks::<4>();
    for c in chunks {
        t[0][c[0] as usize] += 1;
        t[1][c[1] as usize] += 1;
        t[2][c[2] as usize] += 1;
        t[3][c[3] as usize] += 1;
    }
    for &b in tail {
        t[0][b as usize] += 1;
    }
    core::array::from_fn(|i| t[0][i] + t[1][i] + t[2][i] + t[3][i])
}

fn pack_codes(
    codes: &[u16; MAX_SYMBOL_VALUE + 1],
    num_bits: &[u8; MAX_SYMBOL_VALUE + 1],
) -> [u32; MAX_SYMBOL_VALUE + 1] {
    core::array::from_fn(|s| u32::from(codes[s]) | (u32::from(num_bits[s]) << 16))
}

/// Canonical codes for the symbols in `syms` (in symbol order) with these
/// weights: by weight, then by symbol, as the decoder assigns them.
fn build_encode_codes(
    weights: &[u8; MAX_SYMBOL_VALUE + 1],
    syms: &[u8],
    table_log: u8,
) -> [u16; MAX_SYMBOL_VALUE + 1] {
    // Weights are at most `table_log + 1 <= 12`, so `& 15` only drops the
    // bounds checks.
    let mut rank_count = [0u32; 16];
    for &s in syms {
        rank_count[usize::from(weights[s as usize]) & 15] += 1;
    }

    let mut rank_start = [0u32; 16];
    let mut cumul = 0u32;
    for w in 1..=usize::from(table_log) + 1 {
        rank_start[w & 15] = cumul;
        cumul += rank_count[w & 15] << (w - 1);
    }

    let mut codes = [0u16; MAX_SYMBOL_VALUE + 1];
    for &s in syms {
        let w = usize::from(weights[s as usize]);
        let start = rank_start[w & 15];
        codes[s as usize] = (start >> (w - 1)) as u16;
        rank_start[w & 15] += 1u32 << (w - 1);
    }
    codes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fibonacci counts: the optimal tree for 20 symbols is 19 levels deep.
    fn fibonacci_symbols() -> Vec<u8> {
        let mut data = Vec::new();
        let (mut a, mut b) = (1usize, 1usize);
        for sym in 0..20u8 {
            data.extend(core::iter::repeat_n(sym, a));
            (a, b) = (b, a + b);
        }
        data
    }

    #[test]
    fn compressed_weights_roundtrip_through_parser() {
        // 200 symbols with skewed counts force the FSE-compressed header.
        let mut data = Vec::new();
        for s in 0..200u32 {
            data.extend(core::iter::repeat_n(s as u8, 1 + (s * 7 % 23) as usize));
        }
        let table = HuffmanEncodeTable::from_data(&data).expect("table");
        let desc = table.serialize_weights();
        assert!(desc[0] < 128, "expected compressed header");
        let (parsed, used) = crate::huffman::weights::parse_huffman_weights(&desc).expect("parse");
        assert_eq!(used, desc.len());
        assert_eq!(&parsed[..], &table.explicit_weights()[..]);
    }

    #[test]
    fn compressed_weights_with_rare_weight_roundtrip() {
        // One symbol far rarer than the rest gets a weight value that occurs
        // once, which normalizes to a "less than one" (-1) FSE probability.
        let mut data = Vec::new();
        for s in 0..180u32 {
            data.extend(core::iter::repeat_n(s as u8, 40 + (s % 3) as usize));
        }
        data.extend(core::iter::repeat_n(200u8, 5000));
        data.push(201);
        let table = HuffmanEncodeTable::from_data(&data).expect("table");
        let desc = table.serialize_weights();
        assert!(desc[0] < 128, "expected compressed header");
        let (parsed, _) = crate::huffman::weights::parse_huffman_weights(&desc).expect("parse");
        assert_eq!(&parsed[..], &table.explicit_weights()[..]);
    }

    #[test]
    fn uniform_bytes_terminate_without_a_table() {
        // 256 equally frequent symbols give 256 equal weights. The FSE weight
        // header cannot code a single value, and flattening cannot change
        // that. Construction must give up instead of looping.
        let data: Vec<u8> = (0..8192u32).map(|i| i as u8).collect();
        assert!(HuffmanEncodeTable::from_data(&data).is_none());
    }

    #[test]
    fn from_data_limits_code_length() {
        let table = HuffmanEncodeTable::from_data(&fibonacci_symbols())
            .expect("a deep tree must be length-limited, not rejected");
        let tl = table.table_log;
        assert!(tl <= MAX_BITS);
        assert!(table.num_bits.iter().all(|&n| n <= tl));
        // zstd derives the last weight from the rest, so the code must be
        // complete: the Kraft sum is exactly 2^table_log.
        let kraft: u32 = table.num_bits[..20].iter().map(|&n| 1u32 << (tl - n)).sum();
        assert_eq!(kraft, 1 << tl);
    }

    #[test]
    fn from_decode_table_roundtrip() {
        let data = b"hello world hello world hello world!";
        let original = HuffmanEncodeTable::from_data(data).unwrap();
        let weights_raw = original.serialize_weights();

        let (parsed_weights, _) =
            crate::huffman::weights::parse_huffman_weights(&weights_raw).unwrap();
        let (decode_table, decode_log) =
            crate::huffman::weights::build_huffman_decode_table(&parsed_weights).unwrap();

        let rebuilt = HuffmanEncodeTable::from_decode_table(&decode_table, decode_log).unwrap();
        assert_eq!(original.table_log, rebuilt.table_log);
        assert_eq!(original.explicit_weights(), rebuilt.explicit_weights());
        assert_eq!(original.num_bits, rebuilt.num_bits);
        assert_eq!(original.packed, rebuilt.packed);

        let encoded = rebuilt.encode_single_stream(data);
        let decoded = crate::huffman::decode::decode_single_stream(
            &decode_table,
            decode_log,
            &encoded,
            data.len(),
        )
        .unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn from_decode_table_skewed() {
        let mut data = vec![0u8; 900];
        data.extend(vec![1u8; 80]);
        data.extend(vec![2u8; 15]);
        data.extend(vec![3u8; 5]);
        let original = HuffmanEncodeTable::from_data(&data).unwrap();
        let weights_raw = original.serialize_weights();

        let (parsed_weights, _) =
            crate::huffman::weights::parse_huffman_weights(&weights_raw).unwrap();
        let (decode_table, decode_log) =
            crate::huffman::weights::build_huffman_decode_table(&parsed_weights).unwrap();

        let rebuilt = HuffmanEncodeTable::from_decode_table(&decode_table, decode_log).unwrap();
        assert_eq!(original.packed, rebuilt.packed);
        assert_eq!(original.num_bits, rebuilt.num_bits);

        let encoded = rebuilt.encode_single_stream(&data);
        let decoded = crate::huffman::decode::decode_single_stream(
            &decode_table,
            decode_log,
            &encoded,
            data.len(),
        )
        .unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn roundtrip_simple() {
        let data = b"hello world hello world hello world!";
        let table = HuffmanEncodeTable::from_data(data).unwrap();
        let weights_raw = table.serialize_weights();
        let encoded = table.encode_single_stream(data);

        let (parsed_weights, _) =
            crate::huffman::weights::parse_huffman_weights(&weights_raw).unwrap();
        let (decode_table, decode_log) =
            crate::huffman::weights::build_huffman_decode_table(&parsed_weights).unwrap();
        let decoded = crate::huffman::decode::decode_single_stream(
            &decode_table,
            decode_log,
            &encoded,
            data.len(),
        )
        .unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn roundtrip_4_streams() {
        let data: Vec<u8> = b"ABCDEFGH".iter().cycle().take(1024).copied().collect();
        let table = HuffmanEncodeTable::from_data(&data).unwrap();
        let weights_raw = table.serialize_weights();
        let encoded = table.encode_4_streams(&data);

        let (parsed_weights, _) =
            crate::huffman::weights::parse_huffman_weights(&weights_raw).unwrap();
        let (decode_table, decode_log) =
            crate::huffman::weights::build_huffman_decode_table(&parsed_weights).unwrap();
        let decoded = crate::huffman::decode::decode_4_streams(
            &decode_table,
            decode_log,
            &encoded,
            data.len(),
        )
        .unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn roundtrip_all_bytes() {
        let data: Vec<u8> = (0u8..=127).cycle().take(4096).collect();
        let table = HuffmanEncodeTable::from_data(&data).unwrap();
        let weights_raw = table.serialize_weights();
        let encoded = table.encode_single_stream(&data);

        let (parsed_weights, _) =
            crate::huffman::weights::parse_huffman_weights(&weights_raw).unwrap();
        let (decode_table, decode_log) =
            crate::huffman::weights::build_huffman_decode_table(&parsed_weights).unwrap();
        let decoded = crate::huffman::decode::decode_single_stream(
            &decode_table,
            decode_log,
            &encoded,
            data.len(),
        )
        .unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn skewed_distribution() {
        let mut data = vec![0u8; 900];
        data.extend(vec![1u8; 80]);
        data.extend(vec![2u8; 15]);
        data.extend(vec![3u8; 5]);
        let table = HuffmanEncodeTable::from_data(&data).unwrap();
        assert!(table.num_bits[0] < table.num_bits[3]);
        let weights_raw = table.serialize_weights();
        let encoded = table.encode_single_stream(&data);

        let (parsed_weights, _) =
            crate::huffman::weights::parse_huffman_weights(&weights_raw).unwrap();
        let (decode_table, decode_log) =
            crate::huffman::weights::build_huffman_decode_table(&parsed_weights).unwrap();
        let decoded = crate::huffman::decode::decode_single_stream(
            &decode_table,
            decode_log,
            &encoded,
            data.len(),
        )
        .unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn two_symbols() {
        let mut data = vec![0u8; 500];
        data.extend(vec![1u8; 500]);
        let table = HuffmanEncodeTable::from_data(&data).unwrap();
        assert_eq!(table.num_bits[0], 1);
        assert_eq!(table.num_bits[1], 1);
        let encoded = table.encode_single_stream(&data);

        let weights_raw = table.serialize_weights();
        let (parsed_weights, _) =
            crate::huffman::weights::parse_huffman_weights(&weights_raw).unwrap();
        let (decode_table, decode_log) =
            crate::huffman::weights::build_huffman_decode_table(&parsed_weights).unwrap();
        let decoded = crate::huffman::decode::decode_single_stream(
            &decode_table,
            decode_log,
            &encoded,
            data.len(),
        )
        .unwrap();
        assert_eq!(decoded, data);
    }
}
