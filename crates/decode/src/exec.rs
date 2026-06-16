#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::sequences::{SequenceDecodeTables, compute_offset};
use zrip_core::bitstream::reader_reverse::ReverseBitReader;
use zrip_core::error::DecompressError;
use zrip_core::hint::{likely, unlikely};

pub fn decode_execute_sequences(
    data: &[u8],
    num_sequences: u32,
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

    let mut ll_state = rev_reader.read_bits(tables.ll_accuracy)?;
    let mut of_state = rev_reader.read_bits(tables.of_accuracy)?;
    let mut ml_state = rev_reader.read_bits(tables.ml_accuracy)?;

    const WILDCOPY_OVERLENGTH: usize = 64;
    output.reserve(zrip_core::frame::MAX_BLOCK_SIZE + WILDCOPY_OVERLENGTH);

    let out_base = output.as_mut_ptr();
    let mut op = unsafe { out_base.add(output.len()) };
    let op_limit = unsafe { out_base.add(output.capacity() - WILDCOPY_OVERLENGTH) };
    let lit_ptr = literals.as_ptr();
    let mut lit_off: usize = 0;
    let of_mask = ((1u32 << tables.of_accuracy) - 1) as usize;
    let ml_mask = ((1u32 << tables.ml_accuracy) - 1) as usize;
    let ll_mask = ((1u32 << tables.ll_accuracy) - 1) as usize;

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
            unsafe {
                let src = lit_ptr.add(lit_off);
                let lit_remaining = literals.len() - lit_off;
                if lit_remaining >= 16 {
                    (op as *mut u64).write_unaligned((src as *const u64).read_unaligned());
                    (op.add(8) as *mut u64)
                        .write_unaligned((src.add(8) as *const u64).read_unaligned());
                    if ll > 16 {
                        core::ptr::copy_nonoverlapping(src.add(16), op.add(16), ll - 16);
                    }
                } else {
                    core::ptr::copy_nonoverlapping(src, op, ll);
                }
            }
            op = unsafe { op.add(ll) };
            lit_off += ll;

            let ml = $match_length as usize;
            let off = $offset as usize;
            if unlikely(off == 0) {
                return Err(DecompressError::CorruptSequences);
            }
            let out_pos = unsafe { op.offset_from(out_base) } as usize;
            if unlikely(off > out_pos + history.len()) {
                return Err(DecompressError::CorruptSequences);
            }
            unsafe {
                if likely(off <= out_pos) {
                    zrip_core::simd::scalar::copy_match(op, off, ml);
                } else {
                    copy_match_from_history(op, history, off, out_pos, ml);
                }
            }
            op = unsafe { op.add(ml) };
        }};
    }

    macro_rules! decode_and_execute_update {
        ($rev_reader:expr, $offsets:expr) => {{
            let of_e = unsafe { *tables.of_table.get_unchecked(of_state as usize & of_mask) };
            let ml_e = unsafe { *tables.ml_table.get_unchecked(ml_state as usize & ml_mask) };
            let ll_e = unsafe { *tables.ll_table.get_unchecked(ll_state as usize & ll_mask) };

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
        let of_e = unsafe { *tables.of_table.get_unchecked(of_state as usize & of_mask) };
        let ml_e = unsafe { *tables.ml_table.get_unchecked(ml_state as usize & ml_mask) };
        let ll_e = unsafe { *tables.ll_table.get_unchecked(ll_state as usize & ll_mask) };

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
        let of_e = unsafe { *tables.of_table.get_unchecked(of_state as usize & of_mask) };
        let ml_e = unsafe { *tables.ml_table.get_unchecked(ml_state as usize & ml_mask) };
        let ll_e = unsafe { *tables.ll_table.get_unchecked(ll_state as usize & ll_mask) };

        let of_extra = rev_reader.read_bits_branchless(of_e.extra_bits);
        let offset_value = of_e.baseline_value + of_extra;
        let ml_extra = rev_reader.read_bits_branchless(ml_e.extra_bits);
        let match_length = ml_e.baseline_value + ml_extra;
        let ll_extra = rev_reader.read_bits_branchless(ll_e.extra_bits);
        let literal_length = ll_e.baseline_value + ll_extra;
        let offset = compute_offset(offset_value, literal_length, offsets);

        execute_seq!(literal_length, match_length, offset);
    }

    if lit_off < literals.len() {
        let remaining = literals.len() - lit_off;
        if unsafe { op.add(remaining) } > unsafe { out_base.add(output.capacity()) } {
            return Err(DecompressError::CorruptSequences);
        }
        unsafe {
            core::ptr::copy_nonoverlapping(lit_ptr.add(lit_off), op, remaining);
        }
        op = unsafe { op.add(remaining) };
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
            zrip_core::simd::scalar::copy_match(op.add(from_history), offset, remaining);
        }
    }
}
