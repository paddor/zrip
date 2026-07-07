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

#[cfg(feature = "alloc")]
pub(crate) struct BitstreamScratch<'a> {
    buf: &'a mut Vec<u8>,
    initialized: usize,
}

#[cfg(feature = "alloc")]
impl<'a> BitstreamScratch<'a> {
    #[inline(always)]
    pub(crate) fn new(buf: &'a mut Vec<u8>, reserve: usize) -> Self {
        buf.clear();
        buf.reserve(reserve);
        Self {
            buf,
            initialized: 0,
        }
    }

    #[inline(always)]
    pub(crate) fn flush(&mut self, pos: usize, bits: u64) {
        let needed = pos + 8;
        self.ensure_capacity(needed);

        #[cfg(not(feature = "paranoid"))]
        {
            // SAFETY: ensure_capacity proves the 8-byte write fits in the Vec
            // allocation. initialized tracks the largest written range before
            // finish exposes bytes through the Vec length.
            unsafe {
                (self.buf.as_mut_ptr().add(pos) as *mut u64).write_unaligned(bits.to_le());
            }
        }

        #[cfg(feature = "paranoid")]
        {
            if self.buf.len() < needed {
                self.buf.resize(needed, 0);
            }
            self.buf[pos..needed].copy_from_slice(&bits.to_le_bytes());
        }

        self.initialized = self.initialized.max(needed);
    }

    #[inline(always)]
    pub(crate) fn write_byte(&mut self, pos: usize, val: u8) {
        let needed = pos + 1;
        self.ensure_capacity(needed);

        #[cfg(not(feature = "paranoid"))]
        {
            // SAFETY: ensure_capacity proves the byte write fits in the Vec
            // allocation. initialized tracks the byte before finish exposes it.
            unsafe { *self.buf.as_mut_ptr().add(pos) = val }
        }

        #[cfg(feature = "paranoid")]
        {
            if self.buf.len() < needed {
                self.buf.resize(needed, 0);
            }
            self.buf[pos] = val;
        }

        self.initialized = self.initialized.max(needed);
    }

    #[inline(always)]
    pub(crate) fn finish(&mut self, len: usize) {
        assert!(len <= self.initialized);

        #[cfg(not(feature = "paranoid"))]
        {
            // SAFETY: flush and write_byte initialized every byte range that
            // callers expose. finish refuses to expose bytes beyond that range.
            unsafe { self.buf.set_len(len) }
        }

        #[cfg(feature = "paranoid")]
        {
            self.buf.truncate(len);
        }
    }

    #[inline(always)]
    fn ensure_capacity(&mut self, needed: usize) {
        if needed > self.buf.capacity() {
            self.buf.reserve(needed - self.buf.capacity());
        }
    }
}
