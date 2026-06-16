use crate::huffman::HuffmanDecodeEntry;

#[inline(always)]
pub unsafe fn decode_table_get_unchecked(
    table: &[HuffmanDecodeEntry],
    idx: usize,
) -> &HuffmanDecodeEntry {
    debug_assert!(idx < table.len());
    // SAFETY: caller ensures idx < table.len(). idx = peek_bits(table_log) < 2^table_log = table.len().
    unsafe { table.get_unchecked(idx) }
}
