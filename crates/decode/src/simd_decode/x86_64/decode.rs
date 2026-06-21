#[cfg(feature = "alloc")]
use alloc::vec::Vec;

#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{__m256i, _mm256_loadu_si256, _mm256_storeu_si256};

use crate::sequences::SequenceDecodeTables;
use zrip_core::bitstream::reader_reverse::ReverseBitReader;
use zrip_core::error::DecompressError;
use zrip_core::fse::FseSeqDecodeEntry;
use zrip_core::hint::{likely, unlikely};

/// # Safety
/// AVX2 and BMI2 must be available.
#[target_feature(enable = "avx2,bmi2")]
unsafe fn decode_execute_avx2_inner<const HAS_HISTORY: bool>(
    seq_data: &[u8],
    num_sequences: u32,
    tables: &SequenceDecodeTables,
    rep_offsets: &mut [u32; 3],
    literals: &[u8],
    output: &mut Vec<u8>,
    history: &[u8],
) -> Result<(), DecompressError> {
    if num_sequences == 0 {
        return Ok(());
    }
    if seq_data.is_empty() {
        return Err(DecompressError::CorruptSequences);
    }

    let mut rev_reader =
        ReverseBitReader::new(seq_data).map_err(|_| DecompressError::CorruptSequences)?;

    let mut ll_state = init_state(&tables.ll_table, tables.ll_accuracy, &mut rev_reader)?;
    let mut of_state = init_state(&tables.of_table, tables.of_accuracy, &mut rev_reader)?;
    let mut ml_state = init_state(&tables.ml_table, tables.ml_accuracy, &mut rev_reader)?;

    const WILDCOPY_OVERLENGTH: usize = 64;
    let max_output = zrip_core::frame::MAX_BLOCK_SIZE;
    output.reserve(max_output + WILDCOPY_OVERLENGTH);

    let out_base = output.as_mut_ptr();
    let mut op = unsafe { out_base.add(output.len()) };
    let op_limit = unsafe { out_base.add(output.capacity() - WILDCOPY_OVERLENGTH) };

    let of_tbl = tables.of_table.as_ptr();
    let ml_tbl = tables.ml_table.as_ptr();
    let ll_tbl = tables.ll_table.as_ptr();
    let of_mask = ((1u32 << tables.of_accuracy) - 1) as usize;
    let ml_mask = ((1u32 << tables.ml_accuracy) - 1) as usize;
    let ll_mask = ((1u32 << tables.ll_accuracy) - 1) as usize;
    let lit_ptr = literals.as_ptr();
    let lit_end = unsafe { lit_ptr.add(literals.len()) };
    let hist_len = if HAS_HISTORY { history.len() } else { 0 };
    let mut lit_pos = lit_ptr;

    let mut bs_container = rev_reader.container;
    let mut bs_consumed = rev_reader.bits_consumed;
    let seq_base = seq_data.as_ptr();
    let mut bs_ptr = unsafe { seq_base.add(rev_reader.ptr) };
    let bs_fast_limit_off = rev_reader.limit_ptr.saturating_add(16);

    let last_seq = num_sequences - 1;

    let mut rep0 = rep_offsets[0];
    let mut rep1 = rep_offsets[1];
    let mut rep2 = rep_offsets[2];

    macro_rules! read_bits {
        ($n:expr) => {{
            let r = ((bs_container << (bs_consumed & 63)) >> 1 >> (63 - $n as u32)) as u32;
            bs_consumed += $n as u32;
            r
        }};
    }

    macro_rules! refill_fast {
        () => {{
            let byte_shift = (bs_consumed >> 3) as usize;
            bs_ptr = unsafe { bs_ptr.sub(byte_shift) };
            bs_consumed -= (byte_shift as u32) * 8;
            bs_container = unsafe { (bs_ptr as *const u64).read_unaligned() };
        }};
    }

    macro_rules! compute_offset_inline {
        ($offset_value:expr, $literal_length:expr) => {{
            let ov = $offset_value;
            if ov > 3 {
                let offset = ov - 3;
                rep2 = rep1;
                rep1 = rep0;
                rep0 = offset;
                offset
            } else {
                let ll0 = ($literal_length == 0) as u32;
                let rep_idx = ov - 1 + ll0;
                let offset = if rep_idx == 0 {
                    rep0
                } else if rep_idx == 1 {
                    rep1
                } else if rep_idx == 2 {
                    rep2
                } else {
                    rep0.wrapping_sub(1)
                };
                if rep_idx >= 2 {
                    rep2 = rep1;
                }
                if rep_idx >= 1 {
                    rep1 = rep0;
                }
                rep0 = offset;
                offset
            }
        }};
    }

    macro_rules! decode_and_update {
        () => {{
            refill_fast!();

            let of_e = unsafe { *of_tbl.add(of_state as usize & of_mask) };
            let ml_e = unsafe { *ml_tbl.add(ml_state as usize & ml_mask) };
            let ll_e = unsafe { *ll_tbl.add(ll_state as usize & ll_mask) };

            let offset_value = of_e.baseline_value + read_bits!(of_e.extra_bits);
            let match_length = ml_e.baseline_value + read_bits!(ml_e.extra_bits);
            let literal_length = ll_e.baseline_value + read_bits!(ll_e.extra_bits);

            let offset = compute_offset_inline!(offset_value, literal_length);

            refill_fast!();
            ll_state = ll_e.base_line as u32 + read_bits!(ll_e.num_bits);
            ml_state = ml_e.base_line as u32 + read_bits!(ml_e.num_bits);
            of_state = of_e.base_line as u32 + read_bits!(of_e.num_bits);

            (literal_length, match_length, offset)
        }};
    }

    macro_rules! execute_seq {
        ($literal_length:expr, $match_length:expr, $offset:expr) => {{
            let ll = $literal_length as usize;
            let ml = $match_length as usize;
            if unlikely(unsafe { op.add(ll + ml) } > op_limit) {
                return Err(DecompressError::CorruptSequences);
            }
            let lit_remaining = unsafe { lit_end.offset_from(lit_pos) } as usize;
            if unlikely(ll > lit_remaining) {
                return Err(DecompressError::CorruptSequences);
            }
            unsafe {
                if lit_remaining >= 32 {
                    let chunk = _mm256_loadu_si256(lit_pos as *const __m256i);
                    _mm256_storeu_si256(op as *mut __m256i, chunk);
                    if ll > 32 {
                        core::ptr::copy_nonoverlapping(lit_pos.add(32), op.add(32), ll - 32);
                    }
                } else if ll > 0 {
                    core::ptr::copy_nonoverlapping(lit_pos, op, ll);
                }
                op = op.add(ll);
                lit_pos = lit_pos.add(ll);
            }

            let offset = $offset;
            if unlikely(offset == 0) {
                return Err(DecompressError::InvalidOffset);
            }
            let off = offset as usize;
            let out_pos = unsafe { op.offset_from(out_base) } as usize;
            if unlikely(off > out_pos + hist_len) {
                return Err(DecompressError::InvalidOffset);
            }
            if unlikely(ml == 0) {
                return Err(DecompressError::CorruptSequences);
            }
            unsafe {
                if !HAS_HISTORY || likely(off <= out_pos) {
                    if off >= 32 {
                        let mut s = op.sub(off);
                        let mut d = op;
                        let end = op.add(ml);
                        loop {
                            let chunk = _mm256_loadu_si256(s as *const __m256i);
                            _mm256_storeu_si256(d as *mut __m256i, chunk);
                            s = s.add(32);
                            d = d.add(32);
                            if d >= end {
                                break;
                            }
                        }
                    } else if off >= 8 {
                        let s = op.sub(off);
                        (op as *mut u64).write_unaligned((s as *const u64).read_unaligned());
                        (op.add(8) as *mut u64)
                            .write_unaligned((s.add(8) as *const u64).read_unaligned());
                        if ml > 16 {
                            let mut cs = s.add(16);
                            let mut cd = op.add(16);
                            let end = op.add(ml);
                            while cd < end {
                                (cd as *mut u64)
                                    .write_unaligned((cs as *const u64).read_unaligned());
                                cs = cs.add(8);
                                cd = cd.add(8);
                            }
                        }
                    } else {
                        zrip_core::simd::scalar::copy_match(op, off, ml);
                    }
                } else {
                    copy_match_from_history(op, out_base, history, off, out_pos, ml);
                }
                op = op.add(ml);
            }
        }};
    }

    let mut remaining = last_seq;
    while remaining > 0 && unsafe { bs_ptr.offset_from(seq_base) } as usize >= bs_fast_limit_off {
        let (ll, ml, off) = decode_and_update!();
        execute_seq!(ll, ml, off);
        remaining -= 1;
    }

    // Slow path: restore ReverseBitReader for checked refills
    rev_reader.container = bs_container;
    rev_reader.bits_consumed = bs_consumed;
    rev_reader.ptr = unsafe { bs_ptr.offset_from(seq_data.as_ptr()) } as usize;

    while remaining > 0 {
        rev_reader.refill();

        let of_e = unsafe { *of_tbl.add(of_state as usize & of_mask) };
        let ml_e = unsafe { *ml_tbl.add(ml_state as usize & ml_mask) };
        let ll_e = unsafe { *ll_tbl.add(ll_state as usize & ll_mask) };

        let of_extra = rev_reader.read_bits_branchless(of_e.extra_bits);
        let offset_value = of_e.baseline_value + of_extra;
        let ml_extra = rev_reader.read_bits_branchless(ml_e.extra_bits);
        let match_length = ml_e.baseline_value + ml_extra;
        let ll_extra = rev_reader.read_bits_branchless(ll_e.extra_bits);
        let literal_length = ll_e.baseline_value + ll_extra;
        let offset = compute_offset_inline!(offset_value, literal_length);

        rev_reader.refill();
        ll_state = ll_e.base_line as u32 + rev_reader.read_bits_branchless(ll_e.num_bits);
        ml_state = ml_e.base_line as u32 + rev_reader.read_bits_branchless(ml_e.num_bits);
        of_state = of_e.base_line as u32 + rev_reader.read_bits_branchless(of_e.num_bits);

        execute_seq!(literal_length, match_length, offset);
        remaining -= 1;
    }

    // Last sequence
    if num_sequences > 0 {
        rev_reader.refill();
        let of_e = unsafe { *of_tbl.add(of_state as usize & of_mask) };
        let ml_e = unsafe { *ml_tbl.add(ml_state as usize & ml_mask) };
        let ll_e = unsafe { *ll_tbl.add(ll_state as usize & ll_mask) };
        let offset_value = of_e.baseline_value + rev_reader.read_bits_branchless(of_e.extra_bits);
        let match_length = ml_e.baseline_value + rev_reader.read_bits_branchless(ml_e.extra_bits);
        let literal_length = ll_e.baseline_value + rev_reader.read_bits_branchless(ll_e.extra_bits);
        let offset = compute_offset_inline!(offset_value, literal_length);
        execute_seq!(literal_length, match_length, offset);
    }

    rep_offsets[0] = rep0;
    rep_offsets[1] = rep1;
    rep_offsets[2] = rep2;

    // Trailing literals
    if lit_pos < lit_end {
        let remaining = unsafe { lit_end.offset_from(lit_pos) } as usize;
        if unsafe { op.add(remaining) } > unsafe { out_base.add(output.capacity()) } {
            return Err(DecompressError::CorruptSequences);
        }
        unsafe {
            core::ptr::copy_nonoverlapping(lit_pos, op, remaining);
            op = op.add(remaining);
        }
    }

    let new_len = unsafe { op.offset_from(out_base) } as usize;
    if new_len > output.capacity() {
        return Err(DecompressError::CorruptSequences);
    }
    unsafe {
        output.set_len(new_len);
    }

    Ok(())
}

/// # Safety
/// AVX2 and BMI2 must be available.
#[target_feature(enable = "avx2,bmi2")]
pub unsafe fn decode_execute_avx2(
    seq_data: &[u8],
    num_sequences: u32,
    tables: &SequenceDecodeTables,
    rep_offsets: &mut [u32; 3],
    literals: &[u8],
    output: &mut Vec<u8>,
    history: &[u8],
) -> Result<(), DecompressError> {
    if history.is_empty() {
        unsafe {
            decode_execute_avx2_inner::<false>(
                seq_data,
                num_sequences,
                tables,
                rep_offsets,
                literals,
                output,
                history,
            )
        }
    } else {
        unsafe {
            decode_execute_avx2_with_history(
                seq_data,
                num_sequences,
                tables,
                rep_offsets,
                literals,
                output,
                history,
            )
        }
    }
}

#[target_feature(enable = "avx2,bmi2")]
#[inline(never)]
unsafe fn decode_execute_avx2_with_history(
    seq_data: &[u8],
    num_sequences: u32,
    tables: &SequenceDecodeTables,
    rep_offsets: &mut [u32; 3],
    literals: &[u8],
    output: &mut Vec<u8>,
    history: &[u8],
) -> Result<(), DecompressError> {
    unsafe {
        decode_execute_avx2_inner::<true>(
            seq_data,
            num_sequences,
            tables,
            rep_offsets,
            literals,
            output,
            history,
        )
    }
}

#[inline(always)]
unsafe fn copy_match_from_history(
    op: *mut u8,
    _out_base: *const u8,
    history: &[u8],
    offset: usize,
    out_pos: usize,
    match_length: usize,
) {
    let history_reach = offset - out_pos;
    let history_start = history.len() - history_reach;
    let from_history = history_reach.min(match_length);
    unsafe {
        core::ptr::copy_nonoverlapping(history.as_ptr().add(history_start), op, from_history);
    }
    let remaining = match_length - from_history;
    if remaining > 0 {
        unsafe {
            zrip_core::simd::x86_64::avx2::copy_match_avx2(op.add(from_history), offset, remaining);
        }
    }
}

/// Safe wrapper around `decode_execute_avx2`.
/// Caller must have verified AVX2+BMI2 availability via `CpuTier::Avx2`.
pub(crate) fn decode_execute_avx2_safe(
    seq_data: &[u8],
    num_sequences: u32,
    tables: &SequenceDecodeTables,
    rep_offsets: &mut [u32; 3],
    literals: &[u8],
    output: &mut Vec<u8>,
    history: &[u8],
) -> Result<(), DecompressError> {
    unsafe {
        decode_execute_avx2(
            seq_data,
            num_sequences,
            tables,
            rep_offsets,
            literals,
            output,
            history,
        )
    }
}

#[inline(always)]
fn init_state(
    _table: &[FseSeqDecodeEntry],
    accuracy_log: u8,
    reader: &mut ReverseBitReader,
) -> Result<u32, DecompressError> {
    reader.read_bits(accuracy_log)
}
