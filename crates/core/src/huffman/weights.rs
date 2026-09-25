#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::bitstream::reader::BitReader;
use crate::bitstream::reader_reverse::ReverseBitReader;
use crate::error::DecompressError;
use crate::fse::decode::FseState;
use crate::fse::table_builder::build_decode_table;

pub fn parse_huffman_weights(data: &[u8]) -> Result<(Vec<u8>, usize), DecompressError> {
    if data.is_empty() {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let header_byte = data[0];

    if header_byte >= 128 {
        parse_direct_weights(data)
    } else {
        parse_fse_compressed_weights(data)
    }
}

#[cfg(feature = "alloc")]
pub fn parse_huffman_weights_into(
    data: &[u8],
    weights: &mut Vec<u8>,
    fse_table: &mut Vec<crate::fse::FseDecodeEntry>,
    fse_symbol_next: &mut Vec<u16>,
    fse_dist: &mut Vec<i16>,
) -> Result<usize, DecompressError> {
    if data.is_empty() {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let header_byte = data[0];

    if header_byte >= 128 {
        parse_direct_weights_into(data, weights)
    } else {
        parse_fse_compressed_weights_into(data, weights, fse_table, fse_symbol_next, fse_dist)
    }
}

fn parse_direct_weights(data: &[u8]) -> Result<(Vec<u8>, usize), DecompressError> {
    let num_symbols = (data[0] as usize) - 127;
    let num_bytes = num_symbols.div_ceil(2);

    if data.len() < 1 + num_bytes {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let mut weights = Vec::with_capacity(num_symbols);
    for i in 0..num_symbols {
        let byte_idx = 1 + i / 2;
        let weight = if i % 2 == 0 {
            data[byte_idx] >> 4
        } else {
            data[byte_idx] & 0x0F
        };
        weights.push(weight);
    }

    Ok((weights, 1 + num_bytes))
}

#[cfg(feature = "alloc")]
fn parse_direct_weights_into(data: &[u8], weights: &mut Vec<u8>) -> Result<usize, DecompressError> {
    let num_symbols = (data[0] as usize) - 127;
    let num_bytes = num_symbols.div_ceil(2);

    if data.len() < 1 + num_bytes {
        return Err(DecompressError::BadHuffmanWeights);
    }

    weights.clear();
    for i in 0..num_symbols {
        let byte_idx = 1 + i / 2;
        let weight = if i % 2 == 0 {
            data[byte_idx] >> 4
        } else {
            data[byte_idx] & 0x0F
        };
        weights.push(weight);
    }

    Ok(1 + num_bytes)
}

fn parse_fse_compressed_weights(data: &[u8]) -> Result<(Vec<u8>, usize), DecompressError> {
    let compressed_size = data[0] as usize;
    if compressed_size == 0 || data.len() < 1 + compressed_size {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let compressed = &data[1..=compressed_size];

    let mut bit_reader = BitReader::new(compressed);
    let (distribution, accuracy_log) =
        crate::fse::table_builder::parse_fse_table_description(&mut bit_reader, 12)?;
    if accuracy_log > 6 {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let table = build_decode_table(&distribution, accuracy_log)
        .map_err(|_| DecompressError::BadHuffmanWeights)?;

    let table_desc_bytes = bit_reader.bytes_consumed();
    let fse_stream = &compressed[table_desc_bytes..];

    if fse_stream.is_empty() {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let mut rev_reader =
        ReverseBitReader::new(fse_stream).map_err(|_| DecompressError::BadHuffmanWeights)?;

    let mut state1 = FseState::new(&table, accuracy_log, &mut rev_reader)
        .map_err(|_| DecompressError::BadHuffmanWeights)?;
    let mut state2 = FseState::new(&table, accuracy_log, &mut rev_reader)
        .map_err(|_| DecompressError::BadHuffmanWeights)?;

    let mut weights = Vec::new();

    loop {
        weights.push(state1.symbol());
        let nb1 = state1.num_bits();
        if nb1 > 0 && rev_reader.bits_remaining() < nb1 as usize {
            weights.push(state2.symbol());
            break;
        }
        state1
            .update_state(&mut rev_reader)
            .map_err(|_| DecompressError::BadHuffmanWeights)?;

        weights.push(state2.symbol());
        let nb2 = state2.num_bits();
        if nb2 > 0 && rev_reader.bits_remaining() < nb2 as usize {
            weights.push(state1.symbol());
            break;
        }
        state2
            .update_state(&mut rev_reader)
            .map_err(|_| DecompressError::BadHuffmanWeights)?;

        if weights.len() > 255 {
            return Err(DecompressError::BadHuffmanWeights);
        }
    }

    // The final step pushes up to two weights past the in-loop check. At
    // most 255 weights are explicit; the last one is implied.
    if weights.len() > 255 {
        return Err(DecompressError::BadHuffmanWeights);
    }

    Ok((weights, 1 + compressed_size))
}

#[cfg(feature = "alloc")]
fn parse_fse_compressed_weights_into(
    data: &[u8],
    weights: &mut Vec<u8>,
    fse_table: &mut Vec<crate::fse::FseDecodeEntry>,
    fse_symbol_next: &mut Vec<u16>,
    fse_dist: &mut Vec<i16>,
) -> Result<usize, DecompressError> {
    use crate::fse::table_builder::{build_decode_table_into, parse_fse_table_description_into};

    let compressed_size = data[0] as usize;
    if compressed_size == 0 || data.len() < 1 + compressed_size {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let compressed = &data[1..=compressed_size];

    let mut bit_reader = BitReader::new(compressed);
    let accuracy_log = parse_fse_table_description_into(&mut bit_reader, 12, fse_dist)?;
    if accuracy_log > 6 {
        return Err(DecompressError::BadHuffmanWeights);
    }

    build_decode_table_into(fse_dist, accuracy_log, fse_table, fse_symbol_next)
        .map_err(|_| DecompressError::BadHuffmanWeights)?;

    let table_desc_bytes = bit_reader.bytes_consumed();
    let fse_stream = &compressed[table_desc_bytes..];

    if fse_stream.is_empty() {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let mut rev_reader =
        ReverseBitReader::new(fse_stream).map_err(|_| DecompressError::BadHuffmanWeights)?;

    // The weight table has at most 64 entries (accuracy log <= 6). Padding
    // it to 64 lets `& 63` replace bounds checks on every lookup.
    fse_table.resize(
        64,
        crate::fse::FseDecodeEntry {
            base_line: 0,
            num_bits: 0,
            symbol: 0,
        },
    );
    let table: &[crate::fse::FseDecodeEntry; 64] = fse_table[..64].try_into().unwrap();
    let mut state1 = FseState::new(fse_table, accuracy_log, &mut rev_reader)
        .map_err(|_| DecompressError::BadHuffmanWeights)?
        .state() as usize;
    let mut state2 = FseState::new(fse_table, accuracy_log, &mut rev_reader)
        .map_err(|_| DecompressError::BadHuffmanWeights)?
        .state() as usize;

    // Up to 255 weights, plus up to three more from the step that detects
    // the overflow.
    let mut out = [0u8; 260];
    let mut n = 0usize;

    // Fast path: four steps read at most 24 bits (accuracy log <= 6). After a
    // refill, the container holds all remaining bits or at least 57, so with
    // 24 bits left none of the four steps can reach the end of the stream.
    while n <= 251 {
        rev_reader.refill_fast_or_regular();
        if rev_reader.bits_remaining() < 24 {
            break;
        }
        let e1 = table[state1 & 63];
        state1 = e1.base_line as usize + rev_reader.read_bits_branchless(e1.num_bits) as usize;
        let e2 = table[state2 & 63];
        state2 = e2.base_line as usize + rev_reader.read_bits_branchless(e2.num_bits) as usize;
        let e3 = table[state1 & 63];
        state1 = e3.base_line as usize + rev_reader.read_bits_branchless(e3.num_bits) as usize;
        let e4 = table[state2 & 63];
        state2 = e4.base_line as usize + rev_reader.read_bits_branchless(e4.num_bits) as usize;
        out[n..n + 4].copy_from_slice(&[e1.symbol, e2.symbol, e3.symbol, e4.symbol]);
        n += 4;
    }

    // Each step reads at most `accuracy_log` bits. After the remaining-bits
    // check, a refill leaves at least that many bits in the container, so the
    // unchecked read is exact.
    loop {
        let e1 = table[state1 & 63];
        out[n] = e1.symbol;
        n += 1;
        if e1.num_bits > 0 && rev_reader.bits_remaining() < e1.num_bits as usize {
            out[n] = table[state2 & 63].symbol;
            n += 1;
            break;
        }
        rev_reader.refill();
        state1 = e1.base_line as usize + rev_reader.read_bits_branchless(e1.num_bits) as usize;

        let e2 = table[state2 & 63];
        out[n] = e2.symbol;
        n += 1;
        if e2.num_bits > 0 && rev_reader.bits_remaining() < e2.num_bits as usize {
            out[n] = table[state1 & 63].symbol;
            n += 1;
            break;
        }
        rev_reader.refill();
        state2 = e2.base_line as usize + rev_reader.read_bits_branchless(e2.num_bits) as usize;

        if n > 255 {
            return Err(DecompressError::BadHuffmanWeights);
        }
    }

    // The final step pushes up to two weights past the in-loop check. At
    // most 255 weights are explicit; the last one is implied.
    if n > 255 {
        return Err(DecompressError::BadHuffmanWeights);
    }

    weights.clear();
    weights.extend_from_slice(&out[..n]);
    Ok(1 + compressed_size)
}

#[cfg(feature = "alloc")]
pub fn build_huffman_decode_table(
    weights: &[u8],
) -> Result<(Vec<crate::huffman::HuffmanDecodeEntry>, u8), DecompressError> {
    use crate::huffman::{HuffmanDecodeEntry, MAX_BITS};

    if weights.is_empty() || weights.len() > 255 {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let mut weight_sum: u32 = 0;
    let mut max_weight = 0u8;
    for &w in weights.iter() {
        max_weight = max_weight.max(w);
        if w > 0 {
            weight_sum += 1u32 << (w - 1);
        }
    }

    if max_weight > MAX_BITS + 1 {
        return Err(DecompressError::BadHuffmanWeights);
    }

    if weight_sum == 0 {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let table_log = high_bit(weight_sum) + 1;
    if table_log > MAX_BITS as u32 {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let total_capacity = 1u32 << table_log;
    let last_weight_value = total_capacity - weight_sum;
    if last_weight_value == 0 || !last_weight_value.is_power_of_two() {
        return Err(DecompressError::BadHuffmanWeights);
    }
    let last_weight = high_bit(last_weight_value) as u8 + 1;

    let mut all_weights: Vec<u8> = weights.to_vec();
    all_weights.push(last_weight);

    let mut table = vec![HuffmanDecodeEntry::default(); super::DECODE_TABLE_SIZE];

    let max_w = table_log as u8 + 1;

    let mut rank_count = vec![0u32; max_w as usize + 1];
    for &w in all_weights.iter() {
        if w > 0 && w <= max_w {
            rank_count[w as usize] += 1;
        }
    }

    let mut rank_start = vec![0u32; max_w as usize + 1];
    {
        let mut cumul = 0u32;
        for w in 1..=max_w {
            rank_start[w as usize] = cumul;
            cumul += rank_count[w as usize] * (1u32 << (w - 1));
        }
    }

    for (symbol, &w) in all_weights.iter().enumerate() {
        if w == 0 {
            continue;
        }
        let num_bits = (table_log as u8 + 1) - w;
        let entries = 1usize << (w - 1);
        let start = rank_start[w as usize] as usize;
        rank_start[w as usize] += entries as u32;
        let entry = HuffmanDecodeEntry {
            symbol: symbol as u8,
            num_bits,
        };
        table[start..start + entries].fill(entry);
    }

    Ok((table, table_log as u8))
}

/// Builds the decode table for `weights` into `table` and returns the table
/// log.
#[cfg(feature = "alloc")]
pub fn build_huffman_decode_table_into(
    weights: &[u8],
    table: &mut Vec<crate::huffman::HuffmanDecodeEntry>,
    all_weights: &mut Vec<u8>,
) -> Result<u8, DecompressError> {
    use crate::huffman::{HuffmanDecodeEntry, MAX_BITS};

    if weights.is_empty() || weights.len() > 255 {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let mut weight_sum: u32 = 0;
    let mut max_weight = 0u8;
    for &w in weights.iter() {
        max_weight = max_weight.max(w);
        if w > 0 {
            weight_sum += 1u32 << (w - 1);
        }
    }

    if max_weight > MAX_BITS + 1 {
        return Err(DecompressError::BadHuffmanWeights);
    }

    if weight_sum == 0 {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let table_log = high_bit(weight_sum) + 1;
    if table_log > MAX_BITS as u32 {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let total_capacity = 1u32 << table_log;
    let last_weight_value = total_capacity - weight_sum;
    if last_weight_value == 0 || !last_weight_value.is_power_of_two() {
        return Err(DecompressError::BadHuffmanWeights);
    }
    let last_weight = high_bit(last_weight_value) as u8 + 1;

    all_weights.clear();
    all_weights.extend_from_slice(weights);
    all_weights.push(last_weight);

    if table.len() != super::DECODE_TABLE_SIZE {
        table.clear();
        table.resize(super::DECODE_TABLE_SIZE, HuffmanDecodeEntry::default());
    }

    let max_w = table_log as usize + 1;

    // Every weight is at most `table_log`, because `weight_sum` includes its
    // `1 << (w - 1)`. So `w & 15` is `w` and only drops the bounds check.
    // Zero weights go to bucket 0 instead of being skipped: zero and nonzero
    // weights interleave unpredictably, so a branch per symbol mispredicts.
    //
    // The histogram and the counting sort below run on four contiguous
    // partitions of the symbols, each with its own counters. With one shared
    // counter array, consecutive symbols of the same weight waited on each
    // other's counter store. Contiguous partitions keep symbols of equal
    // weight in increasing order, as the canonical code needs. Padding
    // symbols get weight 0 and land in bucket 0, which the fill skips.
    let q = all_weights.len().div_ceil(4);
    all_weights.resize(4 * q, 0);
    let aw = &all_weights[..];
    let mut hist = [[0u32; 16]; 4];
    for i in 0..q {
        for (k, h) in hist.iter_mut().enumerate() {
            h[(aw[k * q + i] & 15) as usize] += 1;
        }
    }
    let mut rank_count = [0u32; 16];
    for (w, count) in rank_count.iter_mut().enumerate() {
        *count = hist[0][w] + hist[1][w] + hist[2][w] + hist[3][w];
    }

    // Group symbols by weight (counting sort), weight 0 first, and within a
    // weight by partition. Each nonzero weight then fills one contiguous range
    // with a fixed number of entries per symbol, which compiles to fixed-width
    // stores instead of a variable-length fill per symbol. There are at most
    // 256 symbols, so `& 255` only drops the bounds check.
    let mut next = [[0u32; 16]; 4];
    let mut acc = 0u32;
    for w in 0..16 {
        for k in 0..4 {
            next[k][w] = acc;
            acc += hist[k][w];
        }
    }
    let mut sorted = [0u8; 256];
    for i in 0..q {
        for (k, slots) in next.iter_mut().enumerate() {
            let symbol = k * q + i;
            let slot = &mut slots[(aw[symbol] & 15) as usize];
            sorted[*slot as usize & 255] = symbol as u8;
            *slot += 1;
        }
    }
    let mut first = rank_count[0] as usize;
    let mut start = 0usize;
    for (w, &count) in rank_count.iter().enumerate().take(max_w + 1).skip(1) {
        let count = count as usize;
        let symbols = &sorted[first..first + count];
        first += count;
        let num_bits = (max_w - w) as u8;
        match w {
            1 => fill_runs::<1>(table, start, symbols, num_bits),
            2 => fill_runs::<2>(table, start, symbols, num_bits),
            3 => fill_runs::<4>(table, start, symbols, num_bits),
            4 => fill_runs::<8>(table, start, symbols, num_bits),
            5 => fill_runs::<16>(table, start, symbols, num_bits),
            _ => {
                let entries = 1usize << (w - 1);
                for (i, &symbol) in symbols.iter().enumerate() {
                    let pos = start + i * entries;
                    table[pos..pos + entries].fill(HuffmanDecodeEntry { symbol, num_bits });
                }
            }
        }
        start += count << (w - 1);
    }

    Ok(table_log as u8)
}

/// Writes `L` copies of each symbol's entry, one symbol after another,
/// starting at `start`.
#[inline(always)]
fn fill_runs<const L: usize>(
    table: &mut [crate::huffman::HuffmanDecodeEntry],
    start: usize,
    symbols: &[u8],
    num_bits: u8,
) {
    let run = &mut table[start..start + symbols.len() * L];
    for (chunk, &symbol) in run.as_chunks_mut::<L>().0.iter_mut().zip(symbols) {
        *chunk = [crate::huffman::HuffmanDecodeEntry { symbol, num_bits }; L];
    }
}

fn high_bit(val: u32) -> u32 {
    debug_assert!(val > 0);
    31 - val.leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_direct_2_symbols() {
        // header = 127 + 2 = 129
        let data = [129, 0x42];
        let (weights, consumed) = parse_huffman_weights(&data).unwrap();
        assert_eq!(weights.len(), 2);
        assert_eq!(weights[0], 4);
        assert_eq!(weights[1], 2);
        assert_eq!(consumed, 2);
    }

    #[test]
    fn parse_direct_3_symbols() {
        // header = 127 + 3 = 130
        let data = [130, 0x31, 0x20];
        let (weights, consumed) = parse_huffman_weights(&data).unwrap();
        assert_eq!(weights.len(), 3);
        assert_eq!(weights[0], 3);
        assert_eq!(weights[1], 1);
        assert_eq!(weights[2], 2);
        assert_eq!(consumed, 3);
    }

    #[test]
    fn build_table_two_symbols() {
        // weight 2 means 2^1=2 slots. Sum=2. Implied symbol needs 2 slots too.
        // Total = 4 = 2^2, so table_log = 2.
        let weights = vec![2];
        let (table, table_log) = build_huffman_decode_table(&weights).unwrap();
        assert_eq!(table_log, 2);
        assert_eq!(table.len(), crate::huffman::DECODE_TABLE_SIZE);
        let sym0_count = table[..4].iter().filter(|e| e.symbol == 0).count();
        let sym1_count = table[..4].iter().filter(|e| e.symbol == 1).count();
        assert_eq!(sym0_count, 2);
        assert_eq!(sym1_count, 2);
    }

    /// FSE-compressed weight header whose stream decodes to `count` weights.
    /// With two symbols at 16/32, every state reads one bit: two 5-bit
    /// initial states, then one bit per weight except the last two.
    fn fse_weights_header(count: usize) -> Vec<u8> {
        use crate::bitstream::writer::BitWriter;
        use crate::fse::table_builder::serialize_fse_table_description;

        let mut compressed = serialize_fse_table_description(&[16, 16], 5);
        let mut stream = BitWriter::new();
        for _ in 0..10 + count - 2 {
            stream.write_bits(0, 1);
        }
        stream.close_reverse_stream();
        compressed.extend_from_slice(&stream.into_bytes());
        let mut data = vec![compressed.len() as u8];
        data.extend_from_slice(&compressed);
        data
    }

    #[test]
    fn parse_fse_weights_limit() {
        let data = fse_weights_header(255);
        let (weights, _) = parse_huffman_weights(&data).unwrap();
        assert_eq!(weights.len(), 255);
        let mut out = Vec::new();
        parse_huffman_weights_into(
            &data,
            &mut out,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(out.len(), 255);

        for count in [256, 257] {
            let data = fse_weights_header(count);
            assert!(parse_huffman_weights(&data).is_err(), "{count} weights");
            let result = parse_huffman_weights_into(
                &data,
                &mut out,
                &mut Vec::new(),
                &mut Vec::new(),
                &mut Vec::new(),
            );
            assert_eq!(
                result,
                Err(DecompressError::BadHuffmanWeights),
                "{count} weights"
            );
        }
    }

    #[test]
    fn parse_fse_weights_into_matches_reference() {
        use crate::fse::table_builder::{normalize_counts, serialize_fse_table_description};

        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let (mut out, mut table, mut next, mut dist) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (mut ok, mut too_many) = (0, 0);
        for _ in 0..20_000 {
            let accuracy_log = 5 + (rand() % 2) as u8;
            let symbols = 2 + (rand() % 12) as usize;
            // A dominant symbol gives zero- and one-bit states, so some
            // streams decode more than 255 weights.
            let freqs: Vec<u32> = (0..symbols)
                .map(|_| {
                    if rand() % 4 == 0 {
                        1000
                    } else {
                        (rand() % 50) as u32 + 1
                    }
                })
                .collect();
            let mut compressed = serialize_fse_table_description(
                &normalize_counts(&freqs, accuracy_log),
                accuracy_log,
            );
            let stream_len = 1 + (rand() % 100) as usize;
            compressed.extend((0..stream_len).map(|_| rand() as u8));
            *compressed.last_mut().unwrap() |= 1 << (rand() % 8);
            if compressed.len() > 127 {
                continue;
            }
            let mut data = vec![compressed.len() as u8];
            data.extend_from_slice(&compressed);

            let reference = parse_huffman_weights(&data);
            let result =
                parse_huffman_weights_into(&data, &mut out, &mut table, &mut next, &mut dist);
            match (reference, result) {
                (Ok((weights, consumed)), Ok(result_consumed)) => {
                    assert_eq!(out, weights);
                    assert_eq!(result_consumed, consumed);
                    ok += 1;
                }
                (Err(e), Err(result_e)) => {
                    assert_eq!(result_e, e);
                    too_many += usize::from(e == DecompressError::BadHuffmanWeights);
                }
                (reference, result) => panic!("{reference:?} vs {result:?} for {data:?}"),
            }
        }
        assert!(ok > 1000 && too_many > 100, "ok {ok}, too many {too_many}");
    }

    #[test]
    fn build_table_into_matches_reference() {
        let mut seed = 0x7a3d_5c19_e0b4_8f21u64;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let (mut table, mut all) = (Vec::new(), Vec::new());
        for _ in 0..3000 {
            // A random complete prefix code with 2 to 256 leaves, at most 11
            // bits deep, over random symbols. The highest symbol's weight is
            // implied.
            let leaves = 2 + (rand() as usize) % 255;
            let mut depths = vec![1u8, 1];
            while depths.len() < leaves {
                let i = (rand() as usize) % depths.len();
                if depths[i] < 11 {
                    depths[i] += 1;
                    depths.push(depths[i]);
                }
            }
            let table_log = *depths.iter().max().unwrap();
            let last_symbol = leaves - 1 + (rand() as usize) % (256 - leaves + 1);
            let mut symbols: Vec<usize> = (0..last_symbol).collect();
            for i in (1..symbols.len()).rev() {
                symbols.swap(i, (rand() as usize) % (i + 1));
            }
            let mut weights = vec![0u8; last_symbol];
            for (&symbol, &depth) in symbols[..leaves - 1].iter().zip(&depths) {
                weights[symbol] = table_log + 1 - depth;
            }

            let (expected, expected_log) = build_huffman_decode_table(&weights).unwrap();
            let log = build_huffman_decode_table_into(&weights, &mut table, &mut all).unwrap();
            assert_eq!(log, expected_log);
            let n = 1 << log;
            let fields = |t: &[crate::huffman::HuffmanDecodeEntry]| -> Vec<(u8, u8)> {
                t[..n].iter().map(|e| (e.symbol, e.num_bits)).collect()
            };
            assert_eq!(fields(&table), fields(&expected), "{weights:?}");
        }
    }

    #[test]
    fn build_table_rejects_more_than_255_weights() {
        // 256 weight-1 symbols and one weight-8 symbol sum to 384. The
        // implied last symbol (weight 8) completes a 512-entry table, but
        // the alphabet would have 258 symbols.
        let mut weights = vec![1u8; 256];
        weights.push(8);
        let mut table = Vec::new();
        let result = build_huffman_decode_table_into(&weights, &mut table, &mut Vec::new());
        assert_eq!(result, Err(DecompressError::BadHuffmanWeights));
    }
}
