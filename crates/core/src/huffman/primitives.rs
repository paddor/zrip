#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use super::HuffmanDecodeEntry;

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn huf_table_lookup(table: &[HuffmanDecodeEntry], idx: usize) -> HuffmanDecodeEntry {
    table[idx]
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn huf_table_lookup(table: &[HuffmanDecodeEntry], idx: usize) -> HuffmanDecodeEntry {
    table[idx]
}

#[cfg(not(feature = "paranoid"))]
#[inline(always)]
pub(crate) fn huf_output_write(output: &mut [u8], pos: usize, val: u8) {
    output[pos] = val;
}

#[cfg(feature = "paranoid")]
#[inline(always)]
pub(crate) fn huf_output_write(output: &mut [u8], pos: usize, val: u8) {
    output[pos] = val;
}

/// Forward bit-output buffer for Huffman streams.
///
/// The vector is zero-filled to the reserved size up front, so every write is
/// a plain in-bounds slice store. `finish` truncates it to the stream length.
#[cfg(feature = "alloc")]
pub(crate) struct BitstreamScratch<'a> {
    buf: &'a mut Vec<u8>,
}

#[cfg(feature = "alloc")]
impl<'a> BitstreamScratch<'a> {
    #[inline(always)]
    pub(crate) fn new(buf: &'a mut Vec<u8>, reserve: usize) -> Self {
        buf.clear();
        buf.resize(reserve, 0);
        Self { buf }
    }

    #[inline(always)]
    pub(crate) fn flush(&mut self, pos: usize, bits: u64) {
        match self.buf.get_mut(pos..pos + 8) {
            Some(dst) => dst.copy_from_slice(&bits.to_le_bytes()),
            None => self.grow_and_flush(pos, bits),
        }
    }

    #[cold]
    #[inline(never)]
    fn grow_and_flush(&mut self, pos: usize, bits: u64) {
        let needed = (pos + 8).max(self.buf.len() * 2);
        self.buf.resize(needed, 0);
        self.buf[pos..pos + 8].copy_from_slice(&bits.to_le_bytes());
    }

    #[inline(always)]
    pub(crate) fn write_byte(&mut self, pos: usize, val: u8) {
        if pos >= self.buf.len() {
            self.buf.resize(pos + 1, 0);
        }
        self.buf[pos] = val;
    }

    #[inline(always)]
    pub(crate) fn finish(&mut self, len: usize) {
        assert!(len <= self.buf.len());
        self.buf.truncate(len);
    }
}
