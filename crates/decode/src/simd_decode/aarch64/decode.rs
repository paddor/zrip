#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::sequences::{Sequence, SequenceDecodeTables, compute_offset};
use zrip_core::bitstream::reader_reverse::ReverseBitReader;
use zrip_core::error::DecompressError;
use zrip_core::fse::FseSeqDecodeEntry;
use zrip_core::hint::{likely, unlikely};

/// Fused sequence decode + execute with NEON wildcopy.
/// Uses raw `op` pointer throughout the loop to eliminate per-sequence Vec overhead.
///
/// # Safety
/// Must be called on aarch64 with NEON available (always true on ARMv8-A).
pub unsafe fn decode_execute_neon(
    seq_data: &[u8],
    num_sequences: u32,
    tables: &SequenceDecodeTables,
    rep_offsets: &mut [u32; 3],
    literals: &[u8],
    output: &mut Vec<u8>,
    history: &[u8],
) -> Result<(), DecompressError> {
    if seq_data.is_empty() {
        return Err(DecompressError::CorruptSequences);
    }

    let mut rev_reader =
        ReverseBitReader::new(seq_data).map_err(|_| DecompressError::CorruptSequences)?;

    let mut ll_state = init_state(&tables.ll_table, tables.ll_accuracy, &mut rev_reader)?;
    let mut of_state = init_state(&tables.of_table, tables.of_accuracy, &mut rev_reader)?;
    let mut ml_state = init_state(&tables.ml_table, tables.ml_accuracy, &mut rev_reader)?;

    const WILDCOPY_OVERLENGTH: usize = 32;
    output.reserve(zrip_core::frame::MAX_BLOCK_SIZE + WILDCOPY_OVERLENGTH);

    let out_base = output.as_mut_ptr();
    let mut op = unsafe { out_base.add(output.len()) };
    let op_limit = unsafe { out_base.add(output.len() + zrip_core::frame::MAX_BLOCK_SIZE) };
    let lit_ptr = literals.as_ptr();
    let mut lit_off: usize = 0;

    macro_rules! decode_fields {
        ($rev_reader:expr, $rep_offsets:expr) => {{
            let of_e = tables.of_table[of_state as usize];
            let ml_e = tables.ml_table[ml_state as usize];
            let ll_e = tables.ll_table[ll_state as usize];

            let of_extra = $rev_reader.read_bits_branchless(of_e.extra_bits);
            let offset_value = of_e.baseline_value + of_extra;

            let ml_extra = $rev_reader.read_bits_branchless(ml_e.extra_bits);
            let match_length = ml_e.baseline_value + ml_extra;

            let ll_extra = $rev_reader.read_bits_branchless(ll_e.extra_bits);
            let literal_length = ll_e.baseline_value + ll_extra;

            let offset = compute_offset(offset_value, literal_length, $rep_offsets);
            (literal_length, match_length, offset)
        }};
    }

    macro_rules! update_fse_states {
        ($rev_reader:expr) => {{
            let ll_entry = tables.ll_table[ll_state as usize];
            let ml_entry = tables.ml_table[ml_state as usize];
            let of_entry = tables.of_table[of_state as usize];

            ll_state =
                ll_entry.base_line as u32 + $rev_reader.read_bits_branchless(ll_entry.num_bits);
            ml_state =
                ml_entry.base_line as u32 + $rev_reader.read_bits_branchless(ml_entry.num_bits);
            of_state =
                of_entry.base_line as u32 + $rev_reader.read_bits_branchless(of_entry.num_bits);
        }};
    }

    macro_rules! execute_seq {
        ($literal_length:expr, $match_length:expr, $offset:expr) => {{
            let ll = $literal_length as usize;
            let ml_check = $match_length as usize;
            if unlikely(unsafe { op.add(ll + ml_check) } > op_limit) {
                return Err(DecompressError::CorruptSequences);
            }
            if unlikely(lit_off + ll > literals.len()) {
                return Err(DecompressError::CorruptLiterals);
            }
            if ll > 0 {
                unsafe {
                    let src = lit_ptr.add(lit_off);
                    (op as *mut u64).write_unaligned((src as *const u64).read_unaligned());
                    (op.add(8) as *mut u64)
                        .write_unaligned((src.add(8) as *const u64).read_unaligned());
                    if ll > 16 {
                        core::ptr::copy_nonoverlapping(src.add(16), op.add(16), ll - 16);
                    }
                }
                op = unsafe { op.add(ll) };
                lit_off += ll;
            }

            let ml = $match_length as usize;
            let offset = $offset;
            if unlikely(offset == 0) {
                return Err(DecompressError::CorruptSequences);
            }
            let off = offset as usize;
            let out_pos = unsafe { op.offset_from(out_base) } as usize;
            if unlikely(off > out_pos + history.len()) {
                return Err(DecompressError::CorruptSequences);
            }
            unsafe {
                if likely(off <= out_pos) {
                    super::neon::copy_match_neon(op, off, ml);
                } else {
                    copy_match_from_history(op, history, off, out_pos, ml);
                }
            }
            op = unsafe { op.add(ml) };
        }};
    }

    let last_seq = num_sequences - 1;
    let mut seq_idx: u32 = 0;
    let fast_limit = rev_reader.limit_ptr + 16;
    while seq_idx + 2 <= last_seq && rev_reader.ptr >= fast_limit {
        rev_reader.refill_fast();
        let (ll1, ml1, off1) = decode_fields!(rev_reader, rep_offsets);
        rev_reader.refill_fast();
        update_fse_states!(rev_reader);
        execute_seq!(ll1, ml1, off1);

        rev_reader.refill_fast();
        let (ll2, ml2, off2) = decode_fields!(rev_reader, rep_offsets);
        rev_reader.refill_fast();
        update_fse_states!(rev_reader);
        execute_seq!(ll2, ml2, off2);

        seq_idx += 2;
    }
    while seq_idx < last_seq && rev_reader.ptr >= fast_limit {
        rev_reader.refill_fast();
        let (ll, ml, off) = decode_fields!(rev_reader, rep_offsets);
        rev_reader.refill_fast();
        update_fse_states!(rev_reader);
        execute_seq!(ll, ml, off);
        seq_idx += 1;
    }
    while seq_idx < last_seq {
        rev_reader.refill();
        let (ll, ml, off) = decode_fields!(rev_reader, rep_offsets);
        rev_reader.refill();
        update_fse_states!(rev_reader);
        execute_seq!(ll, ml, off);
        seq_idx += 1;
    }

    // Last sequence: no FSE state update
    {
        rev_reader.refill();
        let (ll, ml, off) = decode_fields!(rev_reader, rep_offsets);
        execute_seq!(ll, ml, off);
    }

    if lit_off < literals.len() {
        let remaining = literals.len() - lit_off;
        unsafe {
            core::ptr::copy_nonoverlapping(lit_ptr.add(lit_off), op, remaining);
        }
        op = unsafe { op.add(remaining) };
    }

    unsafe {
        output.set_len(op.offset_from(out_base) as usize);
    }

    Ok(())
}

/// Safe wrapper around `decode_execute_neon`.
pub fn decode_execute_neon_safe(
    seq_data: &[u8],
    num_sequences: u32,
    tables: &SequenceDecodeTables,
    rep_offsets: &mut [u32; 3],
    literals: &[u8],
    output: &mut Vec<u8>,
    history: &[u8],
) -> Result<(), DecompressError> {
    unsafe {
        decode_execute_neon(
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

/// Decode sequences into a Vec (for compatibility with non-monolithic path).
///
/// # Safety
/// Must be called on aarch64.
pub unsafe fn decode_sequences_neon(
    data: &[u8],
    num_sequences: u32,
    tables: &SequenceDecodeTables,
    offsets: &mut [u32; 3],
) -> Result<Vec<Sequence>, DecompressError> {
    if data.is_empty() {
        return Err(DecompressError::CorruptSequences);
    }

    let mut rev_reader =
        ReverseBitReader::new(data).map_err(|_| DecompressError::CorruptSequences)?;

    let mut ll_state = init_state(&tables.ll_table, tables.ll_accuracy, &mut rev_reader)?;
    let mut of_state = init_state(&tables.of_table, tables.of_accuracy, &mut rev_reader)?;
    let mut ml_state = init_state(&tables.ml_table, tables.ml_accuracy, &mut rev_reader)?;

    let mut sequences = Vec::with_capacity(num_sequences as usize);

    for i in 0..num_sequences {
        let of_e = tables.of_table[of_state as usize];
        let ml_e = tables.ml_table[ml_state as usize];
        let ll_e = tables.ll_table[ll_state as usize];

        rev_reader.refill();

        let of_extra = rev_reader.read_bits_fast(of_e.extra_bits);
        let offset_value = of_e.baseline_value + of_extra;

        let ml_extra = rev_reader.read_bits_fast(ml_e.extra_bits);
        let match_length = ml_e.baseline_value + ml_extra;

        let ll_extra = rev_reader.read_bits_fast(ll_e.extra_bits);
        let literal_length = ll_e.baseline_value + ll_extra;

        let offset = compute_offset(offset_value, literal_length, offsets);

        sequences.push(Sequence {
            literal_length,
            offset,
            match_length,
        });

        if i < num_sequences - 1 {
            rev_reader.refill();

            let ll_entry = &tables.ll_table[ll_state as usize];
            ll_state = ll_entry.base_line as u32 + rev_reader.read_bits_fast(ll_entry.num_bits);

            let ml_entry = &tables.ml_table[ml_state as usize];
            ml_state = ml_entry.base_line as u32 + rev_reader.read_bits_fast(ml_entry.num_bits);

            let of_entry = &tables.of_table[of_state as usize];
            of_state = of_entry.base_line as u32 + rev_reader.read_bits_fast(of_entry.num_bits);
        }
    }

    Ok(sequences)
}

#[inline(always)]
fn init_state(
    _table: &[FseSeqDecodeEntry],
    accuracy_log: u8,
    reader: &mut ReverseBitReader,
) -> Result<u32, DecompressError> {
    reader.read_bits(accuracy_log)
}

#[inline(always)]
unsafe fn copy_match_from_history(
    op: *mut u8,
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
            super::neon::copy_match_neon(op.add(from_history), offset, remaining);
        }
    }
}
