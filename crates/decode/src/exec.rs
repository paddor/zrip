#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::fast_vec::{
    fast_extend_from_slice, fast_extend_from_slice_range, wild_copy_match, wild_copy_match_16plus,
    wild_copy_match_single,
};
use crate::sequences::{SequenceDecodeTables, compute_offset};
use zrip_core::bitstream::reader_reverse::ReverseBitReader;
use zrip_core::error::DecompressError;
use zrip_core::fse::FSE_SEQ_TABLE_MASK;
use zrip_core::hint::{likely, unlikely};

#[allow(unused_assignments)]
#[inline(always)]
pub(crate) fn decode_execute_sequences<const HAS_HISTORY: bool>(
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
    let output_start = op;
    let op_limit = output_start + zrip_core::frame::MAX_BLOCK_SIZE;
    let mut lit_off: usize = 0;
    let mut rep0 = offsets[0];
    let mut rep1 = offsets[1];
    let mut rep2 = offsets[2];
    macro_rules! table_entry {
        ($table:expr, $state:expr) => {{
            let idx = ($state & FSE_SEQ_TABLE_MASK) as usize;
            $table.get(idx)
        }};
    }
    macro_rules! compute_offset_local {
        ($offset_value:expr, $literal_length:expr) => {{
            let offset_value = $offset_value;
            if likely(offset_value > 3) {
                let offset = offset_value - 3;
                rep2 = rep1;
                rep1 = rep0;
                rep0 = offset;
                offset
            } else {
                let ll0 = ($literal_length == 0) as u32;
                let rep_idx = offset_value - 1 + ll0;
                let offset = if rep_idx < 3 {
                    match rep_idx {
                        0 => rep0,
                        1 => rep1,
                        _ => rep2,
                    }
                } else {
                    rep0.wrapping_sub(1)
                };
                let old_rep0 = rep0;
                let old_rep1 = rep1;
                let old_rep2 = rep2;
                rep2 = if rep_idx >= 2 { old_rep1 } else { old_rep2 };
                rep1 = if rep_idx >= 1 { old_rep0 } else { old_rep1 };
                rep0 = offset;
                offset
            }
        }};
    }
    macro_rules! execute_seq {
        ($literal_length:expr, $match_length:expr, $offset:expr) => {{
            let ll = $literal_length as usize;
            let ml = $match_length as usize;
            if ll == 0 {
                if unlikely(op + ml > op_limit) {
                    return Err(DecompressError::CorruptSequences);
                }
            } else {
                if unlikely(op + ll + ml > op_limit) {
                    return Err(DecompressError::CorruptSequences);
                }
                if unlikely(lit_off + ll > literals.len()) {
                    return Err(DecompressError::CorruptLiterals);
                }

                fast_extend_from_slice_range(output, literals, lit_off, ll);
                op += ll;
                lit_off += ll;
            }

            let off = $offset as usize;
            if unlikely(off == 0) {
                return Err(DecompressError::InvalidOffset);
            }
            let out_pos = op;
            if HAS_HISTORY {
                if unlikely(off > out_pos + history.len()) {
                    return Err(DecompressError::InvalidOffset);
                }
                if likely(off <= out_pos) {
                    wild_copy_match(output, off, ml);
                } else {
                    copy_match_from_history(output, history, off, out_pos, ml);
                }
            } else {
                if unlikely(off > out_pos) {
                    return Err(DecompressError::InvalidOffset);
                }
                if likely(off >= 16) {
                    wild_copy_match_16plus(output, off, ml);
                } else {
                    wild_copy_match(output, off, ml);
                }
            }
            op += ml;
        }};
    }

    macro_rules! decode_and_execute_update_fast {
        ($rev_reader:expr) => {{
            let of_e = table_entry!(tables.of_table, of_state);
            let ml_e = table_entry!(tables.ml_table, ml_state);
            let ll_e = table_entry!(tables.ll_table, ll_state);

            let of_extra = $rev_reader.read_bits_branchless(of_e.extra_bits);
            let offset_value = of_e.baseline_value + of_extra;

            let ml_extra = $rev_reader.read_bits_branchless(ml_e.extra_bits);
            let match_length = ml_e.baseline_value + ml_extra;

            let ll_extra = $rev_reader.read_bits_branchless(ll_e.extra_bits);
            let literal_length = ll_e.baseline_value + ll_extra;

            let offset = compute_offset_local!(offset_value, literal_length);

            $rev_reader.refill_fast_or_regular();
            ll_state = ll_e.base_line as u32 + $rev_reader.read_bits_branchless(ll_e.num_bits);
            ml_state = ml_e.base_line as u32 + $rev_reader.read_bits_branchless(ml_e.num_bits);
            of_state = of_e.base_line as u32 + $rev_reader.read_bits_branchless(of_e.num_bits);

            execute_seq!(literal_length, match_length, offset);
        }};
    }

    let last_seq = num_sequences - 1;
    let mut seq_idx: u32 = 0;
    let fast_limit = rev_reader.limit_ptr() + 16;
    while seq_idx + 4 <= last_seq && rev_reader.ptr() >= fast_limit {
        if unlikely(!rev_reader.try_refill_fast()) {
            break;
        }
        decode_and_execute_update_fast!(rev_reader);
        seq_idx += 1;

        if unlikely(!rev_reader.try_refill_fast()) {
            break;
        }
        decode_and_execute_update_fast!(rev_reader);
        seq_idx += 1;

        if unlikely(!rev_reader.try_refill_fast()) {
            break;
        }
        decode_and_execute_update_fast!(rev_reader);
        seq_idx += 1;

        if unlikely(!rev_reader.try_refill_fast()) {
            break;
        }
        decode_and_execute_update_fast!(rev_reader);
        seq_idx += 1;
    }
    while seq_idx + 2 <= last_seq && rev_reader.ptr() >= fast_limit {
        if unlikely(!rev_reader.try_refill_fast()) {
            break;
        }
        decode_and_execute_update_fast!(rev_reader);
        seq_idx += 1;

        if unlikely(!rev_reader.try_refill_fast()) {
            break;
        }
        decode_and_execute_update_fast!(rev_reader);
        seq_idx += 1;
    }
    while seq_idx < last_seq && rev_reader.ptr() >= fast_limit {
        if unlikely(!rev_reader.try_refill_fast()) {
            break;
        }
        decode_and_execute_update_fast!(rev_reader);
        seq_idx += 1;
    }
    while seq_idx < last_seq {
        rev_reader.refill();
        let of_e = table_entry!(tables.of_table, of_state);
        let ml_e = table_entry!(tables.ml_table, ml_state);
        let ll_e = table_entry!(tables.ll_table, ll_state);

        let of_extra = rev_reader.read_bits_branchless(of_e.extra_bits);
        let offset_value = of_e.baseline_value + of_extra;
        let ml_extra = rev_reader.read_bits_branchless(ml_e.extra_bits);
        let match_length = ml_e.baseline_value + ml_extra;
        let ll_extra = rev_reader.read_bits_branchless(ll_e.extra_bits);
        let literal_length = ll_e.baseline_value + ll_extra;
        let offset = compute_offset_local!(offset_value, literal_length);

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
        let of_e = table_entry!(tables.of_table, of_state);
        let ml_e = table_entry!(tables.ml_table, ml_state);
        let ll_e = table_entry!(tables.ll_table, ll_state);

        let of_extra = rev_reader.read_bits_branchless(of_e.extra_bits);
        let offset_value = of_e.baseline_value + of_extra;
        let ml_extra = rev_reader.read_bits_branchless(ml_e.extra_bits);
        let match_length = ml_e.baseline_value + ml_extra;
        let ll_extra = rev_reader.read_bits_branchless(ll_e.extra_bits);
        let literal_length = ll_e.baseline_value + ll_extra;
        let offset = compute_offset_local!(offset_value, literal_length);

        execute_seq!(literal_length, match_length, offset);
    }

    if rev_reader.bits_remaining() != 0 {
        return Err(DecompressError::CorruptSequences);
    }

    if lit_off < literals.len() {
        let remaining = literals.len() - lit_off;
        if op + remaining > op_limit {
            return Err(DecompressError::CorruptSequences);
        }
        fast_extend_from_slice_range(output, literals, lit_off, remaining);
    }

    offsets[0] = rep0;
    offsets[1] = rep1;
    offsets[2] = rep2;

    Ok(())
}

#[inline(always)]
pub(crate) fn decode_execute_single_sequence<const HAS_HISTORY: bool>(
    data: &[u8],
    tables: &SequenceDecodeTables,
    offsets: &mut [u32; 3],
    literals: &[u8],
    output: &mut Vec<u8>,
    history: &[u8],
) -> Result<(), DecompressError> {
    if data.is_empty() {
        return Err(DecompressError::CorruptSequences);
    }

    let mut rev_reader =
        ReverseBitReader::new(data).map_err(|_| DecompressError::CorruptSequences)?;
    let ll_state = rev_reader.read_bits(tables.ll_accuracy)?;
    let of_state = rev_reader.read_bits(tables.of_accuracy)?;
    let ml_state = rev_reader.read_bits(tables.ml_accuracy)?;

    const WILDCOPY_OVERLENGTH: usize = 64;
    output.reserve(zrip_core::frame::MAX_BLOCK_SIZE + WILDCOPY_OVERLENGTH);

    macro_rules! table_entry {
        ($table:expr, $state:expr) => {{
            let idx = ($state & FSE_SEQ_TABLE_MASK) as usize;
            $table.get(idx)
        }};
    }

    rev_reader.refill();
    let of_e = table_entry!(tables.of_table, of_state);
    let ml_e = table_entry!(tables.ml_table, ml_state);
    let ll_e = table_entry!(tables.ll_table, ll_state);

    let of_extra = rev_reader.read_bits_branchless(of_e.extra_bits);
    let offset_value = of_e.baseline_value + of_extra;
    let ml_extra = rev_reader.read_bits_branchless(ml_e.extra_bits);
    let match_length = ml_e.baseline_value + ml_extra;
    let ll_extra = rev_reader.read_bits_branchless(ll_e.extra_bits);
    let literal_length = ll_e.baseline_value + ll_extra;
    let offset = compute_offset(offset_value, literal_length, offsets);

    let ll = literal_length as usize;
    let ml = match_length as usize;
    let output_start = output.len();
    let op_limit = output_start + zrip_core::frame::MAX_BLOCK_SIZE;
    if unlikely(output_start + ll + ml > op_limit) {
        return Err(DecompressError::CorruptSequences);
    }
    if unlikely(ll > literals.len()) {
        return Err(DecompressError::CorruptLiterals);
    }

    fast_extend_from_slice(output, &literals[..ll]);

    let off = offset as usize;
    if unlikely(off == 0) {
        return Err(DecompressError::InvalidOffset);
    }
    let out_pos = output.len();
    if HAS_HISTORY {
        if unlikely(off > out_pos + history.len()) {
            return Err(DecompressError::InvalidOffset);
        }
        if likely(off <= out_pos) {
            wild_copy_match_single(output, off, ml);
        } else {
            copy_match_from_history(output, history, off, out_pos, ml);
        }
    } else {
        if unlikely(off > out_pos) {
            return Err(DecompressError::InvalidOffset);
        }
        wild_copy_match_single(output, off, ml);
    }

    if rev_reader.bits_remaining() != 0 {
        return Err(DecompressError::CorruptSequences);
    }

    if ll < literals.len() {
        let remaining = literals.len() - ll;
        if output.len() + remaining > op_limit {
            return Err(DecompressError::CorruptSequences);
        }
        fast_extend_from_slice(output, &literals[ll..]);
    }

    Ok(())
}

#[inline(always)]
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

    fast_extend_from_slice(
        output,
        &history[history_start..history_start + from_history],
    );

    let remaining = match_length - from_history;
    if remaining > 0 {
        wild_copy_match(output, offset, remaining);
    }
}
