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
        let _ = reader.read_bits(entry.num_bits)?;
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
    output.clear();
    output.reserve(output_size);
    primitives::set_vec_len(output, output_size);
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
    output.clear();
    output.reserve(output_size);
    primitives::set_vec_len(output, output_size);
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

#[cfg(not(feature = "paranoid"))]
fn decode_4_streams_core_safe(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
    output: &mut [u8],
) -> Result<(), DecompressError> {
    super::decode_4stream::decode_4_streams_core(table, table_log, data, output_size, output)
}

#[cfg(feature = "paranoid")]
fn decode_4_streams_core_safe(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
    output: &mut [u8],
) -> Result<(), DecompressError> {
    use crate::bitstream::reader_reverse::ReverseBitReader;

    if data.len() < 6 {
        return Err(DecompressError::BadHuffmanStream);
    }

    let s1_size = u16::from_le_bytes([data[0], data[1]]) as usize;
    let s2_size = u16::from_le_bytes([data[2], data[3]]) as usize;
    let s3_size = u16::from_le_bytes([data[4], data[5]]) as usize;

    let jump_table_size = 6;
    let s1_start = jump_table_size;
    let s2_start = s1_start + s1_size;
    let s3_start = s2_start + s2_size;
    let s4_start = s3_start + s3_size;

    if s4_start > data.len() {
        return Err(DecompressError::BadHuffmanStream);
    }

    let seg = output_size.div_ceil(4);
    if seg * 3 >= output_size {
        return Err(DecompressError::BadHuffmanStream);
    }
    let remaining = output_size - seg * 3;

    let mut r1 = ReverseBitReader::new(&data[s1_start..s2_start])
        .map_err(|_| DecompressError::BadHuffmanStream)?;
    let mut r2 = ReverseBitReader::new(&data[s2_start..s3_start])
        .map_err(|_| DecompressError::BadHuffmanStream)?;
    let mut r3 = ReverseBitReader::new(&data[s3_start..s4_start])
        .map_err(|_| DecompressError::BadHuffmanStream)?;
    let mut r4 =
        ReverseBitReader::new(&data[s4_start..]).map_err(|_| DecompressError::BadHuffmanStream)?;

    let seg1_end = seg;
    let seg2_end = seg * 2;
    let seg3_end = seg * 3;
    let seg4_end = seg * 3 + remaining;

    decode_stream_tail(table, table_log, &mut r1, &mut output[0..seg1_end])?;
    decode_stream_tail(table, table_log, &mut r2, &mut output[seg..seg2_end])?;
    decode_stream_tail(table, table_log, &mut r3, &mut output[seg * 2..seg3_end])?;
    decode_stream_tail(table, table_log, &mut r4, &mut output[seg * 3..seg4_end])?;

    Ok(())
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

    reader.refill();
    while pos + 4 <= output_size && reader.bits_remaining() >= tl + tl + tl + tl {
        for _ in 0..4 {
            let bits = reader.peek_bits(table_log);
            let entry = primitives::huf_table_lookup(table, bits as usize);
            primitives::huf_output_write(output, pos, entry.symbol);
            pos += 1;
            reader.consume_bits(entry.num_bits);
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
        let _ = reader.read_bits(entry.num_bits)?;
    }

    if reader.bits_remaining() != 0 {
        return Err(DecompressError::BadHuffmanStream);
    }

    Ok(())
}
