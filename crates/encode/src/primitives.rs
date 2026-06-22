#[cfg(feature = "alloc")]
use alloc::vec::Vec;

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn rd32(src: &[u8], pos: usize) -> u32 {
    debug_assert!(pos + 4 <= src.len());
    unsafe { (src.as_ptr().add(pos) as *const u32).read_unaligned() }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn rd32(src: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes(src[pos..pos + 4].try_into().unwrap())
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn rd64(src: &[u8], pos: usize) -> u64 {
    debug_assert!(pos + 8 <= src.len());
    unsafe { (src.as_ptr().add(pos) as *const u64).read_unaligned() }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn rd64(src: &[u8], pos: usize) -> u64 {
    u64::from_le_bytes(src[pos..pos + 8].try_into().unwrap())
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn src_byte(src: &[u8], pos: usize) -> u8 {
    debug_assert!(pos < src.len());
    unsafe { *src.get_unchecked(pos) }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn src_byte(src: &[u8], pos: usize) -> u8 {
    src[pos]
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn hash_load(table: &[u32], idx: usize) -> u32 {
    debug_assert!(idx < table.len());
    unsafe { *table.get_unchecked(idx) }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn hash_load(table: &[u32], idx: usize) -> u32 {
    table[idx]
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn hash_store(table: &mut [u32], idx: usize, val: u32) {
    debug_assert!(idx < table.len());
    unsafe { *table.get_unchecked_mut(idx) = val }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn hash_store(table: &mut [u32], idx: usize, val: u32) {
    table[idx] = val;
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

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn count_match(src: &[u8], p1: usize, p2: usize, limit: usize) -> usize {
    debug_assert!(p1 <= limit && limit <= src.len());
    debug_assert!(p2 < p1, "match position must be behind cursor");
    debug_assert!(p2 < src.len());
    let src_ptr = src.as_ptr();
    unsafe { count_match_raw(src_ptr.add(p1), src_ptr.add(p2), src_ptr.add(limit)) }
}

#[cfg(not(feature = "paranoid"))]
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

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn count_match(src: &[u8], p1: usize, p2: usize, limit: usize) -> usize {
    debug_assert!(p1 <= limit && limit <= src.len());
    debug_assert!(p2 < p1, "match position must be behind cursor");
    let max_len = limit - p1;
    let mut i = 0;
    while i + 8 <= max_len {
        let a = u64::from_le_bytes(src[p1 + i..p1 + i + 8].try_into().unwrap());
        let b = u64::from_le_bytes(src[p2 + i..p2 + i + 8].try_into().unwrap());
        let diff = a ^ b;
        if diff != 0 {
            return i + (diff.trailing_zeros() >> 3) as usize;
        }
        i += 8;
    }
    while i < max_len {
        if src[p1 + i] != src[p2 + i] {
            break;
        }
        i += 1;
    }
    i
}

#[inline(always)]
pub(crate) fn assert_rep_valid(r0: u32, r1: u32) {
    if r0 == 0 || r1 == 0 {
        cold_rep_panic(r0, r1);
    }
}

#[cold]
#[inline(never)]
fn cold_rep_panic(r0: u32, r1: u32) -> ! {
    panic!("rep offsets must be non-zero: r0={r0}, r1={r1}");
}

#[cfg(all(target_arch = "x86_64", not(miri), not(feature = "paranoid")))]
#[inline(always)]
pub(crate) fn prefetch_ht(table: &[u32], idx: usize) {
    unsafe {
        core::arch::x86_64::_mm_prefetch(
            table.as_ptr().add(idx) as *const i8,
            core::arch::x86_64::_MM_HINT_T0,
        );
    }
}

#[cfg(all(target_arch = "aarch64", not(miri), not(feature = "paranoid")))]
#[inline(always)]
pub(crate) fn prefetch_ht(table: &[u32], idx: usize) {
    debug_assert!(idx < table.len());
    unsafe {
        let ptr = table.as_ptr().add(idx) as *const u8;
        core::arch::asm!("prfm pldl1keep, [{x}]", x = in(reg) ptr, options(nostack, preserves_flags));
    }
}

#[cfg(any(miri, feature = "paranoid"))]
#[inline(always)]
pub(crate) fn prefetch_ht(_table: &[u32], _idx: usize) {}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn slice_get<T: Copy>(slice: &[T], idx: usize) -> T {
    debug_assert!(idx < slice.len());
    unsafe { *slice.get_unchecked(idx) }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn slice_get<T: Copy>(slice: &[T], idx: usize) -> T {
    slice[idx]
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn slice_get_ref<T>(slice: &[T], idx: usize) -> &T {
    debug_assert!(idx < slice.len());
    unsafe { slice.get_unchecked(idx) }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn slice_get_ref<T>(slice: &[T], idx: usize) -> &T {
    &slice[idx]
}

#[cfg(not(feature = "paranoid"))]
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
    debug_assert!(
        len > 16 || dst_off + 16 <= dst.capacity(),
        "copy_literals_fast requires 16 bytes of dst headroom for short copies"
    );
    unsafe {
        let s = src.as_ptr().add(src_off);
        let d = dst.as_mut_ptr().add(dst_off);
        if len <= 16 && src_off + 16 <= src.len() {
            (d as *mut u64).write_unaligned((s as *const u64).read_unaligned());
            (d.add(8) as *mut u64).write_unaligned((s.add(8) as *const u64).read_unaligned());
        } else {
            core::ptr::copy_nonoverlapping(s, d, len);
        }
    }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn copy_literals_fast(
    src: &[u8],
    src_off: usize,
    dst: &mut Vec<u8>,
    dst_off: usize,
    len: usize,
) {
    let needed = dst_off + len;
    if dst.len() < needed {
        dst.resize(needed, 0);
    }
    dst[dst_off..dst_off + len].copy_from_slice(&src[src_off..src_off + len]);
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn bitstream_flush(buf: &mut Vec<u8>, pos: usize, bits: u64) {
    debug_assert!(pos + 8 <= buf.capacity());
    unsafe {
        (buf.as_mut_ptr().add(pos) as *mut u64).write_unaligned(bits.to_le());
    }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn bitstream_flush(buf: &mut Vec<u8>, pos: usize, bits: u64) {
    let needed = pos + 8;
    if buf.len() < needed {
        buf.resize(needed, 0);
    }
    buf[pos..pos + 8].copy_from_slice(&bits.to_le_bytes());
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn bitstream_write_byte(buf: &mut Vec<u8>, pos: usize, val: u8) {
    debug_assert!(pos < buf.capacity());
    unsafe { *buf.as_mut_ptr().add(pos) = val }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn bitstream_write_byte(buf: &mut Vec<u8>, pos: usize, val: u8) {
    if buf.len() <= pos {
        buf.resize(pos + 1, 0);
    }
    buf[pos] = val;
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn vec_write_at<T>(vec: &mut Vec<T>, idx: usize, val: T) {
    debug_assert!(idx < vec.capacity());
    unsafe { vec.as_mut_ptr().add(idx).write(val) }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn vec_write_at<T: Default + Clone>(vec: &mut Vec<T>, idx: usize, val: T) {
    if vec.len() <= idx {
        vec.resize(idx + 1, T::default());
    }
    vec[idx] = val;
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn set_vec_len<T>(vec: &mut Vec<T>, len: usize) {
    debug_assert!(len <= vec.capacity());
    unsafe { vec.set_len(len) }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn set_vec_len<T: Default + Clone>(vec: &mut Vec<T>, len: usize) {
    vec.resize(len, T::default());
}
