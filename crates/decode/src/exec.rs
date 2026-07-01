#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::fast_vec::{fast_extend_from_slice, wild_copy_match};
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

            fast_extend_from_slice(output, &literals[lit_off..lit_off + ll]);
            op += ll;
            lit_off += ll;

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
                wild_copy_match(output, off, ml);
            }
            op += ml;
        }};
    }

    macro_rules! decode_and_execute_update {
        ($rev_reader:expr, $offsets:expr) => {{
            let of_e = tables.of_table[(of_state & FSE_SEQ_TABLE_MASK) as usize];
            let ml_e = tables.ml_table[(ml_state & FSE_SEQ_TABLE_MASK) as usize];
            let ll_e = tables.ll_table[(ll_state & FSE_SEQ_TABLE_MASK) as usize];

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
        let of_e = tables.of_table[(of_state & FSE_SEQ_TABLE_MASK) as usize];
        let ml_e = tables.ml_table[(ml_state & FSE_SEQ_TABLE_MASK) as usize];
        let ll_e = tables.ll_table[(ll_state & FSE_SEQ_TABLE_MASK) as usize];

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
        let of_e = tables.of_table[(of_state & FSE_SEQ_TABLE_MASK) as usize];
        let ml_e = tables.ml_table[(ml_state & FSE_SEQ_TABLE_MASK) as usize];
        let ll_e = tables.ll_table[(ll_state & FSE_SEQ_TABLE_MASK) as usize];

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
        let remaining = literals.len() - lit_off;
        if op + remaining > op_limit {
            return Err(DecompressError::CorruptSequences);
        }
        fast_extend_from_slice(output, &literals[lit_off..]);
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
