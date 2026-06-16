#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

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
            let entry = unsafe { table.get_unchecked(bits as usize) };
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
        let entry = unsafe { table.get_unchecked(bits as usize) };
        if entry.num_bits as usize > remaining {
            return Err(DecompressError::BadHuffmanStream);
        }
        output.push(entry.symbol);
        let _ = reader.read_bits(entry.num_bits)?;
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
    unsafe {
        output.set_len(output_size);
    }
    #[cfg(target_arch = "x86_64")]
    {
        if crate::simd::cpu_tier() >= crate::simd::CpuTier::Bmi2 {
            return unsafe { decode_single_stream_bmi2(table, table_log, data, output) };
        }
    }
    decode_single_stream_into(table, table_log, data, output)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "bmi2")]
unsafe fn decode_single_stream_bmi2(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output: &mut [u8],
) -> Result<(), DecompressError> {
    decode_single_stream_into(table, table_log, data, output)
}

fn decode_stream_tail(
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
            let entry = unsafe { table.get_unchecked(bits as usize) };
            unsafe { *output.get_unchecked_mut(pos) = entry.symbol };
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
        let entry = unsafe { table.get_unchecked(bits as usize) };
        if entry.num_bits as usize > remaining {
            return Err(DecompressError::BadHuffmanStream);
        }
        unsafe { *output.get_unchecked_mut(pos) = entry.symbol };
        pos += 1;
        let _ = reader.read_bits(entry.num_bits)?;
    }

    Ok(())
}

pub fn decode_4_streams(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
) -> Result<Vec<u8>, DecompressError> {
    let mut output = vec![0u8; output_size];
    decode_4_streams_core(table, table_log, data, output_size, &mut output)?;
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
    unsafe {
        output.set_len(output_size);
    }
    #[cfg(target_arch = "x86_64")]
    {
        if crate::simd::cpu_tier() >= crate::simd::CpuTier::Bmi2 {
            return unsafe {
                decode_4_streams_core_bmi2(table, table_log, data, output_size, output)
            };
        }
    }
    decode_4_streams_core(table, table_log, data, output_size, output)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "bmi2")]
unsafe fn decode_4_streams_core_bmi2(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
    output: &mut [u8],
) -> Result<(), DecompressError> {
    decode_4_streams_core(table, table_log, data, output_size, output)
}

fn decode_4_streams_core(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
    output: &mut [u8],
) -> Result<(), DecompressError> {
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

    let seg = (output_size + 3) / 4;
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

    let tbl = table.as_ptr();

    let mut c1 = r1.container;
    let mut bc1 = r1.bits_consumed;
    let mut p1_ptr = unsafe { r1.data.as_ptr().add(r1.ptr) };

    let mut c2 = r2.container;
    let mut bc2 = r2.bits_consumed;
    let mut p2_ptr = unsafe { r2.data.as_ptr().add(r2.ptr) };

    let mut c3 = r3.container;
    let mut bc3 = r3.bits_consumed;
    let mut p3_ptr = unsafe { r3.data.as_ptr().add(r3.ptr) };

    let mut c4 = r4.container;
    let mut bc4 = r4.bits_consumed;
    let mut p4_ptr = unsafe { r4.data.as_ptr().add(r4.ptr) };

    let fast1 = unsafe { r1.data.as_ptr().add(r1.limit_ptr + 8) };
    let fast2 = unsafe { r2.data.as_ptr().add(r2.limit_ptr + 8) };
    let fast3 = unsafe { r3.data.as_ptr().add(r3.limit_ptr + 8) };
    let fast4 = unsafe { r4.data.as_ptr().add(r4.limit_ptr + 8) };

    let out_base = output.as_mut_ptr();
    let mut o1 = unsafe { out_base.add(0) };
    let mut o2 = unsafe { out_base.add(seg) };
    let mut o3 = unsafe { out_base.add(seg * 2) };
    let mut o4 = unsafe { out_base.add(seg * 3) };

    let o1_end = unsafe { out_base.add(seg1_end) };
    let o2_end = unsafe { out_base.add(seg2_end) };
    let o3_end = unsafe { out_base.add(seg3_end) };
    let o4_end = unsafe { out_base.add(seg4_end) };

    let tl = table_log as u32;

    macro_rules! refill {
        ($c:expr, $bc:expr, $p:expr) => {{
            let byte_shift = ($bc >> 3) as usize;
            $p = unsafe { $p.sub(byte_shift) };
            $bc -= (byte_shift as u32) * 8;
            $c = unsafe { ($p as *const u64).read_unaligned() };
        }};
    }

    macro_rules! decode_one {
        ($c:expr, $bc:expr, $o:expr) => {{
            let idx = (($c << ($bc & 63)) >> (64 - tl)) as usize;
            let e = unsafe { *tbl.add(idx) };
            unsafe { *$o = e.symbol };
            $bc += e.num_bits as u32;
            $o = unsafe { $o.add(1) };
        }};
    }

    while unsafe { o1.add(5) } <= o1_end
        && unsafe { o2.add(5) } <= o2_end
        && unsafe { o3.add(5) } <= o3_end
        && unsafe { o4.add(5) } <= o4_end
        && p1_ptr >= fast1
        && p2_ptr >= fast2
        && p3_ptr >= fast3
        && p4_ptr >= fast4
    {
        refill!(c1, bc1, p1_ptr);
        refill!(c2, bc2, p2_ptr);
        refill!(c3, bc3, p3_ptr);
        refill!(c4, bc4, p4_ptr);

        decode_one!(c1, bc1, o1);
        decode_one!(c2, bc2, o2);
        decode_one!(c3, bc3, o3);
        decode_one!(c4, bc4, o4);

        decode_one!(c1, bc1, o1);
        decode_one!(c2, bc2, o2);
        decode_one!(c3, bc3, o3);
        decode_one!(c4, bc4, o4);

        decode_one!(c1, bc1, o1);
        decode_one!(c2, bc2, o2);
        decode_one!(c3, bc3, o3);
        decode_one!(c4, bc4, o4);

        decode_one!(c1, bc1, o1);
        decode_one!(c2, bc2, o2);
        decode_one!(c3, bc3, o3);
        decode_one!(c4, bc4, o4);

        decode_one!(c1, bc1, o1);
        decode_one!(c2, bc2, o2);
        decode_one!(c3, bc3, o3);
        decode_one!(c4, bc4, o4);
    }

    r1.container = c1;
    r1.bits_consumed = bc1;
    r1.ptr = unsafe { p1_ptr.offset_from(r1.data.as_ptr()) } as usize;
    r2.container = c2;
    r2.bits_consumed = bc2;
    r2.ptr = unsafe { p2_ptr.offset_from(r2.data.as_ptr()) } as usize;
    r3.container = c3;
    r3.bits_consumed = bc3;
    r3.ptr = unsafe { p3_ptr.offset_from(r3.data.as_ptr()) } as usize;
    r4.container = c4;
    r4.bits_consumed = bc4;
    r4.ptr = unsafe { p4_ptr.offset_from(r4.data.as_ptr()) } as usize;

    let p1 = unsafe { o1.offset_from(out_base) } as usize;
    let p2 = unsafe { o2.offset_from(out_base) } as usize;
    let p3 = unsafe { o3.offset_from(out_base) } as usize;
    let p4 = unsafe { o4.offset_from(out_base) } as usize;

    decode_stream_tail(table, table_log, &mut r1, &mut output[p1..seg1_end])?;
    decode_stream_tail(table, table_log, &mut r2, &mut output[p2..seg2_end])?;
    decode_stream_tail(table, table_log, &mut r3, &mut output[p3..seg3_end])?;
    decode_stream_tail(table, table_log, &mut r4, &mut output[p4..seg4_end])?;

    Ok(())
}
