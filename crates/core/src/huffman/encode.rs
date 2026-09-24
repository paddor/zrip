#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec;
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
        let max_sym = freqs.iter().rposition(|&f| f > 0).unwrap_or(0) as u8;

        let num_symbols = max_sym as usize + 1;
        let active_count = freqs[..num_symbols].iter().filter(|&&f| f > 0).count();
        if active_count < 2 {
            return None;
        }

        // Beyond 128 explicit weights only the FSE-compressed form exists, and
        // it must fit in 127 bytes. If it does not, flatten the counts: fewer
        // distinct weights compress better.
        let mut counts = *freqs;
        let (weights, table_log, description) = loop {
            let (weights, table_log) = compute_huffman_weights(&counts, num_symbols)?;
            if let Some(description) = describe_weights(&weights[..max_sym as usize]) {
                break (weights, table_log, description);
            }
            if counts.iter().all(|&c| c <= 1) {
                return None;
            }
            // (c + 1) / 2 keeps every symbol present and reaches 1 for all,
            // so the loop ends.
            for c in counts.iter_mut().filter(|c| **c > 0) {
                *c = c.div_ceil(2);
            }
        };
        let (codes, num_bits) = build_encode_codes(&weights, table_log);

        Some(Self {
            packed: pack_codes(&codes, &num_bits),
            description,
            num_bits,
            table_log,
        })
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

        let num_symbols = max_sym as usize + 1;
        let active_count = seen[..num_symbols].iter().filter(|&&s| s).count();
        if active_count < 2 {
            return None;
        }

        let mut weights = vec![0u8; num_symbols];
        for s in 0..num_symbols {
            if seen[s] {
                weights[s] = table_log + 1 - num_bits_per_sym[s];
            }
        }

        let (codes, num_bits) = build_encode_codes(&weights, table_log);
        let description = describe_weights(&weights[..max_sym as usize]).unwrap_or_default();

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
        debug_assert!(!self.description.is_empty());
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

fn compute_huffman_weights(freqs: &[u32], num_symbols: usize) -> Option<(Vec<u8>, u8)> {
    let mut active: Vec<(u64, usize)> = freqs[..num_symbols]
        .iter()
        .enumerate()
        .filter(|(_, f)| **f > 0)
        .map(|(s, &f)| (f as u64, s))
        .collect();

    if active.len() < 2 {
        return None;
    }

    // An optimal tree can be deeper than MAX_BITS. Flatten the frequencies
    // and rebuild until it fits, as bzip2 does. Each pass halves the spread
    // between counts, so the depth converges to ceil(log2(active)) <= 8.
    loop {
        let bit_lengths = huffman_bit_lengths(&active, num_symbols);
        let max_bl = *bit_lengths.iter().max().unwrap();
        if max_bl == 0 {
            return None;
        }
        if max_bl <= MAX_BITS {
            let table_log = max_bl;
            let mut weights = vec![0u8; num_symbols];
            for (s, &bl) in bit_lengths.iter().enumerate() {
                if bl > 0 {
                    weights[s] = table_log + 1 - bl;
                }
            }
            return Some((weights, table_log));
        }
        for (f, _) in &mut active {
            *f = 1 + *f / 2;
        }
    }
}

/// Builds unrestricted Huffman code lengths for the `(frequency, symbol)`
/// pairs in `active`.
fn huffman_bit_lengths(active: &[(u64, usize)], num_symbols: usize) -> Vec<u8> {
    // Two-queue construction: leaves sorted by (frequency, index), internal
    // nodes created in nondecreasing frequency order. Preferring the leaf on
    // equal frequency picks the same node as a min-heap keyed by
    // (frequency, id), because leaf ids precede internal ids.
    const MAX_NODES: usize = 2 * (MAX_SYMBOL_VALUE + 1);
    let n = active.len();
    debug_assert!((2..=MAX_SYMBOL_VALUE + 1).contains(&n));

    // Sort packed (frequency, index) keys. Block literal counts stay below
    // 2^17 and flattening only lowers them, so a u32 holds both fields.
    let mut keys = [0u32; MAX_SYMBOL_VALUE + 1];
    for (i, (k, &(f, _))) in keys.iter_mut().zip(active).enumerate() {
        debug_assert!(f < 1 << 23);
        *k = ((f as u32) << 9) | i as u32;
    }
    keys[..n].sort_unstable();
    let mut order = [0u16; MAX_SYMBOL_VALUE + 1];
    for (o, &k) in order.iter_mut().zip(&keys[..n]) {
        *o = (k & 0x1FF) as u16;
    }

    let mut freq = [0u64; MAX_NODES];
    for (f, &(af, _)) in freq.iter_mut().zip(active) {
        *f = af;
    }
    let mut parent = [0u16; MAX_NODES];
    let mut leaf = 0usize;
    let mut internal = n;
    for next in n..2 * n - 1 {
        let mut pick = || {
            if leaf < n && (internal >= next || freq[order[leaf] as usize] <= freq[internal]) {
                leaf += 1;
                order[leaf - 1] as usize
            } else {
                internal += 1;
                internal - 1
            }
        };
        let a = pick();
        let b = pick();
        freq[next] = freq[a] + freq[b];
        parent[a] = next as u16;
        parent[b] = next as u16;
    }

    // Parents always follow their children, so one backward pass from the
    // root assigns every depth.
    let root = 2 * n - 2;
    let mut depth = [0u8; MAX_NODES];
    for k in (0..root).rev() {
        depth[k] = depth[parent[k] as usize] + 1;
    }

    let mut bit_lengths = vec![0u8; num_symbols];
    for (i, &(_, sym)) in active.iter().enumerate() {
        bit_lengths[sym] = depth[i];
    }
    bit_lengths
}

/// Serializes explicit Huffman weights, preferring the FSE-compressed form
/// when it is smaller, as C zstd's `HUF_writeCTable` does. Returns `None` when
/// more than 128 weights cannot be FSE-compressed.
fn describe_weights(explicit: &[u8]) -> Option<Vec<u8>> {
    let num_symbols = explicit.len();
    let direct_len = 1 + num_symbols.div_ceil(2);
    if let Some(compressed) = compress_weights(explicit)
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
fn compress_weights(weights: &[u8]) -> Option<Vec<u8>> {
    // C zstd's FSE_optimalTableLog settles on the minimum log, 5, for every
    // weight count the format allows.
    compress_weights_with_log(weights, 5)
}

fn compress_weights_with_log(weights: &[u8], accuracy_log: u8) -> Option<Vec<u8>> {
    use crate::fse::encode::{FseEncodeState, FseEncodeTable};
    use crate::fse::table_builder::{normalize_counts, serialize_fse_table_description};

    let n = weights.len();
    if n < 2 {
        return None;
    }
    let max_w = *weights.iter().max()? as usize;
    let mut hist = [0u32; MAX_BITS as usize + 2];
    for &w in weights {
        hist[w as usize] += 1;
    }
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

fn build_encode_codes(
    weights: &[u8],
    table_log: u8,
) -> ([u16; MAX_SYMBOL_VALUE + 1], [u8; MAX_SYMBOL_VALUE + 1]) {
    let mut codes = [0u16; MAX_SYMBOL_VALUE + 1];
    let mut num_bits = [0u8; MAX_SYMBOL_VALUE + 1];

    let max_w = table_log + 1;
    let mut rank_count = [0u32; MAX_BITS as usize + 2];

    for (s, &w) in weights.iter().enumerate() {
        if w > 0 && w <= max_w {
            num_bits[s] = table_log + 1 - w;
            rank_count[w as usize] += 1;
        }
    }

    let mut rank_start = [0u32; MAX_BITS as usize + 2];
    let mut cumul = 0u32;
    for w in 1..=max_w {
        rank_start[w as usize] = cumul;
        cumul += rank_count[w as usize] * (1u32 << (w - 1));
    }

    for (s, &w) in weights.iter().enumerate() {
        if w == 0 {
            continue;
        }
        let start = rank_start[w as usize];
        codes[s] = (start >> (w - 1)) as u16;
        rank_start[w as usize] += 1u32 << (w - 1);
    }

    (codes, num_bits)
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
