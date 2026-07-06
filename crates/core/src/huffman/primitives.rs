#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use super::HuffmanDecodeEntry;

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn huf_table_lookup(table: &[HuffmanDecodeEntry], idx: usize) -> HuffmanDecodeEntry {
    debug_assert!(idx < table.len());
    // SAFETY: Huffman decode state bounds idx to the decode table; debug builds
    // verify the invariant at this leaf.
    unsafe { *table.get_unchecked(idx) }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn huf_table_lookup(table: &[HuffmanDecodeEntry], idx: usize) -> HuffmanDecodeEntry {
    table[idx]
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn huf_output_write(output: &mut [u8], pos: usize, val: u8) {
    debug_assert!(pos < output.len());
    // SAFETY: The decode loop writes each symbol inside the output slice.
    unsafe { *output.get_unchecked_mut(pos) = val }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn huf_output_write(output: &mut [u8], pos: usize, val: u8) {
    output[pos] = val;
}

#[cfg(all(feature = "alloc", not(feature = "paranoid")))]
#[inline(always)]
pub(crate) fn set_vec_len(vec: &mut Vec<u8>, len: usize) {
    debug_assert!(len <= vec.capacity());
    // SAFETY: Huffman builders initialize all bytes before exposing them.
    unsafe { vec.set_len(len) }
}

#[cfg(all(feature = "alloc", feature = "paranoid"))]
#[inline(always)]
pub(crate) fn set_vec_len(vec: &mut Vec<u8>, len: usize) {
    vec.resize(len, 0);
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn get_unchecked_byte(data: &[u8], idx: usize) -> u8 {
    debug_assert!(idx < data.len());
    // SAFETY: The caller proves idx is in bounds; debug builds check it.
    unsafe { *data.get_unchecked(idx) }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn get_unchecked_byte(data: &[u8], idx: usize) -> u8 {
    data[idx]
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn get_unchecked_u16(data: &[u16], idx: usize) -> u16 {
    debug_assert!(idx < data.len());
    // SAFETY: The caller proves idx is in bounds; debug builds check it.
    unsafe { *data.get_unchecked(idx) }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn get_unchecked_u16(data: &[u16], idx: usize) -> u16 {
    data[idx]
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn get_unchecked_u8_arr(data: &[u8], idx: usize) -> u8 {
    debug_assert!(idx < data.len());
    // SAFETY: The caller proves idx is in bounds; debug builds check it.
    unsafe { *data.get_unchecked(idx) }
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn get_unchecked_u8_arr(data: &[u8], idx: usize) -> u8 {
    data[idx]
}

#[cfg(all(feature = "alloc", not(feature = "paranoid")))]
#[inline(always)]
pub(crate) fn bitstream_flush_vec(buf: &mut Vec<u8>, pos: usize, bits: u64) {
    debug_assert!(pos + 8 <= buf.capacity());
    // SAFETY: The bitstream writer reserves pos+8 capacity before flushing.
    // The write initializes bytes that will be exposed by set_len.
    unsafe {
        (buf.as_mut_ptr().add(pos) as *mut u64).write_unaligned(bits.to_le());
    }
}

#[cfg(all(feature = "alloc", feature = "paranoid"))]
#[inline(always)]
pub(crate) fn bitstream_flush_vec(buf: &mut Vec<u8>, pos: usize, bits: u64) {
    let needed = pos + 8;
    if buf.len() < needed {
        buf.resize(needed, 0);
    }
    buf[pos..pos + 8].copy_from_slice(&bits.to_le_bytes());
}
