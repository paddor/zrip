#[inline(always)]
pub unsafe fn read_u32_unchecked(src: &[u8], pos: usize) -> u32 {
    debug_assert!(pos + 4 <= src.len());
    // SAFETY: caller ensures pos + 4 <= src.len().
    unsafe { (src.as_ptr().add(pos) as *const u32).read_unaligned() }
}

#[inline(always)]
pub unsafe fn read_u64_unchecked(src: &[u8], pos: usize) -> u64 {
    debug_assert!(pos + 8 <= src.len());
    // SAFETY: caller ensures pos + 8 <= src.len().
    unsafe { (src.as_ptr().add(pos) as *const u64).read_unaligned() }
}

#[inline(always)]
pub unsafe fn read_byte_unchecked(src: &[u8], pos: usize) -> u8 {
    debug_assert!(pos < src.len());
    // SAFETY: caller ensures pos < src.len().
    unsafe { *src.get_unchecked(pos) }
}

#[inline(always)]
pub unsafe fn hash_get_unchecked(table: &[u32], idx: usize) -> u32 {
    debug_assert!(idx < table.len());
    // SAFETY: caller ensures idx < table.len(). Typically idx = hash & (table.len() - 1).
    unsafe { *table.get_unchecked(idx) }
}

#[inline(always)]
pub unsafe fn hash_put_unchecked(table: &mut [u32], idx: usize, val: u32) {
    debug_assert!(idx < table.len());
    // SAFETY: caller ensures idx < table.len(). Typically idx = hash & (table.len() - 1).
    unsafe {
        *table.get_unchecked_mut(idx) = val;
    }
}
