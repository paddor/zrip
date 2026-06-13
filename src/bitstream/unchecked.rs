#[inline(always)]
pub unsafe fn read_u64_le_unchecked(src: &[u8], pos: usize) -> u64 {
    debug_assert!(pos + 8 <= src.len());
    // SAFETY: caller ensures pos + 8 <= src.len().
    unsafe { u64::from_le((src.as_ptr().add(pos) as *const u64).read_unaligned()) }
}

#[inline(always)]
pub unsafe fn read_byte_unchecked(src: &[u8], pos: usize) -> u8 {
    debug_assert!(pos < src.len());
    // SAFETY: caller ensures pos < src.len().
    unsafe { *src.get_unchecked(pos) }
}
