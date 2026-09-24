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

    weights.clear();

    // Each step reads at most `accuracy_log` bits. After the remaining-bits
    // check, a refill leaves at least that many bits in the container, so the
    // unchecked read is exact.
    loop {
        let e1 = table[state1 & 63];
        weights.push(e1.symbol);
        if e1.num_bits > 0 && rev_reader.bits_remaining() < e1.num_bits as usize {
            weights.push(table[state2 & 63].symbol);
            break;
        }
        rev_reader.refill();
        state1 = e1.base_line as usize + rev_reader.read_bits_branchless(e1.num_bits) as usize;

        let e2 = table[state2 & 63];
        weights.push(e2.symbol);
        if e2.num_bits > 0 && rev_reader.bits_remaining() < e2.num_bits as usize {
            weights.push(table[state1 & 63].symbol);
            break;
        }
        rev_reader.refill();
        state2 = e2.base_line as usize + rev_reader.read_bits_branchless(e2.num_bits) as usize;

        if weights.len() > 255 {
            return Err(DecompressError::BadHuffmanWeights);
        }
    }

    // The final step pushes up to two weights past the in-loop check. At
    // most 255 weights are explicit; the last one is implied.
    if weights.len() > 255 {
        return Err(DecompressError::BadHuffmanWeights);
    }

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

#[cfg(feature = "alloc")]
pub fn build_huffman_decode_table_into(
    weights: &[u8],
    table: &mut Vec<crate::huffman::HuffmanDecodeEntry>,
    all_weights: &mut Vec<u8>,
    rank_count: &mut Vec<u32>,
    rank_start: &mut Vec<u32>,
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

    let max_w = table_log as u8 + 1;

    rank_count.resize(max_w as usize + 1, 0);
    rank_count.fill(0);
    for &w in all_weights.iter() {
        if w > 0 && w <= max_w {
            rank_count[w as usize] += 1;
        }
    }

    rank_start.resize(max_w as usize + 1, 0);
    {
        let mut cumul = 0u32;
        for w in 1..=max_w {
            rank_start[w as usize] = cumul;
            cumul += rank_count[w as usize] * (1u32 << (w - 1));
        }
    }

    // Group symbols by weight (counting sort). Each weight then fills one
    // contiguous range with a fixed number of entries per symbol, which
    // compiles to fixed-width stores instead of a variable-length fill per
    // symbol.
    let mut sorted = [0u8; 256];
    let mut next = [0usize; MAX_BITS as usize + 2];
    let mut acc = 0usize;
    for w in 1..=max_w as usize {
        next[w] = acc;
        acc += rank_count[w] as usize;
    }
    for (symbol, &w) in all_weights.iter().enumerate() {
        if w > 0 {
            sorted[next[w as usize]] = symbol as u8;
            next[w as usize] += 1;
        }
    }
    let mut first = 0usize;
    for w in 1..=max_w {
        let count = rank_count[w as usize] as usize;
        let symbols = &sorted[first..first + count];
        first += count;
        let num_bits = (table_log as u8 + 1) - w;
        let start = rank_start[w as usize] as usize;
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
    fn build_table_rejects_more_than_255_weights() {
        // 256 weight-1 symbols and one weight-8 symbol sum to 384. The
        // implied last symbol (weight 8) completes a 512-entry table, but
        // the alphabet would have 258 symbols.
        let mut weights = vec![1u8; 256];
        weights.push(8);
        let mut table = Vec::new();
        let result = build_huffman_decode_table_into(
            &weights,
            &mut table,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        );
        assert_eq!(result, Err(DecompressError::BadHuffmanWeights));
    }
}
