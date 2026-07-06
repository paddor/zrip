#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use super::primitives;
use crate::bitstream::reader_reverse::ReverseBitReader;
use crate::error::DecompressError;
use crate::huffman::HuffmanDecodeEntry;

pub fn decode_single_stream(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
) -> Result<Vec<u8>, DecompressError> {
    if table.len() < (1usize << table_log) {
        return Err(DecompressError::BadHuffmanStream);
    }
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
    if table.len() < (1usize << table_log) {
        return Err(DecompressError::BadHuffmanStream);
    }
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
    if table.len() < (1usize << table_log) {
        return Err(DecompressError::BadHuffmanStream);
    }
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
    if table.len() < (1usize << table_log) {
        return Err(DecompressError::BadHuffmanStream);
    }
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
    if table.len() < (1usize << table_log) {
        return Err(DecompressError::BadHuffmanStream);
    }
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
    primitives::set_vec_len(output, output_size);
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

#[cfg(all(test, miri, not(feature = "paranoid")))]
mod ub_tests {
    use super::*;
    use crate::bitstream::writer::BitWriter;

    #[test]
    fn public_decode_single_stream_trusts_huffman_table_len() {
        // Issue: decode_single_stream is a safe public API, but huf_table_lookup
        // uses get_unchecked and only debug-asserts that the caller supplied at
        // least 1 << table_log entries. This one-bit stream decodes index 1 from
        // a one-entry table, so miri reports an out-of-bounds read.
        let table = [HuffmanDecodeEntry {
            symbol: 0,
            num_bits: 1,
        }];
        let mut data = BitWriter::new();
        data.write_bits(1, 1);
        data.close_reverse_stream();

        let _ = decode_single_stream(&table, 1, &data.into_bytes(), 1);
    }
}
