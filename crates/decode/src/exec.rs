#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::fast_vec::BlockOutput;
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

    let mut output = BlockOutput::new(output, zrip_core::frame::MAX_BLOCK_SIZE);
    let mut op = output.len();
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
            let mut seq_output = output.begin_sequence(ll, ml)?;
            if ll != 0 {
                seq_output.extend_literals_range(literals, lit_off)?;
                op += ll;
                lit_off += ll;
            }

            let off = $offset as usize;
            let out_pos = op;
            if HAS_HISTORY {
                if likely(off <= out_pos) {
                    seq_output.copy_match(off)?;
                } else {
                    seq_output.copy_match_from_history(history, off, out_pos)?;
                }
            } else {
                if likely(off >= 16) {
                    seq_output.copy_match_16plus(off)?;
                } else {
                    seq_output.copy_match(off)?;
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
        output.extend_literals_range(literals, lit_off, remaining)?;
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

    let mut output = BlockOutput::new(output, zrip_core::frame::MAX_BLOCK_SIZE);

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
    {
        let mut seq_output = output.begin_sequence(ll, ml)?;
        seq_output.extend_literals_range(literals, 0)?;

        let off = offset as usize;
        let out_pos = seq_output.len();
        if HAS_HISTORY {
            if likely(off <= out_pos) {
                seq_output.copy_match_single(off)?;
            } else {
                seq_output.copy_match_from_history(history, off, out_pos)?;
            }
        } else {
            seq_output.copy_match_single(off)?;
        }
    }

    if rev_reader.bits_remaining() != 0 {
        return Err(DecompressError::CorruptSequences);
    }

    if ll < literals.len() {
        let remaining = literals.len() - ll;
        output.extend_literals_range(literals, ll, remaining)?;
    }

    Ok(())
}
