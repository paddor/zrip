#[inline(always)]
pub unsafe fn table_get_unchecked<T>(table: &[T], idx: usize) -> &T {
    debug_assert!(idx < table.len());
    // SAFETY: caller ensures idx < table.len(). FSE states are bounded by table construction.
    unsafe { table.get_unchecked(idx) }
}

#[inline(always)]
pub unsafe fn code_table_get_unchecked<T: Copy>(table: &[T], idx: usize) -> T {
    debug_assert!(idx < table.len());
    // SAFETY: caller ensures idx < table.len(). Code values bounded by LL_MAX_SYMBOL/ML_MAX_SYMBOL.
    unsafe { *table.get_unchecked(idx) }
}
