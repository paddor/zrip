#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::bitstream::reader::BitReader;
use crate::error::DecompressError;
use crate::fse::{FseDecodeEntry, MAX_TABLE_LOG};
use crate::hint::unlikely;

/// Largest table the fast spread in `build_decode_table_into` handles. It
/// covers the sequence tables (accuracy log <= 9) and Huffman weights.
const SPREAD_MAX: usize = 1 << 9;

pub fn parse_fse_table_description_into(
    reader: &mut BitReader,
    max_symbol: u8,
    distribution: &mut Vec<i16>,
) -> Result<u8, DecompressError> {
    // Each field comes from a 64-bit window at `bit` instead of a
    // `read_bits` call. `bit > end` after a field means the input ran out
    // inside it, where `read_bits` would have failed.
    let end = reader.bits_consumed() + reader.bits_remaining();
    let mut bit = reader.bits_consumed();
    if bit + 4 > end {
        return Err(DecompressError::InputExhausted);
    }
    let accuracy_log = (reader.window_at(bit) & 15) as u8 + 5;
    bit += 4;
    if accuracy_log > MAX_TABLE_LOG {
        return Err(DecompressError::BadFseTable);
    }

    let table_size = 1i32 << accuracy_log;
    let mut remaining = table_size + 1;
    // `threshold` is always `1 << (nb_bits - 1)`.
    let mut threshold = table_size;
    let mut nb_bits = accuracy_log as usize + 1;
    // Probabilities go to a zeroed stack buffer, so a run of zero
    // probabilities only advances `n`. `n` is capped at `max_len`: reaching
    // it ends the loop, and a run past it fails (`remaining` stays above 1).
    let max_len = max_symbol as usize + 1;
    let mut out = [0i16; 256];
    let mut n = 0usize;

    while remaining > 1 && n < max_len {
        // A count uses `nb_bits - 1` bits when those are below `max_val`,
        // else `nb_bits` bits, with the high values shifted down by
        // `max_val`. Both candidates are computed so the choice can compile
        // to selects.
        let window = reader.window_at(bit);
        let max_val = (2 * threshold - 1) - remaining;
        let lower = window as i32 & (threshold - 1);
        let full = window as i32 & (2 * threshold - 1);
        let long = lower >= max_val;
        let count = if !long {
            lower
        } else if full >= threshold {
            full - max_val
        } else {
            full
        };
        let used = nb_bits - 1 + usize::from(long);
        bit += used;
        if bit > end {
            return Err(DecompressError::InputExhausted);
        }

        // "Less than one" (-1) takes one table cell.
        let prob = count - 1;
        out[n & 255] = prob as i16;
        n += 1;
        remaining -= prob.abs();
        if remaining < 0 {
            return Err(DecompressError::BadFseTable);
        }

        if prob == 0 {
            // A zero is followed by 2-bit repeat codes: each `3` adds three
            // zeros and continues, the first code below 3 adds that many and
            // ends the run. Count the `3` codes at once. After the count, at
            // least 44 bits of the window are left, enough for 16 codes.
            let mut run = window >> used;
            loop {
                let threes = (!run).trailing_zeros() / 2;
                if threes < 16 {
                    let last = ((run >> (2 * threes)) & 3) as usize;
                    bit += 2 * threes as usize + 2;
                    if bit > end {
                        return Err(DecompressError::InputExhausted);
                    }
                    n = (n + 3 * threes as usize + last).min(max_len);
                    break;
                }
                bit += 32;
                if bit > end {
                    return Err(DecompressError::InputExhausted);
                }
                n = (n + 48).min(max_len);
                run = reader.window_at(bit);
            }
        }

        if remaining < threshold && remaining > 1 {
            let high = 31 - (remaining as u32).leading_zeros();
            nb_bits = high as usize + 1;
            threshold = 1 << high;
        }
    }

    if remaining != 1 {
        return Err(DecompressError::BadFseTable);
    }

    reader.seek(bit);
    reader.align_to_byte();
    distribution.clear();
    distribution.extend_from_slice(&out[..max_len]);
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
    if table.len() != table_size {
        table.clear();
        table.resize(
            table_size,
            FseDecodeEntry {
                base_line: 0,
                num_bits: 0,
                symbol: 0,
            },
        );
    }

    if distribution.len() > 256 {
        return Err(DecompressError::BadFseTable);
    }
    // `table` has exactly `table_size` entries, so indices masked with
    // `len - 1` need no bounds checks. Symbols are `u8`, so `next` indexed by
    // symbol needs none either.
    let table = &mut table[..table_size];
    let mask = table.len() - 1;
    let step = (table_size >> 1) + (table_size >> 3) + 3;
    let mut next = [0u16; 256];
    symbol_next.clear();

    // Zero and positive probabilities mix unpredictably, so `next` is set
    // without a branch. "Less than one" (-1) symbols are rare and take the
    // branch.
    let mut high_threshold = mask;
    let mut total = 0i32;
    for (s, &prob) in distribution.iter().enumerate() {
        next[s & 255] = prob.max(0) as u16 + u16::from(prob == -1);
        total += i32::from(prob.max(0));
        if unlikely(prob == -1) {
            if unlikely(high_threshold == 0) {
                return Err(DecompressError::BadFseTable);
            }
            table[high_threshold & mask].symbol = s as u8;
            high_threshold -= 1;
        }
    }

    let mut position = 0;
    if high_threshold == mask
        && (32..=SPREAD_MAX).contains(&table_size)
        && total == table_size as i32
    {
        // C zstd's fast spread: lay the symbols down in order, eight bytes
        // per write, then scatter them with a loop that has no data-dependent
        // branches. A variable-length loop per symbol mispredicts once per
        // symbol. Most counts in small tables are at most 8, so the inner
        // loop rarely runs. `total == table_size` keeps every write inside
        // `spread`.
        let mut spread = [0u8; SPREAD_MAX + 8];
        let mut pos = 0usize;
        for (s, &prob) in distribution.iter().enumerate() {
            let bytes = [s as u8; 8];
            let n = prob.max(0) as usize;
            spread[pos..pos + 8].copy_from_slice(&bytes);
            let mut i = 8;
            while i < n {
                spread[pos + i..pos + i + 8].copy_from_slice(&bytes);
                i += 8;
            }
            pos += n;
        }
        for &[a, b] in spread[..table_size].as_chunks::<2>().0 {
            table[position & mask].symbol = a;
            table[(position + step) & mask].symbol = b;
            position = (position + 2 * step) & mask;
        }
    } else if high_threshold == mask {
        for (s, &prob) in distribution.iter().enumerate() {
            let sym = s as u8;
            for _ in 0..prob.max(0) {
                table[position].symbol = sym;
                position = (position + step) & mask;
            }
        }
    } else {
        for (s, &prob) in distribution.iter().enumerate() {
            for _ in 0..prob.max(0) {
                table[position].symbol = s as u8;
                position = (position + step) & mask;
                while position > high_threshold {
                    position = (position + step) & mask;
                }
            }
        }
    }

    if position != 0 {
        return Err(DecompressError::BadFseTable);
    }

    for entry in table.iter_mut() {
        let s = entry.symbol as usize;
        let next_state = next[s] as u32;
        next[s] += 1;

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

    /// The field-by-field parser this module used before the windowed one.
    fn reference_parse(
        reader: &mut BitReader,
        max_symbol: u8,
    ) -> Result<(Vec<i16>, u8), DecompressError> {
        let accuracy_log = reader.read_bits(4)? as u8 + 5;
        if accuracy_log > MAX_TABLE_LOG {
            return Err(DecompressError::BadFseTable);
        }
        let table_size = 1i32 << accuracy_log;
        let mut remaining = table_size + 1;
        let mut threshold = table_size;
        let mut nb_bits = accuracy_log + 1;
        let mut distribution = Vec::new();
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
            distribution.push(prob as i16);
            remaining -= prob.abs();
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
        Ok((distribution, accuracy_log))
    }

    #[test]
    fn parse_description_matches_reference() {
        let mut seed = 0x853c_49e6_748f_ea9bu64;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut dist = Vec::new();
        let (mut ok, mut err) = (0, 0);
        for round in 0..30_000 {
            let max_symbol = [12u8, 31, 35, 52, 255][round % 5];
            let data: Vec<u8> = match round % 4 {
                // Two symbols with a long run of zeros between them, whole
                // or truncated.
                3 => {
                    let gap = 1 + (rand() as usize) % 200;
                    let mut sparse = vec![0i16; gap + 2];
                    sparse[0] = 16;
                    sparse[gap + 1] = 16;
                    let mut bytes = serialize_fse_table_description(&sparse, 5);
                    if rand() % 2 == 0 {
                        bytes.truncate((rand() as usize) % bytes.len());
                    }
                    bytes
                }
                // Valid descriptions, then truncated copies of them.
                0 | 1 => {
                    let accuracy_log = 5 + (rand() % 5) as u8;
                    let symbols = 2 + (rand() as usize) % (max_symbol as usize);
                    let freqs: Vec<u32> = (0..symbols)
                        .map(|_| {
                            if rand() % 3 == 0 {
                                0
                            } else {
                                (rand() % 100) as u32 + 1
                            }
                        })
                        .collect();
                    if freqs.iter().filter(|&&f| f > 0).count() < 2 {
                        continue;
                    }
                    let mut bytes = serialize_fse_table_description(
                        &normalize_counts(&freqs, accuracy_log),
                        accuracy_log,
                    );
                    bytes.extend((0..rand() % 4).map(|_| rand() as u8));
                    if round % 4 == 1 {
                        bytes.truncate((rand() as usize) % bytes.len());
                    }
                    bytes
                }
                _ => (0..(rand() % 40)).map(|_| rand() as u8).collect(),
            };
            let mut reader = BitReader::new(&data);
            let mut expected_reader = BitReader::new(&data);
            let result = parse_fse_table_description_into(&mut reader, max_symbol, &mut dist);
            match (result, reference_parse(&mut expected_reader, max_symbol)) {
                (Ok(acc), Ok((expected, expected_acc))) => {
                    assert_eq!((acc, &dist), (expected_acc, &expected), "{data:?}");
                    assert_eq!(reader.bytes_consumed(), expected_reader.bytes_consumed());
                    ok += 1;
                }
                (Err(e), Err(expected)) => {
                    assert_eq!(e, expected, "{data:?}");
                    err += 1;
                }
                (result, expected) => panic!("{result:?} vs {expected:?} for {data:?}"),
            }
        }
        assert!(ok > 5000 && err > 5000, "ok {ok}, err {err}");
    }

    /// The plain spread: one symbol cell at a time, skipping cells taken by
    /// "less than one" symbols.
    fn reference_decode_table(distribution: &[i16], accuracy_log: u8) -> Vec<FseDecodeEntry> {
        let size = 1usize << accuracy_log;
        let mask = size - 1;
        let step = (size >> 1) + (size >> 3) + 3;
        let mut symbols = vec![0u8; size];
        let mut next = vec![0u32; distribution.len()];
        let mut high = mask;
        for (s, &prob) in distribution.iter().enumerate() {
            if prob == -1 {
                symbols[high] = s as u8;
                high -= 1;
                next[s] = 1;
            } else {
                next[s] = prob.max(0) as u32;
            }
        }
        let mut position = 0;
        for (s, &prob) in distribution.iter().enumerate() {
            for _ in 0..prob.max(0) {
                symbols[position] = s as u8;
                position = (position + step) & mask;
                while position > high {
                    position = (position + step) & mask;
                }
            }
        }
        assert_eq!(position, 0);
        symbols
            .iter()
            .map(|&symbol| {
                let state = next[symbol as usize];
                next[symbol as usize] += 1;
                let num_bits = accuracy_log as u32 - high_bit(state);
                FseDecodeEntry {
                    base_line: ((state << num_bits) - size as u32) as u16,
                    num_bits: num_bits as u8,
                    symbol,
                }
            })
            .collect()
    }

    #[test]
    fn decode_table_matches_reference() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let (mut table, mut next) = (Vec::new(), Vec::new());
        for round in 0..4000 {
            let accuracy_log = 5 + (rand() % 5) as u8;
            let size = 1i32 << accuracy_log;
            let symbols = 2 + (rand() % 60) as usize;
            // Half the rounds use "less than one" symbols (-1).
            let low_prob = round % 2 == 0;
            let mut dist = vec![0i16; symbols];
            let mut remaining = size;
            for d in dist.iter_mut() {
                match rand() % 4 {
                    0 => {}
                    1 if low_prob && remaining > 1 => {
                        *d = -1;
                        remaining -= 1;
                    }
                    _ => {
                        let p = 1 + (rand() % (size as u64 / 4)) as i32;
                        let p = p.min(remaining - 1);
                        if p > 0 {
                            *d = p as i16;
                            remaining -= p;
                        }
                    }
                }
            }
            // Give the rest to one symbol so the counts fill the table.
            let last = (rand() as usize) % symbols;
            if dist[last] == -1 {
                dist[last] = 0;
                remaining += 1;
            }
            dist[last] += remaining as i16;

            build_decode_table_into(&dist, accuracy_log, &mut table, &mut next).unwrap();
            let fields = |t: &[FseDecodeEntry]| -> Vec<(u8, u8, u16)> {
                t.iter()
                    .map(|e| (e.symbol, e.num_bits, e.base_line))
                    .collect()
            };
            assert_eq!(
                fields(&table),
                fields(&reference_decode_table(&dist, accuracy_log)),
                "{dist:?} at log {accuracy_log}"
            );
        }
    }
}
