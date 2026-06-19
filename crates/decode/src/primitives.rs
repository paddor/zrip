#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use zrip_core::fse::FseSeqDecodeEntry;

#[inline(always)]
pub(crate) fn fse_table_lookup(table: &[FseSeqDecodeEntry], idx: usize) -> FseSeqDecodeEntry {
    debug_assert!(idx < table.len());
    unsafe { *table.get_unchecked(idx) }
}

#[inline(always)]
pub(crate) fn ptr_add_mut(base: *mut u8, offset: usize) -> *mut u8 {
    unsafe { base.add(offset) }
}

#[inline(always)]
pub(crate) fn ptr_add_const(base: *const u8, offset: usize) -> *const u8 {
    unsafe { base.add(offset) }
}

#[inline(always)]
pub(crate) fn ptr_gt(a: *const u8, b: *const u8) -> bool {
    a > b
}

#[inline(always)]
pub(crate) fn ptr_offset_from_mut(a: *mut u8, b: *mut u8) -> usize {
    debug_assert!(a >= b);
    unsafe { a.offset_from(b) as usize }
}

#[inline(always)]
pub(crate) fn output_write_16(dst: *mut u8, src: *const u8) {
    unsafe {
        (dst as *mut u64).write_unaligned((src as *const u64).read_unaligned());
        (dst.add(8) as *mut u64).write_unaligned((src.add(8) as *const u64).read_unaligned());
    }
}

#[inline(always)]
pub(crate) fn output_copy(src: *const u8, dst: *mut u8, len: usize) {
    unsafe { core::ptr::copy_nonoverlapping(src, dst, len) }
}

#[inline(always)]
pub(crate) fn set_output_len(vec: &mut Vec<u8>, len: usize) {
    debug_assert!(len <= vec.capacity());
    unsafe { vec.set_len(len) }
}

#[inline(always)]
pub(crate) fn copy_match_from_history(
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

#[inline(always)]
pub(crate) fn copy_match_inbuf(op: *mut u8, offset: usize, match_length: usize) {
    unsafe { zrip_core::simd::scalar::copy_match(op, offset, match_length) }
}
