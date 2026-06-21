#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::bitstream::reader::BitReader;
use crate::error::DecompressError;
use crate::fse::{FseDecodeEntry, MAX_TABLE_LOG};
use crate::hint::unlikely;

pub fn parse_fse_table_description_into(
    reader: &mut BitReader,
    max_symbol: u8,
    distribution: &mut Vec<i16>,
) -> Result<u8, DecompressError> {
    let accuracy_log = reader.read_bits(4)? as u8 + 5;
    if accuracy_log > MAX_TABLE_LOG {
        return Err(DecompressError::BadFseTable);
    }

    let table_size = 1i32 << accuracy_log;
    let mut remaining = table_size + 1;
    let mut threshold = table_size;
    let mut nb_bits = accuracy_log + 1;
    distribution.clear();

    while remaining > 1 && distribution.len() <= max_symbol as usize {
        let max_val = (2 * threshold - 1) - remaining;

        let lower = reader.read_bits(nb_bits - 1)? as i32;
        let count = if lower < max_val {
            lower
        } else {
            let extra = reader.read_bits(1)? as i32;
            let full = lower + (extra << (nb_bits - 1));
            if full >= threshold {
                full - max_val
            } else {
                full
            }
        };

        let prob = count - 1;
        if prob == -1 {
            distribution.push(-1);
            remaining -= 1;
        } else if prob == 0 {
            distribution.push(0);
        } else {
            distribution.push(prob as i16);
            remaining -= prob;
        }

        if remaining < 0 {
            return Err(DecompressError::BadFseTable);
        }

        if prob == 0 {
            loop {
                let repeat = reader.read_bits(2)? as usize;
                distribution.extend(core::iter::repeat_n(0, repeat));
                if repeat < 3 {
                    break;
                }
            }
        }

        while remaining < threshold {
            nb_bits -= 1;
            threshold >>= 1;
        }
    }

    if remaining != 1 {
        return Err(DecompressError::BadFseTable);
    }

    reader.align_to_byte();

    while distribution.len() <= max_symbol as usize {
        distribution.push(0);
    }

    Ok(accuracy_log)
}

pub fn parse_fse_table_description(
    reader: &mut BitReader,
    max_symbol: u8,
) -> Result<(Vec<i16>, u8), DecompressError> {
    let mut distribution = Vec::new();
    let accuracy_log = parse_fse_table_description_into(reader, max_symbol, &mut distribution)?;
    Ok((distribution, accuracy_log))
}

pub fn serialize_fse_table_description(distribution: &[i16], accuracy_log: u8) -> Vec<u8> {
    use crate::bitstream::writer::BitWriter;

    let mut writer = BitWriter::new();
    writer.write_bits((accuracy_log - 5) as u32, 4);

    let table_size = 1i32 << accuracy_log;
    let mut remaining = table_size + 1;
    let mut threshold = table_size;
    let mut nb_bits = accuracy_log + 1;

    let mut i = 0;
    while i < distribution.len() && remaining > 1 {
        let prob = distribution[i];
        let count = (prob + 1) as i32;
        let max_val = (2 * threshold - 1) - remaining;

        if count < max_val {
            writer.write_bits(count as u32, nb_bits - 1);
        } else if count < threshold {
            writer.write_bits(count as u32, nb_bits);
        } else {
            writer.write_bits((count + max_val) as u32, nb_bits);
        }

        if prob == -1 {
            remaining -= 1;
        } else if prob > 0 {
            remaining -= prob as i32;
        }

        if prob == 0 {
            let mut zeros = 0;
            let start = i + 1;
            while start + zeros < distribution.len() && distribution[start + zeros] == 0 {
                zeros += 1;
            }
            let mut z = zeros;
            loop {
                if z >= 3 {
                    writer.write_bits(3, 2);
                    z -= 3;
                } else {
                    writer.write_bits(z as u32, 2);
                    break;
                }
            }
            i += 1 + zeros;
        } else {
            i += 1;
        }

        while remaining < threshold {
            nb_bits -= 1;
            threshold >>= 1;
        }
    }

    writer.flush_remaining();
    writer.as_bytes().to_vec()
}

pub fn normalize_counts(freqs: &[u32], accuracy_log: u8) -> Vec<i16> {
    let table_size = 1i32 << accuracy_log;
    let total: u64 = freqs.iter().map(|&f| f as u64).sum();

    if total == 0 {
        return vec![0; freqs.len()];
    }

    let mut dist = vec![0i16; freqs.len()];
    let mut largest_idx = 0;
    let mut largest_freq = 0u32;
    for (i, &freq) in freqs.iter().enumerate() {
        if freq > largest_freq {
            largest_freq = freq;
            largest_idx = i;
        }
    }

    let budget = table_size - 1;
    let mut distributed = 0i32;

    for (i, &freq) in freqs.iter().enumerate() {
        if freq == 0 || i == largest_idx {
            continue;
        }
        if distributed >= budget {
            break;
        }
        let prob = ((freq as u64) * (table_size as u64) / total) as i32;
        if prob < 1 {
            dist[i] = -1;
            distributed += 1;
        } else {
            let capped = prob.min(budget - distributed);
            dist[i] = capped as i16;
            distributed += capped;
        }
    }

    dist[largest_idx] = (table_size - distributed) as i16;

    dist
}

pub fn build_decode_table(
    distribution: &[i16],
    accuracy_log: u8,
) -> Result<Vec<FseDecodeEntry>, DecompressError> {
    let mut table = Vec::new();
    let mut symbol_next = Vec::new();
    build_decode_table_into(distribution, accuracy_log, &mut table, &mut symbol_next)?;
    Ok(table)
}

pub fn build_decode_table_into(
    distribution: &[i16],
    accuracy_log: u8,
    table: &mut Vec<FseDecodeEntry>,
    symbol_next: &mut Vec<u16>,
) -> Result<(), DecompressError> {
    let table_size = 1usize << accuracy_log;
    table.clear();
    table.resize(
        table_size,
        FseDecodeEntry {
            base_line: 0,
            num_bits: 0,
            symbol: 0,
        },
    );

    let step = (table_size >> 1) + (table_size >> 3) + 3;
    let mask = table_size - 1;

    let mut high_threshold = table_size - 1;
    symbol_next.clear();
    symbol_next.resize(distribution.len(), 0);

    for (s, &prob) in distribution.iter().enumerate() {
        if prob == -1 {
            if unlikely(high_threshold == 0) {
                return Err(DecompressError::BadFseTable);
            }
            table[high_threshold].symbol = s as u8;
            high_threshold -= 1;
            symbol_next[s] = 1;
        } else if prob > 0 {
            symbol_next[s] = prob as u16;
        }
    }

    let mut position = 0;
    for (s, &prob) in distribution.iter().enumerate() {
        if prob <= 0 {
            continue;
        }
        for _ in 0..prob {
            table[position].symbol = s as u8;
            position = (position + step) & mask;
            while position > high_threshold {
                position = (position + step) & mask;
            }
        }
    }

    if position != 0 {
        return Err(DecompressError::BadFseTable);
    }

    for entry in table.iter_mut().take(table_size) {
        let s = entry.symbol as usize;
        let next_state = symbol_next[s] as u32;
        symbol_next[s] += 1;

        let nb = accuracy_log as u32 - high_bit(next_state);
        let new_state = (next_state << nb) - table_size as u32;
        entry.num_bits = nb as u8;
        entry.base_line = new_state as u16;
    }

    Ok(())
}

pub fn build_decode_table_from_default(
    default_dist: &[i16],
    accuracy_log: u8,
) -> Vec<FseDecodeEntry> {
    build_decode_table(default_dist, accuracy_log)
        .expect("predefined FSE table distributions are always valid")
}

fn high_bit(val: u32) -> u32 {
    debug_assert!(val > 0);
    31 - val.leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fse::{
        LL_DEFAULT_ACCURACY, LL_DEFAULT_DIST, ML_DEFAULT_ACCURACY, ML_DEFAULT_DIST,
        OF_DEFAULT_ACCURACY, OF_DEFAULT_DIST,
    };

    #[test]
    fn build_ll_default_table() {
        let table = build_decode_table_from_default(&LL_DEFAULT_DIST, LL_DEFAULT_ACCURACY);
        assert_eq!(table.len(), 1 << LL_DEFAULT_ACCURACY);
        let sym_counts: usize = table.iter().map(|_| 1).sum();
        assert_eq!(sym_counts, 64);
    }

    #[test]
    fn build_ml_default_table() {
        let table = build_decode_table_from_default(&ML_DEFAULT_DIST, ML_DEFAULT_ACCURACY);
        assert_eq!(table.len(), 1 << ML_DEFAULT_ACCURACY);
    }

    #[test]
    fn build_of_default_table() {
        let table = build_decode_table_from_default(&OF_DEFAULT_DIST, OF_DEFAULT_ACCURACY);
        assert_eq!(table.len(), 1 << OF_DEFAULT_ACCURACY);
    }

    #[test]
    fn spread_function_no_collision() {
        // accuracy_log=5, table_size=32, step=23 (coprime to 32)
        let dist: Vec<i16> = vec![8, 8, 8, 8];
        let table = build_decode_table(&dist, 5).unwrap();
        assert_eq!(table.len(), 32);
        for i in 0..4u8 {
            let count = table.iter().filter(|e| e.symbol == i).count();
            assert_eq!(count, 8);
        }
    }

    #[test]
    fn less_than_one_prob() {
        // accuracy_log=5, table_size=32. Sum: 1+15+8+4+3=31, plus 1 for -1 = 32
        let dist: Vec<i16> = vec![-1, 15, 8, 4, 4];
        let table = build_decode_table(&dist, 5).unwrap();
        assert_eq!(table.len(), 32);
        let count_sym0 = table.iter().filter(|e| e.symbol == 0).count();
        assert_eq!(count_sym0, 1);
    }

    #[test]
    fn fse_table_description_roundtrip_ll_default() {
        let bytes = serialize_fse_table_description(&LL_DEFAULT_DIST, LL_DEFAULT_ACCURACY);
        let mut reader = BitReader::new(&bytes);
        let (dist, acc) = parse_fse_table_description(&mut reader, 35).unwrap();
        assert_eq!(acc, LL_DEFAULT_ACCURACY);
        assert_eq!(&dist[..LL_DEFAULT_DIST.len()], &LL_DEFAULT_DIST[..]);
    }

    #[test]
    fn fse_table_description_roundtrip_ml_default() {
        let bytes = serialize_fse_table_description(&ML_DEFAULT_DIST, ML_DEFAULT_ACCURACY);
        let mut reader = BitReader::new(&bytes);
        let (dist, acc) = parse_fse_table_description(&mut reader, 52).unwrap();
        assert_eq!(acc, ML_DEFAULT_ACCURACY);
        assert_eq!(&dist[..ML_DEFAULT_DIST.len()], &ML_DEFAULT_DIST[..]);
    }

    #[test]
    fn fse_table_description_roundtrip_of_default() {
        let bytes = serialize_fse_table_description(&OF_DEFAULT_DIST, OF_DEFAULT_ACCURACY);
        let mut reader = BitReader::new(&bytes);
        let (dist, acc) = parse_fse_table_description(&mut reader, 31).unwrap();
        assert_eq!(acc, OF_DEFAULT_ACCURACY);
        assert_eq!(&dist[..OF_DEFAULT_DIST.len()], &OF_DEFAULT_DIST[..]);
    }

    #[test]
    fn fse_table_description_roundtrip_uniform() {
        let dist: Vec<i16> = vec![8, 8, 8, 8];
        let bytes = serialize_fse_table_description(&dist, 5);
        let mut reader = BitReader::new(&bytes);
        let (parsed, acc) = parse_fse_table_description(&mut reader, 3).unwrap();
        assert_eq!(acc, 5);
        assert_eq!(&parsed[..4], &dist[..]);
    }

    #[test]
    fn fse_table_description_roundtrip_skewed() {
        let dist: Vec<i16> = vec![28, 1, 1, 1, 1];
        let bytes = serialize_fse_table_description(&dist, 5);
        let mut reader = BitReader::new(&bytes);
        let (parsed, acc) = parse_fse_table_description(&mut reader, 4).unwrap();
        assert_eq!(acc, 5);
        assert_eq!(&parsed[..5], &dist[..]);
    }

    #[test]
    fn fse_table_description_roundtrip_with_minus_one() {
        let dist: Vec<i16> = vec![-1, 15, 8, 4, 4];
        let bytes = serialize_fse_table_description(&dist, 5);
        let mut reader = BitReader::new(&bytes);
        let (parsed, acc) = parse_fse_table_description(&mut reader, 4).unwrap();
        assert_eq!(acc, 5);
        assert_eq!(&parsed[..5], &dist[..]);
    }

    #[test]
    fn fse_table_description_roundtrip_with_zeros() {
        let dist: Vec<i16> = vec![10, 0, 0, 0, 10, 0, 12];
        let bytes = serialize_fse_table_description(&dist, 5);
        let mut reader = BitReader::new(&bytes);
        let (parsed, acc) = parse_fse_table_description(&mut reader, 6).unwrap();
        assert_eq!(acc, 5);
        assert_eq!(&parsed[..7], &dist[..]);
    }

    #[test]
    fn fse_table_description_roundtrip_many_zeros() {
        let mut dist = vec![0i16; 30];
        dist[0] = 16;
        dist[29] = 16;
        let bytes = serialize_fse_table_description(&dist, 5);
        let mut reader = BitReader::new(&bytes);
        let (parsed, acc) = parse_fse_table_description(&mut reader, 29).unwrap();
        assert_eq!(acc, 5);
        assert_eq!(&parsed[..30], &dist[..]);
    }
}
