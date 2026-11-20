//! Table read and output write helpers for the Huffman decoders.

use super::HuffmanDecodeEntry;

#[inline(always)]
pub(crate) fn huf_table_lookup(table: &[HuffmanDecodeEntry], idx: usize) -> HuffmanDecodeEntry {
    table[idx]
}

#[inline(always)]
pub(crate) fn huf_output_write(output: &mut [u8], pos: usize, val: u8) {
    output[pos] = val;
}
