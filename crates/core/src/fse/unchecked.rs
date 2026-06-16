use crate::fse::encode::SymbolTransform;

#[inline(always)]
pub unsafe fn symbol_tt_get_unchecked(table: &[SymbolTransform], idx: usize) -> &SymbolTransform {
    debug_assert!(idx < table.len());
    // SAFETY: caller ensures idx < table.len(). Symbol values bounded by distribution.len().
    unsafe { table.get_unchecked(idx) }
}

#[inline(always)]
pub unsafe fn state_table_get_unchecked(table: &[u16], idx: usize) -> u16 {
    debug_assert!(idx < table.len());
    // SAFETY: caller ensures idx < table.len(). State bounded by FSE table construction.
    unsafe { *table.get_unchecked(idx) }
}
