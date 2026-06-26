use crate::bitstream::reader_reverse::ReverseBitReader;
use crate::error::DecompressError;
use crate::huffman::HuffmanDecodeEntry;

#[cfg(all(target_arch = "x86_64", not(feature = "paranoid")))]
#[target_feature(enable = "bmi2")]
fn decode_single_stream_bmi2(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output: &mut [u8],
) -> Result<(), DecompressError> {
    super::decode::decode_single_stream_into(table, table_log, data, output)
}

#[cfg(all(target_arch = "x86_64", not(feature = "paranoid")))]
pub(super) fn decode_single_stream_bmi2_safe(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output: &mut [u8],
) -> Result<(), DecompressError> {
    // SAFETY: caller verifies BMI2 availability via cpu_tier() >= CpuTier::Bmi2
    unsafe { decode_single_stream_bmi2(table, table_log, data, output) }
}

#[cfg(all(target_arch = "x86_64", not(feature = "paranoid")))]
#[target_feature(enable = "bmi2")]
fn decode_4_streams_core_bmi2(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
    output: &mut [u8],
) -> Result<(), DecompressError> {
    decode_4_streams_core(table, table_log, data, output_size, output)
}

#[cfg(all(target_arch = "x86_64", not(feature = "paranoid")))]
pub(super) fn decode_4_streams_core_bmi2_safe(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
    output: &mut [u8],
) -> Result<(), DecompressError> {
    // SAFETY: caller verifies BMI2 availability via cpu_tier() >= CpuTier::Bmi2
    unsafe { decode_4_streams_core_bmi2(table, table_log, data, output_size, output) }
}

pub(super) fn decode_4_streams_core(
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

    let mut c1 = r1.container;
    let mut bc1 = r1.bits_consumed;
    let mut p1_idx = r1.ptr;

    let mut c2 = r2.container;
    let mut bc2 = r2.bits_consumed;
    let mut p2_idx = r2.ptr;

    let mut c3 = r3.container;
    let mut bc3 = r3.bits_consumed;
    let mut p3_idx = r3.ptr;

    let mut c4 = r4.container;
    let mut bc4 = r4.bits_consumed;
    let mut p4_idx = r4.ptr;

    let fast1_limit = r1.limit_ptr + 8;
    let fast2_limit = r2.limit_ptr + 8;
    let fast3_limit = r3.limit_ptr + 8;
    let fast4_limit = r4.limit_ptr + 8;

    let mut o1_idx: usize = 0;
    let mut o2_idx: usize = seg;
    let mut o3_idx: usize = seg * 2;
    let mut o4_idx: usize = seg * 3;

    let tl = table_log as u32;

    macro_rules! refill {
        ($c:expr, $bc:expr, $p_idx:expr, $data:expr) => {{
            let byte_shift = ($bc >> 3) as usize;
            $p_idx -= byte_shift;
            $bc -= (byte_shift as u32) * 8;
            $c = u64::from_le_bytes($data[$p_idx..$p_idx + 8].try_into().unwrap());
        }};
    }

    macro_rules! decode_one {
        ($c:expr, $bc:expr, $o_idx:expr) => {{
            let idx = (($c << ($bc & 63)) >> (64 - tl)) as usize;
            let e = table[idx];
            debug_assert!(e.num_bits > 0, "Huffman table entry with 0 bits");
            output[$o_idx] = e.symbol;
            $bc += e.num_bits as u32;
            $o_idx += 1;
        }};
    }

    while o1_idx + 5 <= seg1_end
        && o2_idx + 5 <= seg2_end
        && o3_idx + 5 <= seg3_end
        && o4_idx + 5 <= seg4_end
        && p1_idx >= fast1_limit
        && p2_idx >= fast2_limit
        && p3_idx >= fast3_limit
        && p4_idx >= fast4_limit
    {
        refill!(c1, bc1, p1_idx, r1.data);
        refill!(c2, bc2, p2_idx, r2.data);
        refill!(c3, bc3, p3_idx, r3.data);
        refill!(c4, bc4, p4_idx, r4.data);

        decode_one!(c1, bc1, o1_idx);
        decode_one!(c2, bc2, o2_idx);
        decode_one!(c3, bc3, o3_idx);
        decode_one!(c4, bc4, o4_idx);

        decode_one!(c1, bc1, o1_idx);
        decode_one!(c2, bc2, o2_idx);
        decode_one!(c3, bc3, o3_idx);
        decode_one!(c4, bc4, o4_idx);

        decode_one!(c1, bc1, o1_idx);
        decode_one!(c2, bc2, o2_idx);
        decode_one!(c3, bc3, o3_idx);
        decode_one!(c4, bc4, o4_idx);

        decode_one!(c1, bc1, o1_idx);
        decode_one!(c2, bc2, o2_idx);
        decode_one!(c3, bc3, o3_idx);
        decode_one!(c4, bc4, o4_idx);

        decode_one!(c1, bc1, o1_idx);
        decode_one!(c2, bc2, o2_idx);
        decode_one!(c3, bc3, o3_idx);
        decode_one!(c4, bc4, o4_idx);
    }

    r1.container = c1;
    r1.bits_consumed = bc1;
    r1.ptr = p1_idx;
    r2.container = c2;
    r2.bits_consumed = bc2;
    r2.ptr = p2_idx;
    r3.container = c3;
    r3.bits_consumed = bc3;
    r3.ptr = p3_idx;
    r4.container = c4;
    r4.bits_consumed = bc4;
    r4.ptr = p4_idx;

    super::decode::decode_stream_tail(table, table_log, &mut r1, &mut output[o1_idx..seg1_end])?;
    super::decode::decode_stream_tail(table, table_log, &mut r2, &mut output[o2_idx..seg2_end])?;
    super::decode::decode_stream_tail(table, table_log, &mut r3, &mut output[o3_idx..seg3_end])?;
    super::decode::decode_stream_tail(table, table_log, &mut r4, &mut output[o4_idx..seg4_end])?;

    Ok(())
}
