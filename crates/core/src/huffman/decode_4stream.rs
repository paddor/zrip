use crate::bitstream::primitives as bitstream_primitives;
use crate::bitstream::reader_reverse::ReverseBitReader;
use crate::error::DecompressError;
use crate::huffman::primitives as huffman_primitives;
use crate::huffman::{DECODE_TABLE_SIZE, HuffmanDecodeEntry};

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
pub(super) unsafe fn decode_single_stream_bmi2_safe(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output: &mut [u8],
) -> Result<(), DecompressError> {
    // SAFETY: The caller verifies BMI2 availability via has_bmi2().
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
pub(super) unsafe fn decode_4_streams_core_bmi2_safe(
    table: &[HuffmanDecodeEntry],
    table_log: u8,
    data: &[u8],
    output_size: usize,
    output: &mut [u8],
) -> Result<(), DecompressError> {
    // SAFETY: The caller verifies BMI2 availability via has_bmi2().
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
    let seg2_end = seg;
    let seg3_end = seg;
    let seg4_end = remaining;

    let (out1, rest) = output.split_at_mut(seg);
    let (out2, rest) = rest.split_at_mut(seg);
    let (out3, out4) = rest.split_at_mut(seg);
    let out4 = &mut out4[..remaining];

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

    let fast1_limit = r1.limit_ptr;
    let fast2_limit = r2.limit_ptr;
    let fast3_limit = r3.limit_ptr;
    let fast4_limit = r4.limit_ptr;

    let mut o1_idx: usize = 0;
    let mut o2_idx: usize = 0;
    let mut o3_idx: usize = 0;
    let mut o4_idx: usize = 0;

    let tl = table_log as u32;
    // `shift >= 53` bounds every fast-round index below the 2048-entry table,
    // so the lookups need neither a mask nor a bounds check.
    if !(1..=11).contains(&tl) {
        return Err(DecompressError::BadHuffmanStream);
    }
    let shift = 64 - tl;

    #[inline(always)]
    fn can_refill_fast(bits_consumed: u32, ptr: usize, data_len: usize) -> bool {
        let byte_shift = (bits_consumed >> 3) as usize;
        ptr.checked_sub(byte_shift)
            .and_then(|new_ptr| new_ptr.checked_add(8))
            .is_some_and(|end| end <= data_len)
    }

    macro_rules! refill {
        ($c:expr, $bc:expr, $p_idx:expr, $data:expr) => {{
            let byte_shift = ($bc >> 3) as usize;
            $p_idx -= byte_shift;
            $bc -= (byte_shift as u32) * 8;
            $c = bitstream_primitives::read_u64_le_unaligned($data, $p_idx);
        }};
    }

    macro_rules! decode_one {
        ($c:expr, $bc:expr, $output:expr, $o_idx:expr) => {{
            let idx = (($c << ($bc & 63)) >> (64 - tl)) as usize;
            let e = huffman_primitives::huf_table_lookup(table, idx);
            debug_assert!(e.num_bits > 0, "Huffman table entry with 0 bits");
            huffman_primitives::huf_output_write($output, $o_idx, e.symbol);
            $bc += e.num_bits as u32;
            $o_idx += 1;
        }};
    }

    // A fixed-size table needs no bounds checks for the fast-round indices,
    // which `shift` keeps below its size.
    let fixed: &[HuffmanDecodeEntry; DECODE_TABLE_SIZE] = table
        .first_chunk()
        .ok_or(DecompressError::BadHuffmanStream)?;

    // The fast rounds use C zstd's layout: each stream is a left-aligned
    // container `$b` with a sentinel bit. A load sets bit 0, and each symbol
    // shifts the container left by its length, so the next code is always
    // the top `tl` bits and `trailing_zeros` is the number of bits consumed
    // since the load. A symbol then depends on the previous one only through
    // shift, lookup, shift; no separate bit count sits in that chain. A round
    // reads at most 62 bits after a load, so the sentinel never reaches the
    // top 11 bits and bit 0's own value is never read.
    macro_rules! reload {
        ($b:expr, $p_idx:expr, $data:expr) => {{
            let consumed = $b.trailing_zeros();
            $p_idx -= (consumed >> 3) as usize;
            $b = (bitstream_primitives::read_u64_le_unaligned($data, $p_idx) | 1) << (consumed & 7);
        }};
    }

    macro_rules! decode_five {
        ($b:expr, $output:expr, $o_idx:expr) => {{
            let mut syms = [0u8; 5];
            for s in &mut syms {
                let e = fixed[($b >> shift) as usize];
                *s = e.symbol;
                $b <<= e.num_bits;
            }
            $output[$o_idx..$o_idx + 5].copy_from_slice(&syms);
            $o_idx += 5;
        }};
    }

    // Back to the reader's layout: the container as loaded at `$p_idx`, and
    // the bits consumed since that load.
    macro_rules! unload {
        ($b:expr, $c:expr, $bc:expr, $p_idx:expr, $data:expr) => {{
            $bc = $b.trailing_zeros();
            $c = bitstream_primitives::read_u64_le_unaligned($data, $p_idx);
        }};
    }

    // Rounds a stream can run without further checks. A round refills (the
    // pointer moves back at most 7 bytes: fewer than 64 bits were consumed)
    // and decodes five symbols (at most 55 bits with table_log <= 11).
    #[inline(always)]
    fn safe_rounds(o_idx: usize, end: usize, p_idx: usize, limit: usize) -> usize {
        let by_output = end.saturating_sub(o_idx) / 5;
        let by_input = p_idx.saturating_sub(limit) / 7;
        by_output.min(by_input)
    }

    let mut rounds = safe_rounds(o1_idx, seg1_end, p1_idx, fast1_limit)
        .min(safe_rounds(o2_idx, seg2_end, p2_idx, fast2_limit))
        .min(safe_rounds(o3_idx, seg3_end, p3_idx, fast3_limit))
        .min(safe_rounds(o4_idx, seg4_end, p4_idx, fast4_limit));
    if rounds > 0 {
        // Every stream has at least 8 bytes here, so each container is a full
        // load with at most 8 bits consumed.
        let mut b1 = (c1 | 1) << bc1;
        let mut b2 = (c2 | 1) << bc2;
        let mut b3 = (c3 | 1) << bc3;
        let mut b4 = (c4 | 1) << bc4;
        loop {
            for _ in 0..rounds {
                reload!(b1, p1_idx, r1.data);
                reload!(b2, p2_idx, r2.data);
                reload!(b3, p3_idx, r3.data);
                reload!(b4, p4_idx, r4.data);
                decode_five!(b1, out1, o1_idx);
                decode_five!(b2, out2, o2_idx);
                decode_five!(b3, out3, o3_idx);
                decode_five!(b4, out4, o4_idx);
            }
            rounds = safe_rounds(o1_idx, seg1_end, p1_idx, fast1_limit)
                .min(safe_rounds(o2_idx, seg2_end, p2_idx, fast2_limit))
                .min(safe_rounds(o3_idx, seg3_end, p3_idx, fast3_limit))
                .min(safe_rounds(o4_idx, seg4_end, p4_idx, fast4_limit));
            if rounds == 0 {
                break;
            }
        }
        unload!(b1, c1, bc1, p1_idx, r1.data);
        unload!(b2, c2, bc2, p2_idx, r2.data);
        unload!(b3, c3, bc3, p3_idx, r3.data);
        unload!(b4, c4, bc4, p4_idx, r4.data);
    }

    macro_rules! finish_fast {
        (
            $c:expr,
            $bc:expr,
            $p_idx:expr,
            $data:expr,
            $fast_limit:expr,
            $output:expr,
            $o_idx:expr,
            $end:expr
        ) => {{
            while $o_idx + 5 <= $end && $p_idx >= $fast_limit {
                if !can_refill_fast($bc, $p_idx, $data.len()) {
                    break;
                }
                refill!($c, $bc, $p_idx, $data);

                decode_one!($c, $bc, $output, $o_idx);
                decode_one!($c, $bc, $output, $o_idx);
                decode_one!($c, $bc, $output, $o_idx);
                decode_one!($c, $bc, $output, $o_idx);
                decode_one!($c, $bc, $output, $o_idx);
            }
        }};
    }

    finish_fast!(
        c1,
        bc1,
        p1_idx,
        r1.data,
        fast1_limit,
        out1,
        o1_idx,
        seg1_end
    );
    finish_fast!(
        c2,
        bc2,
        p2_idx,
        r2.data,
        fast2_limit,
        out2,
        o2_idx,
        seg2_end
    );
    finish_fast!(
        c3,
        bc3,
        p3_idx,
        r3.data,
        fast3_limit,
        out3,
        o3_idx,
        seg3_end
    );
    finish_fast!(
        c4,
        bc4,
        p4_idx,
        r4.data,
        fast4_limit,
        out4,
        o4_idx,
        seg4_end
    );

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

    super::decode::decode_stream_tail(table, table_log, &mut r1, &mut out1[o1_idx..seg1_end])?;
    super::decode::decode_stream_tail(table, table_log, &mut r2, &mut out2[o2_idx..seg2_end])?;
    super::decode::decode_stream_tail(table, table_log, &mut r3, &mut out3[o3_idx..seg3_end])?;
    super::decode::decode_stream_tail(table, table_log, &mut r4, &mut out4[o4_idx..seg4_end])?;

    Ok(())
}
