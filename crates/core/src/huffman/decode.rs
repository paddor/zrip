#![cfg_attr(feature = "paranoid", forbid(unsafe_code))]

#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use super::primitives;
use crate::bitstream::reader_reverse::ReverseBitReader;
use crate::error::DecompressError;
use crate::huffman::{HuffmanDecodeEntry, MAX_TABLE_LOG};

pub fn decode_single_stream(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
) -> Result<Vec<u8>, DecompressError> {
    validate_decode_table(table, table_log)?;
    let mut reader = ReverseBitReader::new(data).map_err(|_| DecompressError::BadHuffmanStream)?;

    let mut output = Vec::with_capacity(output_size);

    let tl = table_log as usize;
    while output.len() + 4 <= output_size && reader.bits_remaining() >= tl + tl + tl + tl {
        for _ in 0..4 {
            let bits = reader.peek_bits(table_log);
            let entry = primitives::huf_table_lookup(table, bits as usize);
            output.push(entry.symbol);
            reader.consume_bits(entry.num_bits);
        }
    }

    while output.len() < output_size {
        reader.refill();
        let remaining = reader.bits_remaining();
        if remaining == 0 {
            return Err(DecompressError::BadHuffmanStream);
        }
        let bits = if remaining >= tl {
            reader.peek_bits(table_log)
        } else {
            reader.peek_bits(remaining as u8) << (tl - remaining)
        };
        let entry = primitives::huf_table_lookup(table, bits as usize);
        if entry.num_bits as usize > remaining {
            return Err(DecompressError::BadHuffmanStream);
        }
        output.push(entry.symbol);
        reader.bits_consumed += entry.num_bits as u32;
    }

    if reader.bits_remaining() != 0 {
        return Err(DecompressError::BadHuffmanStream);
    }

    Ok(output)
}

pub fn decode_single_stream_into(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output: &mut [u8],
) -> Result<(), DecompressError> {
    validate_decode_table(table, table_log)?;
    let mut reader = ReverseBitReader::new(data).map_err(|_| DecompressError::BadHuffmanStream)?;
    decode_stream_tail(table, table_log, &mut reader, output)
}

pub fn decode_single_stream_vec(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
    output: &mut Vec<u8>,
) -> Result<(), DecompressError> {
    validate_decode_table(table, table_log)?;
    prepare_output(output, output_size);
    #[cfg(all(target_arch = "x86_64", not(feature = "paranoid")))]
    {
        if crate::simd::has_bmi2() {
            // SAFETY: has_bmi2() proves BMI2 is available.
            let result = unsafe {
                super::decode_4stream::decode_single_stream_bmi2_safe(
                    table, table_log, data, output,
                )
            };
            if result.is_err() {
                output.clear();
            }
            return result;
        }
    }
    let result = decode_single_stream_into(table, table_log, data, output);
    if result.is_err() {
        output.clear();
    }
    result
}

pub fn decode_4_streams(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
) -> Result<Vec<u8>, DecompressError> {
    validate_decode_table(table, table_log)?;
    let mut output = vec![0u8; output_size];
    decode_4_streams_core_safe(table, table_log, data, output_size, &mut output)?;
    Ok(output)
}

pub fn decode_4_streams_into(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
    output: &mut Vec<u8>,
) -> Result<(), DecompressError> {
    validate_decode_table(table, table_log)?;
    prepare_output(output, output_size);
    #[cfg(all(target_arch = "x86_64", not(feature = "paranoid")))]
    {
        if crate::simd::has_bmi2() {
            // SAFETY: has_bmi2() proves BMI2 is available.
            let result = unsafe {
                super::decode_4stream::decode_4_streams_core_bmi2_safe(
                    table,
                    table_log,
                    data,
                    output_size,
                    output,
                )
            };
            if result.is_err() {
                output.clear();
            }
            return result;
        }
    }
    let result = decode_4_streams_core_safe(table, table_log, data, output_size, output);
    if result.is_err() {
        output.clear();
    }
    result
}

fn validate_decode_table(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
) -> Result<(), DecompressError> {
    if table_log == 0 || table_log > MAX_TABLE_LOG {
        return Err(DecompressError::BadHuffmanStream);
    }
    if table.len() < (1usize << table_log) {
        return Err(DecompressError::BadHuffmanStream);
    }
    Ok(())
}

fn decode_4_streams_core_safe(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
    output: &mut [u8],
) -> Result<(), DecompressError> {
    super::decode_4stream::decode_4_streams_core(table, table_log, data, output_size, output)
}

#[inline(always)]
fn prepare_output(output: &mut Vec<u8>, output_size: usize) {
    output.resize(output_size, 0);
}

pub(super) fn decode_stream_tail(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    reader: &mut ReverseBitReader,
    output: &mut [u8],
) -> Result<(), DecompressError> {
    let output_size = output.len();
    let tl = table_log as usize;
    let mut pos = 0;

    let tl5 = (tl + tl + tl + tl + tl) as u32;
    while pos + 5 <= output_size {
        reader.refill();
        if reader.bits_remaining() < tl5 as usize
            || 64u32.saturating_sub(reader.bits_consumed) < tl5
        {
            break;
        }

        for _ in 0..5 {
            let bits =
                ((reader.container << reader.bits_consumed) >> (64 - table_log as u32)) as usize;
            let entry = primitives::huf_table_lookup(table, bits);
            primitives::huf_output_write(output, pos, entry.symbol);
            pos += 1;
            reader.bits_consumed += entry.num_bits as u32;
        }
    }

    // Either fewer than five symbols are left and the refill leaves at least
    // 57 bits in the container, or fewer than `5 * table_log` bits remain and
    // the refill reaches the start of the stream (`ptr == 0`), so the
    // container holds every remaining bit. Past the start, the shift reads
    // zeros. A symbol longer than the bits left pushes the count past 64,
    // which the checks below reject.
    reader.refill();
    let container = reader.container;
    let mut consumed = reader.bits_consumed;
    while pos < output_size {
        if consumed >= 64 {
            return Err(DecompressError::BadHuffmanStream);
        }
        let bits = ((container << consumed) >> (64 - table_log as u32)) as usize;
        let entry = primitives::huf_table_lookup(table, bits);
        primitives::huf_output_write(output, pos, entry.symbol);
        pos += 1;
        consumed += entry.num_bits as u32;
    }
    reader.bits_consumed = consumed;

    if reader.ptr != 0 || consumed != 64 {
        return Err(DecompressError::BadHuffmanStream);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitstream::writer::BitWriter;

    #[test]
    fn huffman_decode_rejects_zero_table_log_before_fast_tail_lookup() {
        // Regression: a one-entry table satisfies `table.len() >= 1 <<
        // table_log` when `table_log == 0`, but zero is not a valid Huffman
        // table log. Reject it before `decode_stream_tail` can compute an
        // unchecked table index from the fast path.
        let table = [HuffmanDecodeEntry {
            symbol: 0,
            num_bits: 1,
        }];
        let data = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x80];
        let mut output = [0u8; 5];

        assert_eq!(
            decode_single_stream_into(&table, 0, &data, &mut output),
            Err(DecompressError::BadHuffmanStream)
        );
    }

    #[test]
    fn huffman_4stream_rejects_zero_table_log_before_tail_lookup() {
        // Regression: the 4-stream decoder reaches the same tail helper after
        // splitting the output. Reject a zero table log at the public boundary
        // so it cannot reach `huf_table_lookup`'s unchecked read.
        let table = [HuffmanDecodeEntry {
            symbol: 0,
            num_bits: 1,
        }];
        let stream = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x80];
        let mut data = Vec::new();
        data.extend_from_slice(&8u16.to_le_bytes());
        data.extend_from_slice(&8u16.to_le_bytes());
        data.extend_from_slice(&8u16.to_le_bytes());
        data.extend_from_slice(&stream);
        data.extend_from_slice(&stream);
        data.extend_from_slice(&stream);
        data.extend_from_slice(&stream);
        let mut output = Vec::new();

        assert_eq!(
            decode_4_streams_into(&table, 0, &data, 24, &mut output),
            Err(DecompressError::BadHuffmanStream)
        );
    }

    #[test]
    fn stream_tail_matches_reference() {
        use crate::huffman::weights::build_huffman_decode_table;

        let mut seed = 0x9e6c_63d0_676a_9a99u64;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let (mut ok, mut err) = (0, 0);
        for _ in 0..2000 {
            // A random complete prefix code: split random leaves, at most
            // 11 bits deep.
            let leaves = 2 + (rand() as usize) % 60;
            let mut depths = vec![1u8, 1];
            while depths.len() < leaves {
                let i = (rand() as usize) % depths.len();
                if depths[i] < 11 {
                    depths[i] += 1;
                    depths.push(depths[i]);
                }
            }
            let table_log = *depths.iter().max().unwrap();
            // Leaf `k` is symbol `symbols[k]`. The last leaf takes the
            // highest symbol, whose weight is implied.
            let last_symbol = leaves - 1 + (rand() as usize) % (256 - leaves + 1);
            let mut symbols: Vec<usize> = (0..last_symbol).collect();
            for i in (1..symbols.len()).rev() {
                symbols.swap(i, (rand() as usize) % (i + 1));
            }
            symbols.truncate(leaves - 1);
            symbols.push(last_symbol);
            let mut weights = vec![0u8; last_symbol];
            for (&symbol, &depth) in symbols[..leaves - 1].iter().zip(&depths) {
                weights[symbol] = table_log + 1 - depth;
            }
            let (table, log) = build_huffman_decode_table(&weights).unwrap();
            assert_eq!(log, table_log);

            let mut codes = vec![(0u32, 0u8); 256];
            for (i, e) in table[..1 << table_log].iter().enumerate() {
                codes[e.symbol as usize] = ((i >> (table_log - e.num_bits)) as u32, e.num_bits);
            }
            let message: Vec<u8> = (0..=(rand() as usize) % 300)
                .map(|_| symbols[(rand() as usize) % leaves] as u8)
                .collect();
            let mut writer = BitWriter::new();
            for &symbol in message.iter().rev() {
                let (code, bits) = codes[symbol as usize];
                writer.write_bits(code, bits);
            }
            writer.close_reverse_stream();
            let stream = writer.into_bytes();

            let mut variants = vec![stream.clone()];
            let mut flipped = stream.clone();
            let bit = (rand() as usize) % (flipped.len() * 8);
            flipped[bit / 8] ^= 1 << (bit % 8);
            variants.push(flipped);
            variants.push(stream[..(rand() as usize) % stream.len()].to_vec());
            let mut longer = stream.clone();
            longer.push(1 + rand() as u8 % 255);
            variants.push(longer);

            for (v, data) in variants.iter().enumerate() {
                let expected = decode_single_stream(&table, log, data, message.len());
                let mut out = vec![0u8; message.len()];
                let result = decode_single_stream_into(&table, log, data, &mut out);
                match (expected, result) {
                    (Ok(expected), Ok(())) => {
                        assert_eq!(out, expected);
                        if v == 0 {
                            assert_eq!(out, message);
                        }
                        ok += 1;
                    }
                    (Err(e), Err(result_e)) => {
                        assert_eq!(result_e, e);
                        err += 1;
                    }
                    (expected, result) => panic!("{expected:?} vs {result:?}"),
                }
            }
        }
        assert!(ok > 2000 && err > 2000, "ok {ok}, err {err}");
    }

    #[test]
    fn decode_single_stream_rejects_short_huffman_table() {
        let table = [HuffmanDecodeEntry {
            symbol: 0,
            num_bits: 1,
        }];
        let mut data = BitWriter::new();
        data.write_bits(1, 1);
        data.close_reverse_stream();

        assert_eq!(
            decode_single_stream(&table, 1, &data.into_bytes(), 1),
            Err(DecompressError::BadHuffmanStream)
        );
    }
}
