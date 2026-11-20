//! Huffman encoder: length-limited codes, tree descriptions, and single-
//! and 4-stream encoding, after C zstd's `huf_compress.c`.

#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::huffman::{MAX_BITS, MAX_SYMBOL_VALUE};

/// Huffman codes for one set of literals, with its serialized tree
/// description.
#[derive(Clone)]
pub struct HuffmanEncodeTable {
    /// Code length per symbol; 0 for absent symbols.
    num_bits: [u8; MAX_SYMBOL_VALUE + 1],
    /// Per symbol, the code in the top `num_bits` bits and `num_bits` in the
    /// low bits (C zstd's `HUF_CElt`), so one load feeds the whole encode
    /// step. Absent symbols are 0.
    celt: [u64; MAX_SYMBOL_VALUE + 1],
    /// Serialized weight description, built once with the table.
    description: Description,
    /// Longest code length, at most [`MAX_BITS`].
    table_log: u8,
}

#[cfg(feature = "alloc")]
impl HuffmanEncodeTable {
    /// Builds a table for the byte counts of `data`. `None` for fewer than
    /// two distinct bytes or when no table can be described.
    pub fn from_data(data: &[u8]) -> Option<Self> {
        if data.is_empty() {
            return None;
        }
        Self::from_histogram(&byte_histogram(data))
    }

    /// Builds a table from byte counts, as returned by [`byte_histogram`].
    pub fn from_histogram(freqs: &[u32; MAX_SYMBOL_VALUE + 1]) -> Option<Self> {
        let max_sym = freqs.iter().rposition(|&f| f > 0)?;

        // The symbols that occur, in symbol order, and their counts. Every
        // later step works on these lists instead of all 256 counts.
        let mut syms = [0u8; MAX_SYMBOL_VALUE + 1];
        let mut counts = [0u32; MAX_SYMBOL_VALUE + 1];
        let mut n = 0usize;
        for (s, &f) in freqs[..=max_sym].iter().enumerate() {
            syms[n & MAX_SYMBOL_VALUE] = s as u8;
            counts[n & MAX_SYMBOL_VALUE] = f;
            n += usize::from(f > 0);
        }
        if n < 2 {
            return None;
        }
        let syms = &syms[..n];
        let counts = &mut counts[..n];

        // Beyond 128 explicit weights only the FSE-compressed form exists, and
        // it must fit in 127 bytes. If it does not, flatten the counts: fewer
        // distinct weights compress better.
        loop {
            let (num_bits, table_log) = code_lengths(counts, syms);
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
                return Some(Self {
                    celt: code_entries(&weights, &num_bits, syms, table_log),
                    description,
                    num_bits,
                    table_log,
                });
            }
            if counts.iter().all(|&c| c <= 1) {
                return None;
            }
            // (c + 1) / 2 keeps every symbol present and reaches 1 for all,
            // so the loop ends.
            for c in counts.iter_mut() {
                *c = c.div_ceil(2);
            }
        }
    }

    /// Rebuilds the codes of a decode table, such as a dictionary's, to
    /// encode with them. `None` for fewer than two symbols, or codes over
    /// [`MAX_BITS`] bits.
    pub fn from_decode_table(
        decode_table: &[super::HuffmanDecodeEntry],
        table_log: u8,
    ) -> Option<Self> {
        // The encoder writes codes of at most `MAX_BITS`; C zstd's decoder
        // also takes 12-bit tables.
        if table_log > MAX_BITS {
            return None;
        }
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

        // Tables from a decoder, such as a dictionary's, are only reused, so
        // weights past the compressed form's 127 bytes may go undescribed.
        let explicit = &weights[..max_sym as usize];
        let description =
            describe_weights(explicit, &weight_histogram(explicit)).unwrap_or_default();

        Some(Self {
            celt: code_entries(&weights, &num_bits, &syms[..n], table_log),
            description,
            num_bits,
            table_log,
        })
    }

    /// Longest code length in bits.
    pub fn table_log(&self) -> u8 {
        self.table_log
    }

    /// Encoded size in bits of data with these byte counts, or `None` when
    /// the table has no code for a byte that occurs.
    pub fn estimate_bits(&self, freqs: &[u32; MAX_SYMBOL_VALUE + 1]) -> Option<u64> {
        // No early exit, so the loop vectorizes.
        let mut bits = 0u64;
        let mut missing = false;
        for (&f, &n) in freqs.iter().zip(&self.num_bits) {
            missing |= f > 0 && n == 0;
            bits += u64::from(f) * u64::from(n);
        }
        (!missing).then_some(bits)
    }

    /// Whether every byte of `data` has a code.
    pub fn can_encode(&self, data: &[u8]) -> bool {
        for &b in data {
            if self.num_bits[b as usize] == 0 {
                return false;
            }
        }
        true
    }

    /// Owned copy of [`weights_description`](Self::weights_description).
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
        debug_assert_ne!(self.description.len, 0);
        &self.description.bytes[..usize::from(self.description.len)]
    }

    /// Encodes `data` as one reverse stream. Every byte needs a code; see
    /// [`can_encode`](Self::can_encode).
    pub fn encode_single_stream(&self, data: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        let len = self.encode_single_stream_into(data, &mut buf);
        buf.truncate(len);
        buf
    }

    /// Writes the single-stream encoding of `data` to the start of `out`,
    /// growing it as needed, and returns its length. Bytes past it are
    /// scratch, so a reused buffer is not cleared.
    pub fn encode_single_stream_into(&self, data: &[u8], out: &mut Vec<u8>) -> usize {
        let need = stream_bound(data.len());
        if out.len() < need {
            out.resize(need, 0);
        }
        self.encode_stream(data, out)
    }

    /// Encodes `data` as four streams after a 6-byte jump table.
    pub fn encode_4_streams(&self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let len = self.encode_4_streams_into(data, &mut out);
        out.truncate(len);
        out
    }

    /// Writes the 4-stream encoding of `data` (jump table, then the streams)
    /// to the start of `out`, growing it as needed, and returns its length.
    /// Bytes past it are scratch, so a reused buffer is not cleared.
    pub fn encode_4_streams_into(&self, data: &[u8], out: &mut Vec<u8>) -> usize {
        let seg = data.len().div_ceil(4);
        let need = 6 + 4 * stream_bound(seg);
        if out.len() < need {
            out.resize(need, 0);
        }
        let mut pos = 6;
        for k in 0..4 {
            let stream = &data[(seg * k).min(data.len())..(seg * (k + 1)).min(data.len())];
            let len = self.encode_stream(stream, &mut out[pos..]);
            // Streams of a 128 KiB block stay below 64 KiB.
            if k < 3 {
                out[2 * k..2 * k + 2].copy_from_slice(&(len as u16).to_le_bytes());
            }
            pos += len;
        }
        pos
    }

    /// Encodes one stream of `data` to the start of `out`, which holds at
    /// least [`stream_bound`] bytes, and returns its length.
    fn encode_stream(&self, data: &[u8], out: &mut [u8]) -> usize {
        // Codes per flush: `7 + n * table_log` pending bits fit in 64.
        match self.table_log {
            0..=7 => self.encode_stream_n::<8>(data, out),
            8 => self.encode_stream_n::<7>(data, out),
            9 => self.encode_stream_n::<6>(data, out),
            _ => self.encode_stream_n::<5>(data, out),
        }
    }

    /// As in C zstd, the bit container takes each code at its top: it shifts
    /// right by the code length and ORs in the whole entry. The length bits
    /// ORed in at the bottom only move down, and the last code of each group
    /// leaves them out, so they never reach the pending bits at the top. Two
    /// containers take turns on `N` codes each, so their shift chains
    /// overlap.
    fn encode_stream_n<const N: usize>(&self, data: &[u8], out: &mut [u8]) -> usize {
        debug_assert!(7 + N * usize::from(self.table_log) <= 64);
        debug_assert!(out.len() >= stream_bound(data.len()));
        let celt = &self.celt;
        let mut bits: u64 = 0;
        let mut used: u64 = 0;
        let mut pos = 0;

        // Entries add into `used` whole: their code bits carry above the low
        // byte, which keeps the bit count.
        macro_rules! put {
            ($bits:ident, $used:ident, $sym:expr, $mask:expr) => {
                let e = celt[usize::from($sym)];
                $bits = ($bits >> (e & 63)) | (e & $mask);
                $used = $used.wrapping_add(e);
            };
        }
        macro_rules! group {
            ($bits:ident, $used:ident, $syms:expr) => {
                for i in 0..N - 1 {
                    put!($bits, $used, $syms[N - 1 - i], !0);
                }
                put!($bits, $used, $syms[0], !15);
            };
        }
        macro_rules! flush {
            () => {
                let nb = (used & 0xFF) as u32;
                let word = bits.wrapping_shr(64 - nb);
                out[pos..pos + 8].copy_from_slice(&word.to_le_bytes());
                pos += (nb >> 3) as usize;
                used &= 7;
            };
        }

        // Symbols are written last to first. rchunks_exact walks groups from
        // the end; the leftover head is encoded after them, in reverse.
        let chunks = data.rchunks_exact(2 * N);
        let head = chunks.remainder();
        for c in chunks {
            let (lo, hi) = c.split_at(N);
            group!(bits, used, hi);
            flush!();
            let mut bits2: u64 = 0;
            let mut used2: u64 = 0;
            group!(bits2, used2, lo);
            bits = (bits >> (used2 & 63)) | bits2;
            used = used.wrapping_add(used2);
            flush!();
        }
        for group in head.rchunks(N) {
            for &b in group.iter().rev() {
                put!(bits, used, b, !15);
            }
            flush!();
        }
        // End mark: a single 1 bit.
        bits = (bits >> 1) | (1 << 63);
        used += 1;
        flush!();
        pos + usize::from(used != 0)
    }

    /// Size in bytes of the single-stream encoding of `data`, end mark
    /// included.
    pub fn compressed_size_single(&self, data: &[u8]) -> usize {
        let total_bits: usize = data
            .iter()
            .map(|&b| self.num_bits[b as usize] as usize)
            .sum();
        (total_bits + 8) / 8
    }
}

/// Code lengths of at most [`MAX_BITS`] for the symbols in `syms` (at
/// least two, in symbol order) with counts `counts`, as a per-symbol array
/// and the longest length.
fn code_lengths(counts: &[u32], syms: &[u8]) -> ([u8; MAX_SYMBOL_VALUE + 1], u8) {
    let n = syms.len();
    debug_assert!((2..=MAX_SYMBOL_VALUE + 1).contains(&n));
    let mut freq = [0u32; MAX_SYMBOL_VALUE + 1];
    freq[..n].copy_from_slice(counts);
    // Block literal counts stay far below 2^23, so the sum of up to 256 of
    // them fits a u32. Larger counts are scaled down to that range.
    let max = freq[..n].iter().copied().max().unwrap_or(0);
    let scale = (u32::BITS - max.leading_zeros()).saturating_sub(23);
    if scale > 0 {
        for f in &mut freq[..n] {
            *f = (*f >> scale).max(1);
        }
    }

    // An optimal tree can be deeper than MAX_BITS. As in C zstd, build it
    // once and then limit the lengths in place, most frequent symbol first.
    let mut keys = [0u32; MAX_SYMBOL_VALUE + 1];
    let mut depths = [0u8; MAX_SYMBOL_VALUE + 1];
    huffman_depths(&freq[..n], syms, &mut keys, &mut depths);
    let mut bits = [0u8; MAX_SYMBOL_VALUE + 1];
    let mut sorted = [0u32; MAX_SYMBOL_VALUE + 1];
    for p in 0..n {
        bits[p] = depths[n - 1 - p];
        sorted[p] = keys[n - 1 - p] >> 8;
    }
    let max_bits = set_max_height(&mut bits[..n], &sorted[..n], MAX_BITS);
    let mut num_bits = [0u8; MAX_SYMBOL_VALUE + 1];
    for p in 0..n {
        num_bits[(keys[n - 1 - p] & 0xFF) as usize] = bits[p];
    }
    (num_bits, max_bits)
}

/// Limits code lengths to `target` bits, as C zstd's `HUF_setMaxHeight`
/// does, and returns the longest length. `bits` and `counts` are in
/// descending count order, so `bits` is nondecreasing.
///
/// Lengths past `target` drop to it, which overfills the code space. The
/// excess is repaid by lengthening the codes where it costs least: one
/// code at `target - k` bits frees `2^(k-1)` units of `2^-target`. An
/// overshoot is given back by shortening `target`-bit codes.
fn set_max_height(bits: &mut [u8], counts: &[u32], target: u8) -> u8 {
    let last = bits.len() - 1;
    let largest = bits[last];
    if largest <= target {
        return largest;
    }
    let base_cost = 1i32 << (largest - target);
    let mut total_cost = 0i32;
    let mut n = last;
    while bits[n] > target {
        total_cost += base_cost - (1 << (largest - bits[n]));
        bits[n] = target;
        n -= 1;
    }
    // At most 256 symbols cannot all use `target >= 9` bits, so `n` stays
    // in range.
    while bits[n] == target {
        n -= 1;
    }
    total_cost >>= largest - target;

    // The last (least frequent) position of each length below `target`,
    // indexed by `target - length`.
    const NONE: usize = usize::MAX;
    let mut rank_last = [NONE; 16];
    let mut current = target;
    for pos in (0..=n).rev() {
        if bits[pos] < current {
            current = bits[pos];
            rank_last[usize::from(target - current) & 15] = pos;
        }
    }

    while total_cost > 0 {
        // Lengthening a code at `target - k` bits repays 2^(k-1). Try the
        // power of two just above the cost, unless two codes one rank down
        // are cheaper.
        let mut k = (31 - total_cost.leading_zeros()) as usize + 1;
        while k > 1 {
            let high = rank_last[k & 15];
            let low = rank_last[(k - 1) & 15];
            if high == NONE {
                k -= 1;
                continue;
            }
            if low == NONE || counts[high] <= 2 * counts[low] {
                break;
            }
            k -= 1;
        }
        while k <= usize::from(MAX_BITS) && rank_last[k & 15] == NONE {
            k += 1;
        }
        let pos = rank_last[k & 15];
        total_cost -= 1 << (k - 1);
        bits[pos] += 1;
        // The lengthened code joins rank k - 1, as its least frequent
        // member if the rank was empty.
        if rank_last[(k - 1) & 15] == NONE {
            rank_last[(k - 1) & 15] = pos;
        }
        // The next more frequent code ends rank k, if it has that length.
        rank_last[k & 15] = if pos == 0 || bits[pos - 1] != target - k as u8 {
            NONE
        } else {
            pos - 1
        };
    }

    // An overshoot: shorten `target`-bit codes, most frequent first, into
    // rank 1.
    while total_cost < 0 {
        if rank_last[1] == NONE {
            while bits[n] == target {
                n -= 1;
            }
            bits[n + 1] -= 1;
            rank_last[1] = n + 1;
        } else {
            bits[rank_last[1] + 1] -= 1;
            rank_last[1] += 1;
        }
        total_cost += 1;
    }
    target
}

/// Builds an unrestricted Huffman tree for the symbols in `syms` (in symbol
/// order) with counts `freq`. Leaf `i` in ascending (count, symbol) order
/// gets its packed `count << 8 | symbol` key in `keys[i]` and its depth in
/// `depths[i]`.
fn huffman_depths(
    freq: &[u32],
    syms: &[u8],
    keys: &mut [u32; MAX_SYMBOL_VALUE + 1],
    depths: &mut [u8; MAX_SYMBOL_VALUE + 1],
) {
    // Two-queue construction: leaves sorted by (frequency, symbol), internal
    // nodes created in nondecreasing frequency order. Preferring the leaf on
    // equal frequency picks the same node as a min-heap keyed by
    // (frequency, id), because leaf ids precede internal ids.
    const SYMS: usize = MAX_SYMBOL_VALUE + 1;
    let n = syms.len();
    debug_assert!((2..=SYMS).contains(&n));

    // Sort packed (frequency, symbol) keys; frequencies stay below 2^23.
    // Counting sort by frequency, with every count of 255 or more in the
    // last bucket: the buckets keep symbol order, so only that last one
    // needs a comparison sort, and only when it holds counts that differ.
    // Only buckets up to the largest count are used.
    const TOP: usize = 255;
    let max = freq.iter().copied().max().unwrap_or(0) as usize;
    let top = max.min(TOP);
    let mut starts = [0u16; TOP + 1];
    for &f in freq {
        starts[(f as usize).min(top) & TOP] += 1;
    }
    let mut at = 0u16;
    for start in &mut starts[..=top] {
        let count = *start;
        *start = at;
        at += count;
    }
    for (&f, &s) in freq.iter().zip(syms) {
        let bucket = &mut starts[(f as usize).min(top) & TOP];
        keys[usize::from(*bucket) & MAX_SYMBOL_VALUE] = (f << 8) | u32::from(s);
        *bucket += 1;
    }
    let keys = &mut keys[..n];
    if max > TOP {
        // Each start now holds its bucket's end, so this is where the last
        // bucket begins.
        let frequent = usize::from(starts[TOP - 1]);
        keys[frequent..].sort_unstable();
    }

    // Both queues end in a sentinel above every sum (at most 2^31), so a
    // pick needs no emptiness check: the sorted leaves, and the internal
    // nodes in creation order, each set once its children are picked. A
    // valid pick always exists, so the leaf index stops at `n`. `parent`
    // holds the internal node above each leaf `i` at `i`, and above each
    // internal node `j` at `SYMS + j`.
    let mut leaves = [u32::MAX; 2 * SYMS];
    for (l, &k) in leaves.iter_mut().zip(keys.iter()) {
        *l = k >> 8;
    }
    let mut sums = [u32::MAX; SYMS];
    let mut parent = [0u8; 2 * SYMS];
    let (mut leaf, mut internal) = (0usize, 0usize);
    for j in 0..n - 1 {
        let mut pick = || {
            let (l, i) = (leaves[leaf & (2 * SYMS - 1)], sums[internal & (SYMS - 1)]);
            let take_leaf = l <= i;
            let id = if take_leaf { leaf } else { SYMS + internal };
            leaf += usize::from(take_leaf);
            internal += usize::from(!take_leaf);
            (id, if take_leaf { l } else { i })
        };
        let (a, fa) = pick();
        let (b, fb) = pick();
        sums[j] = fa + fb;
        parent[a & (2 * SYMS - 1)] = j as u8;
        parent[b & (2 * SYMS - 1)] = j as u8;
    }

    // Parents always follow their children, so one backward pass from the
    // root (the last internal node) assigns every depth.
    let mut depth = [0u8; SYMS];
    for j in (0..n - 2).rev() {
        depth[j] = depth[usize::from(parent[SYMS + j])] + 1;
    }

    for (d, &p) in depths[..n].iter_mut().zip(&parent) {
        *d = depth[usize::from(p)] + 1;
    }
}

/// A serialized weight description. The FSE form takes a size byte and
/// under 128 bytes, and the direct form at most 65, so neither needs the
/// heap.
#[derive(Clone, Copy)]
struct Description {
    bytes: [u8; 128],
    len: u8,
}

impl Default for Description {
    fn default() -> Self {
        Self {
            bytes: [0; 128],
            len: 0,
        }
    }
}

/// Serializes explicit Huffman weights, preferring the FSE-compressed form
/// when it is smaller, as C zstd's `HUF_writeCTable` does. Returns `None` when
/// more than 128 weights cannot be FSE-compressed.
fn describe_weights(explicit: &[u8], hist: &[u32; WEIGHT_HIST_LEN]) -> Option<Description> {
    let num_symbols = explicit.len();
    let direct_len = 1 + num_symbols.div_ceil(2);
    let mut d = Description::default();
    if let Some(len) = compress_weights(explicit, hist, &mut d.bytes[1..])
        && (num_symbols > 128 || 1 + len < direct_len)
    {
        d.bytes[0] = len as u8;
        d.len = 1 + len as u8;
        return Some(d);
    }
    if num_symbols > 128 {
        return None;
    }
    d.bytes[0] = (num_symbols + 127) as u8;
    for (b, pair) in d.bytes[1..].iter_mut().zip(explicit.chunks(2)) {
        *b = (pair[0] << 4) | pair.get(1).copied().unwrap_or(0);
    }
    d.len = direct_len as u8;
    Some(d)
}

/// FSE-compresses Huffman weights into `out` (127 bytes) as in C zstd's
/// `HUF_compressWeights`, and returns the length.
///
/// Two states share one table: the first encodes even-indexed weights, the
/// second odd-indexed ones. Returns `None` when the weights use a single
/// value or the result does not fit the one-byte size header (< 128).
fn compress_weights(
    weights: &[u8],
    hist: &[u32; WEIGHT_HIST_LEN],
    out: &mut [u8],
) -> Option<usize> {
    use crate::fse::encode::{SymbolTransform, build_encode_table};
    use crate::fse::table_builder::{normalize_counts_into, write_fse_table_description};

    // C zstd's FSE_optimalTableLog settles on the minimum log, 5, for every
    // weight count the format allows.
    const LOG: u8 = 5;
    const STATES: usize = 1 << LOG;
    let n = weights.len();
    if n < 2 {
        return None;
    }
    debug_assert_eq!(hist, &weight_histogram(weights));
    let max_w = hist.iter().rposition(|&c| c > 0)?;
    if hist[..=max_w].iter().filter(|&&c| c > 0).count() < 2 {
        return None;
    }
    let syms = max_w + 1;
    let mut dist = [0i16; WEIGHT_HIST_LEN];
    normalize_counts_into(&hist[..syms], LOG, &mut dist[..syms]);
    let mut tt = [SymbolTransform {
        delta_nb_bits: 0,
        delta_find_state: 0,
    }; WEIGHT_HIST_LEN];
    let mut states = [0u16; STATES];
    let mut spread = [0u16; STATES];
    let mut cumul = [0u32; WEIGHT_HIST_LEN + 1];
    build_encode_table(
        &dist[..syms],
        LOG,
        &mut tt[..syms],
        &mut states,
        &mut spread,
        &mut cumul[..=syms],
    );

    // The table description, padded to a byte, then the weights encoded
    // backward so the decoder reads them front to back, starting with the
    // first state. Bits go through a 64-bit accumulator; each step adds at
    // most 7 bits. Output past 128 bytes is dropped, since it fails anyway.
    let mut w = WeightBits {
        buf: [0; 128],
        len: 0,
        acc: 0,
        used: 0,
    };
    write_fse_table_description(&dist[..syms], LOG, |v, nb| w.put(v, u32::from(nb)));
    w.put(0, (8 - w.used % 8) % 8);

    macro_rules! encode {
        ($state:ident, $sym:expr) => {{
            let t = tt[usize::from($sym) % WEIGHT_HIST_LEN];
            let nb = $state.wrapping_add(t.delta_nb_bits) >> 16;
            w.put($state & ((1u32 << nb) - 1), nb);
            $state =
                u32::from(states[((($state >> nb) as i32 + t.delta_find_state) as usize) % STATES]);
        }};
    }
    // C zstd's FSE_initCState2.
    let init = |sym: u8| {
        let t = tt[usize::from(sym) % WEIGHT_HIST_LEN];
        let nb = t.delta_nb_bits.wrapping_add(1 << 16) >> 16;
        let value = (nb << 16).wrapping_sub(t.delta_nb_bits);
        u32::from(states[((value >> nb) as i32 + t.delta_find_state) as usize % STATES])
    };

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
    let mask = (1u32 << LOG) - 1;
    w.put(s2 & mask, u32::from(LOG));
    w.put(s1 & mask, u32::from(LOG));
    w.put(1, 1);
    let len = w.finish();
    if len >= 128 {
        return None;
    }
    out[..len].copy_from_slice(&w.buf[..len]);
    Some(len)
}

/// A little-endian bit accumulator over 128 bytes, for weight descriptions.
/// Bytes past the end are dropped; such a description is too long anyway.
struct WeightBits {
    buf: [u8; 128],
    len: usize,
    acc: u64,
    used: u32,
}

impl WeightBits {
    fn put(&mut self, value: u32, nb: u32) {
        self.acc |= u64::from(value) << self.used;
        self.used += nb;
        if self.used >= 32 {
            if let Some(dst) = self.buf.get_mut(self.len..self.len + 4) {
                dst.copy_from_slice(&(self.acc as u32).to_le_bytes());
            }
            self.len += 4;
            self.acc >>= 32;
            self.used -= 32;
        }
    }

    /// Writes the pending bits, padded to a byte, and returns the length.
    fn finish(&mut self) -> usize {
        let tail = self.used.div_ceil(8) as usize;
        for k in 0..tail {
            if let Some(b) = self.buf.get_mut(self.len + k) {
                *b = (self.acc >> (8 * k)) as u8;
            }
        }
        self.len + tail
    }
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

/// Counts byte frequencies into four interleaved tables, so runs of one byte
/// value do not serialize on a single counter. Each 8-byte word is loaded
/// once and split with shifts, which takes 15% less time than byte loads.
pub fn byte_histogram(data: &[u8]) -> [u32; MAX_SYMBOL_VALUE + 1] {
    let mut t = [[0u32; MAX_SYMBOL_VALUE + 1]; 4];
    let (chunks, tail) = data.as_chunks::<8>();
    for c in chunks {
        let w = u64::from_le_bytes(*c);
        t[0][(w & 0xFF) as usize] += 1;
        t[1][((w >> 8) & 0xFF) as usize] += 1;
        t[2][((w >> 16) & 0xFF) as usize] += 1;
        t[3][((w >> 24) & 0xFF) as usize] += 1;
        t[0][((w >> 32) & 0xFF) as usize] += 1;
        t[1][((w >> 40) & 0xFF) as usize] += 1;
        t[2][((w >> 48) & 0xFF) as usize] += 1;
        t[3][(w >> 56) as usize] += 1;
    }
    for &b in tail {
        t[0][b as usize] += 1;
    }
    core::array::from_fn(|i| t[0][i] + t[1][i] + t[2][i] + t[3][i])
}

/// Worst-case bytes [`HuffmanEncodeTable::encode_stream`] touches for `n`
/// symbols: `MAX_BITS` per symbol plus the end mark, and its last 8-byte
/// store.
const fn stream_bound(n: usize) -> usize {
    n + n / 2 + 16
}

/// Canonical codes for the symbols in `syms` (in symbol order) with these
/// weights, by weight, then by symbol, as the decoder assigns them. Each is
/// left-aligned with its length in the low bits, as
/// [`HuffmanEncodeTable::encode_stream`] takes them; absent symbols get 0.
fn code_entries(
    weights: &[u8; MAX_SYMBOL_VALUE + 1],
    num_bits: &[u8; MAX_SYMBOL_VALUE + 1],
    syms: &[u8],
    table_log: u8,
) -> [u64; MAX_SYMBOL_VALUE + 1] {
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

    let mut celt = [0u64; MAX_SYMBOL_VALUE + 1];
    for &s in syms {
        let w = usize::from(weights[s as usize]);
        let start = rank_start[w & 15];
        rank_start[w & 15] += 1u32 << (w - 1);
        // Present symbols have 1 to `MAX_BITS` bits.
        let nb = u32::from(num_bits[s as usize]);
        celt[s as usize] = (u64::from(start >> (w - 1)) << (64 - nb)) | u64::from(nb);
    }
    celt
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
        assert_eq!(original.celt, rebuilt.celt);

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
        assert_eq!(original.celt, rebuilt.celt);
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
