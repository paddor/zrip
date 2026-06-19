#[cfg(feature = "alloc")]
use alloc::vec::Vec;

#[inline(always)]
pub(crate) fn rd32(src: &[u8], pos: usize) -> u32 {
    debug_assert!(pos + 4 <= src.len());
    unsafe { (src.as_ptr().add(pos) as *const u32).read_unaligned() }
}

#[inline(always)]
pub(crate) fn rd64(src: &[u8], pos: usize) -> u64 {
    debug_assert!(pos + 8 <= src.len());
    unsafe { (src.as_ptr().add(pos) as *const u64).read_unaligned() }
}

#[inline(always)]
pub(crate) fn src_byte(src: &[u8], pos: usize) -> u8 {
    debug_assert!(pos < src.len());
    unsafe { *src.get_unchecked(pos) }
}

#[inline(always)]
pub(crate) fn hash_load(table: &[u32], idx: usize) -> u32 {
    debug_assert!(idx < table.len());
    unsafe { *table.get_unchecked(idx) }
}

#[inline(always)]
pub(crate) fn hash_store(table: &mut [u32], idx: usize, val: u32) {
    debug_assert!(idx < table.len());
    unsafe { *table.get_unchecked_mut(idx) = val }
}

#[inline(always)]
pub(crate) fn match_at<const MLS: usize>(src: &[u8], a: usize, b: usize) -> bool {
    if MLS >= 7 {
        rd64(src, a) == rd64(src, b)
    } else if MLS >= 5 {
        let va = rd64(src, a);
        let vb = rd64(src, b);
        ((va ^ vb) << (64 - 8 * MLS)) == 0
    } else {
        rd32(src, a) == rd32(src, b)
    }
}

#[inline(always)]
pub(crate) fn count_match(src: &[u8], p1: usize, p2: usize, limit: usize) -> usize {
    debug_assert!(p1 <= limit && limit <= src.len());
    debug_assert!(p2 < src.len());
    let src_ptr = src.as_ptr();
    unsafe { count_match_raw(src_ptr.add(p1), src_ptr.add(p2), src_ptr.add(limit)) }
}

#[inline(always)]
unsafe fn count_match_raw(
    mut p_in: *const u8,
    mut p_match: *const u8,
    p_in_limit: *const u8,
) -> usize {
    unsafe {
        let p_start = p_in;
        let safe_limit = p_in_limit.sub(7);

        while p_in < safe_limit {
            let diff =
                (p_in as *const u64).read_unaligned() ^ (p_match as *const u64).read_unaligned();
            if diff != 0 {
                return p_in.offset_from(p_start) as usize + (diff.trailing_zeros() >> 3) as usize;
            }
            p_in = p_in.add(8);
            p_match = p_match.add(8);
        }
        while p_in < p_in_limit {
            if *p_in != *p_match {
                break;
            }
            p_in = p_in.add(1);
            p_match = p_match.add(1);
        }
        p_in.offset_from(p_start) as usize
    }
}

#[inline(always)]
pub(crate) fn assert_rep_valid(r0: u32, r1: u32) {
    debug_assert!(r0 > 0);
    debug_assert!(r1 > 0);
    unsafe {
        core::hint::assert_unchecked(r0 > 0);
        core::hint::assert_unchecked(r1 > 0);
    }
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub(crate) fn prefetch_ht(table: &[u32], idx: usize) {
    unsafe {
        core::arch::x86_64::_mm_prefetch(
            table.as_ptr().add(idx) as *const i8,
            core::arch::x86_64::_MM_HINT_T0,
        );
    }
}

#[inline(always)]
pub(crate) fn slice_get<T: Copy>(slice: &[T], idx: usize) -> T {
    debug_assert!(idx < slice.len());
    unsafe { *slice.get_unchecked(idx) }
}

#[inline(always)]
pub(crate) fn slice_get_ref<T>(slice: &[T], idx: usize) -> &T {
    debug_assert!(idx < slice.len());
    unsafe { slice.get_unchecked(idx) }
}

#[inline(always)]
pub(crate) fn copy_literals_fast(
    src: &[u8],
    src_off: usize,
    dst: &mut Vec<u8>,
    dst_off: usize,
    len: usize,
) {
    debug_assert!(src_off + len <= src.len());
    debug_assert!(dst_off + len <= dst.capacity());
    unsafe {
        let s = src.as_ptr().add(src_off);
        let d = dst.as_mut_ptr().add(dst_off);
        if len <= 16 && src_off + 16 <= src.len() {
            debug_assert!(dst_off + 16 <= dst.capacity());
            (d as *mut u64).write_unaligned((s as *const u64).read_unaligned());
            (d.add(8) as *mut u64).write_unaligned((s.add(8) as *const u64).read_unaligned());
        } else {
            core::ptr::copy_nonoverlapping(s, d, len);
        }
    }
}

#[inline(always)]
pub(crate) fn bitstream_flush(buf: &mut Vec<u8>, pos: usize, bits: u64) {
    debug_assert!(pos + 8 <= buf.capacity());
    unsafe {
        (buf.as_mut_ptr().add(pos) as *mut u64).write_unaligned(bits.to_le());
    }
}

#[inline(always)]
pub(crate) fn bitstream_write_byte(buf: &mut Vec<u8>, pos: usize, val: u8) {
    debug_assert!(pos < buf.capacity());
    unsafe { *buf.as_mut_ptr().add(pos) = val }
}

#[inline(always)]
pub(crate) fn vec_write_at<T>(vec: &mut Vec<T>, idx: usize, val: T) {
    debug_assert!(idx < vec.capacity());
    unsafe { vec.as_mut_ptr().add(idx).write(val) }
}

#[inline(always)]
pub(crate) fn set_vec_len<T>(vec: &mut Vec<T>, len: usize) {
    debug_assert!(len <= vec.capacity());
    unsafe { vec.set_len(len) }
}
