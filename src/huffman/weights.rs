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

fn parse_direct_weights(data: &[u8]) -> Result<(Vec<u8>, usize), DecompressError> {
    let num_symbols = (data[0] as usize) - 127;
    let num_bytes = (num_symbols + 1) / 2;

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

fn parse_fse_compressed_weights(data: &[u8]) -> Result<(Vec<u8>, usize), DecompressError> {
    let compressed_size = data[0] as usize;
    if compressed_size == 0 || data.len() < 1 + compressed_size {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let compressed = &data[1..1 + compressed_size];

    let mut bit_reader = BitReader::new(compressed);
    let (distribution, accuracy_log) =
        crate::fse::table_builder::parse_fse_table_description(&mut bit_reader, 12)?;

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

    Ok((weights, 1 + compressed_size))
}

#[cfg(feature = "alloc")]
pub fn build_huffman_decode_table(
    weights: &[u8],
) -> Result<(Vec<crate::huffman::HuffmanDecodeEntry>, u8), DecompressError> {
    use crate::huffman::{HuffmanDecodeEntry, MAX_BITS};

    if weights.is_empty() {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let max_weight = *weights.iter().max().unwrap();
    if max_weight > MAX_BITS + 1 {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let mut weight_sum: u32 = 0;
    for &w in weights.iter() {
        if w > 0 {
            weight_sum += 1u32 << (w - 1);
        }
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

    let table_size = 1usize << table_log;
    let mut table = vec![HuffmanDecodeEntry::default(); table_size];

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
        for j in 0..entries {
            table[start + j] = HuffmanDecodeEntry {
                symbol: symbol as u8,
                num_bits,
            };
        }
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

    if weights.is_empty() {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let max_weight = *weights.iter().max().unwrap();
    if max_weight > MAX_BITS + 1 {
        return Err(DecompressError::BadHuffmanWeights);
    }

    let mut weight_sum: u32 = 0;
    for &w in weights.iter() {
        if w > 0 {
            weight_sum += 1u32 << (w - 1);
        }
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

    let table_size = 1usize << table_log;
    table.clear();
    table.resize(table_size, HuffmanDecodeEntry::default());

    let max_w = table_log as u8 + 1;

    rank_count.clear();
    rank_count.resize(max_w as usize + 1, 0);
    for &w in all_weights.iter() {
        if w > 0 && w <= max_w {
            rank_count[w as usize] += 1;
        }
    }

    rank_start.clear();
    rank_start.resize(max_w as usize + 1, 0);
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
        for j in 0..entries {
            table[start + j] = HuffmanDecodeEntry {
                symbol: symbol as u8,
                num_bits,
            };
        }
    }

    Ok(table_log as u8)
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
        assert_eq!(table.len(), 4);
        let sym0_count = table.iter().filter(|e| e.symbol == 0).count();
        let sym1_count = table.iter().filter(|e| e.symbol == 1).count();
        assert_eq!(sym0_count, 2);
        assert_eq!(sym1_count, 2);
    }
}
