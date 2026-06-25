#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use safe_unaligned_simd::x86_64::{_mm256_loadu_si256, _mm256_storeu_si256};

use crate::sequences::SequenceDecodeTables;
use zrip_core::bitstream::reader_reverse::ReverseBitReader;
use zrip_core::error::DecompressError;
use zrip_core::fse::FseSeqDecodeEntry;
use zrip_core::hint::{likely, unlikely};

#[target_feature(enable = "avx2,bmi2")]
fn decode_execute_avx2_inner<const HAS_HISTORY: bool>(
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

    let output_start_len = output.len();
    let result = (|| -> Result<(), DecompressError> {
        let mut rev_reader =
            ReverseBitReader::new(seq_data).map_err(|_| DecompressError::CorruptSequences)?;

        let mut ll_state = init_state(&tables.ll_table, tables.ll_accuracy, &mut rev_reader)?;
        let mut of_state = init_state(&tables.of_table, tables.of_accuracy, &mut rev_reader)?;
        let mut ml_state = init_state(&tables.ml_table, tables.ml_accuracy, &mut rev_reader)?;

        let max_output = zrip_core::frame::MAX_BLOCK_SIZE;
        let output_limit = output_start_len
            .checked_add(max_output)
            .ok_or(DecompressError::CorruptSequences)?;
        output.reserve(max_output);

        let of_tbl = tables.of_table.as_ptr();
        let ml_tbl = tables.ml_table.as_ptr();
        let ll_tbl = tables.ll_table.as_ptr();
        let of_mask = ((1u32 << tables.of_accuracy) - 1) as usize;
        let ml_mask = ((1u32 << tables.ml_accuracy) - 1) as usize;
        let ll_mask = ((1u32 << tables.ll_accuracy) - 1) as usize;
        let hist_len = if HAS_HISTORY { history.len() } else { 0 };
        let mut lit_pos = 0;

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
                let seq_end = output
                    .len()
                    .checked_add(ll)
                    .and_then(|len| len.checked_add(ml))
                    .ok_or(DecompressError::CorruptSequences)?;
                if unlikely(seq_end > output_limit) {
                    return Err(DecompressError::CorruptSequences);
                }
                let lit_remaining = literals.len() - lit_pos;
                if unlikely(ll > lit_remaining) {
                    return Err(DecompressError::CorruptSequences);
                }
                extend_literals_avx2(literals, &mut lit_pos, ll, output);

                let offset = $offset;
                if unlikely(offset == 0) {
                    return Err(DecompressError::InvalidOffset);
                }
                let off = offset as usize;
                let out_pos = output.len();
                if unlikely(off > out_pos.saturating_add(hist_len)) {
                    return Err(DecompressError::InvalidOffset);
                }
                if unlikely(ml == 0) {
                    return Err(DecompressError::CorruptSequences);
                }
                if !HAS_HISTORY || likely(off <= out_pos) {
                    copy_match_from_output_avx2(output, off, ml);
                } else {
                    copy_match_from_history(output, history, off, out_pos, ml);
                }
            }};
        }

        let mut remaining = last_seq;
        while remaining > 0 && unsafe { bs_ptr.offset_from(seq_base) } as usize >= bs_fast_limit_off
        {
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
            let offset_value =
                of_e.baseline_value + rev_reader.read_bits_branchless(of_e.extra_bits);
            let match_length =
                ml_e.baseline_value + rev_reader.read_bits_branchless(ml_e.extra_bits);
            let literal_length =
                ll_e.baseline_value + rev_reader.read_bits_branchless(ll_e.extra_bits);
            let offset = compute_offset_inline!(offset_value, literal_length);
            execute_seq!(literal_length, match_length, offset);
        }

        if rev_reader.bits_remaining() != 0 {
            return Err(DecompressError::CorruptSequences);
        }

        rep_offsets[0] = rep0;
        rep_offsets[1] = rep1;
        rep_offsets[2] = rep2;

        // Trailing literals
        if lit_pos < literals.len() {
            let remaining = literals.len() - lit_pos;
            let end = output
                .len()
                .checked_add(remaining)
                .ok_or(DecompressError::CorruptSequences)?;
            if end > output_limit {
                return Err(DecompressError::CorruptSequences);
            }
            extend_literals_avx2(literals, &mut lit_pos, remaining, output);
        }

        Ok(())
    })();

    if result.is_err() {
        output.truncate(output_start_len);
    }

    result
}

/// AVX2 and BMI2 must be available before calling.
#[target_feature(enable = "avx2,bmi2")]
pub fn decode_execute_avx2(
    seq_data: &[u8],
    num_sequences: u32,
    tables: &SequenceDecodeTables,
    rep_offsets: &mut [u32; 3],
    literals: &[u8],
    output: &mut Vec<u8>,
    history: &[u8],
) -> Result<(), DecompressError> {
    if history.is_empty() {
        decode_execute_avx2_inner::<false>(
            seq_data,
            num_sequences,
            tables,
            rep_offsets,
            literals,
            output,
            history,
        )
    } else {
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

#[target_feature(enable = "avx2,bmi2")]
#[inline(never)]
fn decode_execute_avx2_with_history(
    seq_data: &[u8],
    num_sequences: u32,
    tables: &SequenceDecodeTables,
    rep_offsets: &mut [u32; 3],
    literals: &[u8],
    output: &mut Vec<u8>,
    history: &[u8],
) -> Result<(), DecompressError> {
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

#[target_feature(enable = "avx2,bmi2")]
#[inline]
fn extend_literals_avx2(literals: &[u8], lit_pos: &mut usize, len: usize, output: &mut Vec<u8>) {
    debug_assert!(*lit_pos + len <= literals.len());

    if len >= 32 {
        let chunk = _mm256_loadu_si256(simd_chunk(&literals[*lit_pos..]));
        let mut bytes = [0u8; 32];
        _mm256_storeu_si256(&mut bytes, chunk);
        output.extend_from_slice(&bytes);

        let rest_start = *lit_pos + 32;
        let end = *lit_pos + len;
        if rest_start < end {
            output.extend_from_slice(&literals[rest_start..end]);
        }
    } else if len > 0 {
        output.extend_from_slice(&literals[*lit_pos..*lit_pos + len]);
    }

    *lit_pos += len;
}

#[target_feature(enable = "avx2,bmi2")]
#[inline]
fn copy_match_from_output_avx2(output: &mut Vec<u8>, offset: usize, mut len: usize) {
    debug_assert!(offset > 0);
    debug_assert!(offset <= output.len());
    debug_assert!(len > 0);

    if offset >= 32 {
        while len > 0 {
            let src = output.len() - offset;
            let chunk = _mm256_loadu_si256(simd_chunk(&output[src..]));
            let mut bytes = [0u8; 32];
            _mm256_storeu_si256(&mut bytes, chunk);
            let n = len.min(32);
            output.extend_from_slice(&bytes[..n]);
            len -= n;
        }
    } else if offset >= 8 {
        while len > 0 {
            let src = output.len() - offset;
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&output[src..src + 8]);
            let n = len.min(8);
            output.extend_from_slice(&bytes[..n]);
            len -= n;
        }
    } else if offset == 1 {
        let byte = *output.last().expect("offset checked above");
        output.resize(output.len() + len, byte);
    } else {
        while len > 0 {
            let src = output.len() - offset;
            let n = len.min(offset);
            let mut bytes = [0u8; 8];
            bytes[..n].copy_from_slice(&output[src..src + n]);
            output.extend_from_slice(&bytes[..n]);
            len -= n;
        }
    }
}

#[inline(always)]
fn simd_chunk(bytes: &[u8]) -> &[u8; 32] {
    bytes[..32].try_into().expect("slice has at least 32 bytes")
}

#[target_feature(enable = "avx2,bmi2")]
#[inline]
fn copy_match_from_history(
    output: &mut Vec<u8>,
    history: &[u8],
    offset: usize,
    out_pos: usize,
    match_length: usize,
) {
    let history_reach = offset - out_pos;
    let history_start = history.len() - history_reach;
    let from_history = history_reach.min(match_length);
    output.extend_from_slice(&history[history_start..history_start + from_history]);

    let remaining = match_length - from_history;
    if remaining > 0 {
        copy_match_from_output_avx2(output, offset, remaining);
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
