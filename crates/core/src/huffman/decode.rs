#![cfg_attr(feature = "paranoid", forbid(unsafe_code))]

#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use super::primitives;
use crate::bitstream::reader_reverse::ReverseBitReader;
use crate::error::DecompressError;
use crate::huffman::{HuffmanDecodeEntry, MAX_TABLE_LOG};

#[cfg(not(feature = "paranoid"))]
macro_rules! huf_set_vec_len {
    ($buf:expr, $len:expr) => {{
        // SAFETY: decode_stream_tail writes every byte in the resized output
        // before callers can observe it.
        unsafe { primitives::set_vec_len($buf, $len) }
    }};
}

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
        if crate::simd::cpu_tier() >= crate::simd::CpuTier::Bmi2 {
            let result = super::decode_4stream::decode_single_stream_bmi2_safe(
                table, table_log, data, output,
            );
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
        if crate::simd::cpu_tier() >= crate::simd::CpuTier::Bmi2 {
            let result = super::decode_4stream::decode_4_streams_core_bmi2_safe(
                table,
                table_log,
                data,
                output_size,
                output,
            );
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

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
fn prepare_output(output: &mut Vec<u8>, output_size: usize) {
    output.clear();
    output.reserve(output_size);
    huf_set_vec_len!(output, output_size);
}

#[cfg(feature = "paranoid")]
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

    while pos < output_size {
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
        primitives::huf_output_write(output, pos, entry.symbol);
        pos += 1;
        reader.bits_consumed += entry.num_bits as u32;
    }

    if reader.bits_remaining() != 0 {
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
