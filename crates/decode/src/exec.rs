#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::sequences::{SequenceDecodeTables, compute_offset};
use zrip_core::bitstream::reader_reverse::ReverseBitReader;
use zrip_core::error::DecompressError;
use zrip_core::hint::{likely, unlikely};

#[allow(unused_assignments)]
#[inline(always)]
pub(crate) fn decode_execute_sequences(
    data: &[u8],
    num_sequences: u32,
    tables: &SequenceDecodeTables,
    offsets: &mut [u32; 3],
    literals: &[u8],
    output: &mut Vec<u8>,
    history: &[u8],
) -> Result<(), DecompressError> {
    if num_sequences == 0 {
        return Ok(());
    }
    if data.is_empty() {
        return Err(DecompressError::CorruptSequences);
    }

    let mut rev_reader =
        ReverseBitReader::new(data).map_err(|_| DecompressError::CorruptSequences)?;

    let mut ll_state = rev_reader.read_bits(tables.ll_accuracy)?;
    let mut of_state = rev_reader.read_bits(tables.of_accuracy)?;
    let mut ml_state = rev_reader.read_bits(tables.ml_accuracy)?;

    const WILDCOPY_OVERLENGTH: usize = 64;
    output.reserve(zrip_core::frame::MAX_BLOCK_SIZE + WILDCOPY_OVERLENGTH);

    let mut op = output.len();
    let op_limit = output.capacity() - WILDCOPY_OVERLENGTH;
    let mut lit_off: usize = 0;
    let of_mask = ((1u32 << tables.of_accuracy) - 1) as usize;
    let ml_mask = ((1u32 << tables.ml_accuracy) - 1) as usize;
    let ll_mask = ((1u32 << tables.ll_accuracy) - 1) as usize;

    macro_rules! execute_seq {
        ($literal_length:expr, $match_length:expr, $offset:expr) => {{
            let ll = $literal_length as usize;
            let ml = $match_length as usize;
            if unlikely(op + ll + ml > op_limit) {
                return Err(DecompressError::CorruptSequences);
            }
            if unlikely(lit_off + ll > literals.len()) {
                return Err(DecompressError::CorruptLiterals);
            }

            // Append literals
            output.extend_from_slice(&literals[lit_off..lit_off + ll]);
            op += ll;
            lit_off += ll;

            // Match copy
            let off = $offset as usize;
            if unlikely(off == 0) {
                return Err(DecompressError::InvalidOffset);
            }
            let out_pos = op;
            if unlikely(off > out_pos + history.len()) {
                return Err(DecompressError::InvalidOffset);
            }
            if likely(off <= out_pos) {
                copy_match_safe(output, off, ml);
            } else {
                copy_match_from_history_safe(output, history, off, out_pos, ml);
            }
            op += ml;
        }};
    }

    macro_rules! decode_and_execute_update {
        ($rev_reader:expr, $offsets:expr) => {{
            let of_e = tables.of_table[of_state as usize & of_mask];
            let ml_e = tables.ml_table[ml_state as usize & ml_mask];
            let ll_e = tables.ll_table[ll_state as usize & ll_mask];

            let of_extra = $rev_reader.read_bits_branchless(of_e.extra_bits);
            let offset_value = of_e.baseline_value + of_extra;

            let ml_extra = $rev_reader.read_bits_branchless(ml_e.extra_bits);
            let match_length = ml_e.baseline_value + ml_extra;

            let ll_extra = $rev_reader.read_bits_branchless(ll_e.extra_bits);
            let literal_length = ll_e.baseline_value + ll_extra;

            let offset = compute_offset(offset_value, literal_length, $offsets);

            $rev_reader.refill_fast();
            ll_state = ll_e.base_line as u32 + $rev_reader.read_bits_branchless(ll_e.num_bits);
            ml_state = ml_e.base_line as u32 + $rev_reader.read_bits_branchless(ml_e.num_bits);
            of_state = of_e.base_line as u32 + $rev_reader.read_bits_branchless(of_e.num_bits);

            execute_seq!(literal_length, match_length, offset);
        }};
    }

    let last_seq = num_sequences - 1;
    let mut seq_idx: u32 = 0;
    let fast_limit = rev_reader.limit_ptr + 16;
    while seq_idx + 2 <= last_seq && rev_reader.ptr >= fast_limit {
        rev_reader.refill_fast();
        decode_and_execute_update!(rev_reader, offsets);

        rev_reader.refill_fast();
        decode_and_execute_update!(rev_reader, offsets);

        seq_idx += 2;
    }
    while seq_idx < last_seq && rev_reader.ptr >= fast_limit {
        rev_reader.refill_fast();
        decode_and_execute_update!(rev_reader, offsets);
        seq_idx += 1;
    }
    while seq_idx < last_seq {
        rev_reader.refill();
        let of_e = tables.of_table[of_state as usize & of_mask];
        let ml_e = tables.ml_table[ml_state as usize & ml_mask];
        let ll_e = tables.ll_table[ll_state as usize & ll_mask];

        let of_extra = rev_reader.read_bits_branchless(of_e.extra_bits);
        let offset_value = of_e.baseline_value + of_extra;
        let ml_extra = rev_reader.read_bits_branchless(ml_e.extra_bits);
        let match_length = ml_e.baseline_value + ml_extra;
        let ll_extra = rev_reader.read_bits_branchless(ll_e.extra_bits);
        let literal_length = ll_e.baseline_value + ll_extra;
        let offset = compute_offset(offset_value, literal_length, offsets);

        rev_reader.refill();
        ll_state = ll_e.base_line as u32 + rev_reader.read_bits_branchless(ll_e.num_bits);
        ml_state = ml_e.base_line as u32 + rev_reader.read_bits_branchless(ml_e.num_bits);
        of_state = of_e.base_line as u32 + rev_reader.read_bits_branchless(of_e.num_bits);

        execute_seq!(literal_length, match_length, offset);
        seq_idx += 1;
    }

    // Last sequence: no FSE state update
    {
        rev_reader.refill();
        let of_e = tables.of_table[of_state as usize & of_mask];
        let ml_e = tables.ml_table[ml_state as usize & ml_mask];
        let ll_e = tables.ll_table[ll_state as usize & ll_mask];

        let of_extra = rev_reader.read_bits_branchless(of_e.extra_bits);
        let offset_value = of_e.baseline_value + of_extra;
        let ml_extra = rev_reader.read_bits_branchless(ml_e.extra_bits);
        let match_length = ml_e.baseline_value + ml_extra;
        let ll_extra = rev_reader.read_bits_branchless(ll_e.extra_bits);
        let literal_length = ll_e.baseline_value + ll_extra;
        let offset = compute_offset(offset_value, literal_length, offsets);

        execute_seq!(literal_length, match_length, offset);
    }

    if rev_reader.bits_remaining() != 0 {
        return Err(DecompressError::CorruptSequences);
    }

    if lit_off < literals.len() {
        output.extend_from_slice(&literals[lit_off..]);
    }

    Ok(())
}

#[inline(always)]
fn copy_match_safe(output: &mut Vec<u8>, offset: usize, match_length: usize) {
    let start = output.len() - offset;
    if offset >= match_length {
        output.extend_from_within(start..start + match_length);
    } else {
        let mut remaining = match_length;
        while remaining > 0 {
            let n = remaining.min(offset);
            output.extend_from_within(start..start + n);
            remaining -= n;
        }
    }
}

#[inline(always)]
fn copy_match_from_history_safe(
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
        copy_match_safe(output, offset, remaining);
    }
}

#[cfg(all(target_arch = "x86_64", not(feature = "paranoid")))]
#[target_feature(enable = "avx2,bmi2")]
pub(crate) fn decode_execute_sequences_avx2(
    data: &[u8],
    num_sequences: u32,
    tables: &SequenceDecodeTables,
    offsets: &mut [u32; 3],
    literals: &[u8],
    output: &mut Vec<u8>,
    history: &[u8],
) -> Result<(), DecompressError> {
    decode_execute_sequences(
        data,
        num_sequences,
        tables,
        offsets,
        literals,
        output,
        history,
    )
}

#[cfg(all(target_arch = "aarch64", not(feature = "paranoid")))]
#[target_feature(enable = "neon")]
pub(crate) fn decode_execute_sequences_neon(
    data: &[u8],
    num_sequences: u32,
    tables: &SequenceDecodeTables,
    offsets: &mut [u32; 3],
    literals: &[u8],
    output: &mut Vec<u8>,
    history: &[u8],
) -> Result<(), DecompressError> {
    decode_execute_sequences(
        data,
        num_sequences,
        tables,
        offsets,
        literals,
        output,
        history,
    )
}
