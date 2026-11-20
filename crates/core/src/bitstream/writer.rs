//! Forward bit writer, LSB first. Reverse streams are written with it too:
//! `close_reverse_stream` adds the end mark the reverse reader starts from.

#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use super::primitives;

/// LSB-first bit writer into a growable buffer. Pending bits are stored
/// eight bytes at a time once 32 have accumulated.
pub struct BitWriter {
    #[cfg(feature = "alloc")]
    buf: Vec<u8>,
    /// Pending bits, LSB first; `bits_used` of them are valid.
    bits: u64,
    bits_used: u8,
    /// Bytes of `buf` that hold finished output. The 8-byte stores leave
    /// `buf.len()` past it.
    pos: usize,
}

#[cfg(feature = "alloc")]
impl Default for BitWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "alloc")]
impl BitWriter {
    /// Empty writer with a small buffer.
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(64),
            bits: 0,
            bits_used: 0,
            pos: 0,
        }
    }

    /// Empty writer with room for `cap` bytes plus 8 bytes of store slack.
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap + 8),
            bits: 0,
            bits_used: 0,
            pos: 0,
        }
    }

    /// Empty writer that reuses the allocation of `buf`, dropping its
    /// contents.
    pub fn from_vec(mut buf: Vec<u8>) -> Self {
        buf.clear();
        buf.reserve(8);
        Self {
            buf,
            bits: 0,
            bits_used: 0,
            pos: 0,
        }
    }

    /// Flushes the pending bits, zero-padded to a byte, and returns the
    /// buffer.
    pub fn into_vec(mut self) -> Vec<u8> {
        self.flush_remaining();
        self.buf
    }

    /// Reserves room for the 8-byte store at `pos`. Truncates to `pos` first
    /// so the reallocation copies only finished bytes.
    #[inline(always)]
    fn ensure_capacity(&mut self) {
        if self.pos + 8 > self.buf.capacity() {
            primitives::set_vec_len(&mut self.buf, self.pos);
            self.buf.reserve(64);
        }
    }

    /// Appends the low `n` bits of `value`. Panics if `n > 25` or `value` has
    /// bits above `n`: at most 31 pending bits plus 25 fit in 64.
    #[inline(always)]
    pub fn write_bits(&mut self, value: u32, n: u8) {
        if n == 0 {
            return;
        }
        assert!(n <= 25, "bit count must be <= 25");
        assert!(value < (1u32 << n), "value does not fit in bit count");
        self.bits |= (value as u64) << self.bits_used;
        self.bits_used += n;
        // Store all eight bytes, but advance over the whole ones only. The
        // partial byte stays pending, and the next store overwrites the rest.
        if self.bits_used >= 32 {
            self.ensure_capacity();
            primitives::write_u64_le_unaligned(&mut self.buf, self.pos, self.bits);
            let nb = (self.bits_used >> 3) as usize;
            self.pos += nb;
            self.bits >>= nb << 3;
            self.bits_used &= 7;
        }
    }

    /// Writes the pending bits, the last byte zero-padded. Later writes start
    /// on a byte boundary.
    pub fn flush_remaining(&mut self) {
        primitives::set_vec_len(&mut self.buf, self.pos);
        while self.bits_used > 0 {
            self.buf.push(self.bits as u8);
            self.bits >>= 8;
            self.bits_used = self.bits_used.saturating_sub(8);
        }
        self.pos = self.buf.len();
    }

    /// Ends a reverse stream: writes the 1-bit end mark that
    /// `ReverseBitReader::new` looks for, then flushes.
    pub fn close_reverse_stream(&mut self) {
        self.write_bits(1, 1);
        self.flush_remaining();
    }

    /// Bits written so far, pending ones included.
    pub fn bits_written(&self) -> usize {
        self.pos * 8 + self.bits_used as usize
    }

    /// Same as [`into_vec`](Self::into_vec).
    pub fn into_bytes(mut self) -> Vec<u8> {
        self.flush_remaining();
        self.buf
    }

    /// The finished bytes, without pending bits. Call
    /// [`flush_remaining`](Self::flush_remaining) first to include them.
    pub fn as_bytes(&mut self) -> &[u8] {
        primitives::set_vec_len(&mut self.buf, self.pos);
        &self.buf
    }

    /// Appends `b` as 8 bits, at any bit position.
    pub fn write_byte(&mut self, b: u8) {
        self.write_bits(b as u32, 8);
    }

    /// Appends each byte of `bytes` as 8 bits.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_byte(b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "bit count must be <= 25")]
    fn write_bits_rejects_large_public_bit_count() {
        // Regression: `write_bits` is a safe public API, so violating its
        // maximum bit-count contract must stop before the flush path shifts by
        // an invalid amount.
        let mut writer = BitWriter::new();
        writer.write_bits(0, 255);
    }

    #[test]
    fn write_and_read_back() {
        let mut w = BitWriter::new();
        w.write_bits(0b101, 3);
        w.write_bits(0b11, 2);
        w.write_bits(0b000, 3);
        let bytes = w.into_bytes();
        assert_eq!(bytes, [0b0001_1101]);
    }

    #[test]
    fn write_cross_byte() {
        let mut w = BitWriter::new();
        w.write_bits(0xFF, 8);
        w.write_bits(0x01, 8);
        let bytes = w.into_bytes();
        assert_eq!(bytes, [0xFF, 0x01]);
    }

    #[test]
    fn write_partial() {
        let mut w = BitWriter::new();
        w.write_bits(0b1, 1);
        let bytes = w.into_bytes();
        assert_eq!(bytes, [0b0000_0001]);
    }

    #[test]
    fn reverse_stream_roundtrip() {
        use crate::bitstream::reader_reverse::ReverseBitReader;

        let mut w = BitWriter::new();
        w.write_bits(0b1010, 4);
        w.write_bits(0b0101, 4);
        w.close_reverse_stream();
        let bytes = w.into_bytes();

        let mut r = ReverseBitReader::new(&bytes).unwrap();
        assert_eq!(r.read_bits(4).unwrap(), 0b0101);
        assert_eq!(r.read_bits(4).unwrap(), 0b1010);
        assert_eq!(r.bits_remaining(), 0);
    }
}
