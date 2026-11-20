//! Reverse bit reader for FSE and Huffman streams, which are read from
//! the last byte backward.

#![forbid(unsafe_code)]

use crate::bitstream::primitives;
use crate::error::DecompressError;
use crate::hint::{likely, unlikely};

/// Reverse bitstream reader using C zstd's bitsConsumed model.
///
/// Instead of tracking `bits_available` (decrement on consume, increment on refill),
/// this tracks `bits_consumed` (increment on consume, reset on reload). Peek uses
/// a double-shift: `(container << consumed) >> (64 - n)` = 2 ops vs the old model's
/// 3 ops (shift + mask + subtract).
pub struct ReverseBitReader<'a> {
    pub(crate) data: &'a [u8],
    /// The eight bytes at `ptr`, little-endian. Bits are read from the top.
    pub(crate) container: u64,
    /// Bits taken from the top of `container`. Above 64 means a read went
    /// past the start of the stream.
    pub(crate) bits_consumed: u32,
    /// Byte offset of `container` in `data`. Moves toward 0.
    pub(crate) ptr: usize,
    /// Lowest `ptr` at which the fast decode loops refill, as C zstd's
    /// `limitPtr`: 8, or 0 for streams under 8 bytes.
    pub(crate) limit_ptr: usize,
}

impl<'a> ReverseBitReader<'a> {
    /// Positions the reader below the end mark, the highest set bit of the
    /// last byte. Fails on empty data or a zero last byte.
    pub fn new(data: &'a [u8]) -> Result<Self, DecompressError> {
        if data.is_empty() {
            return Err(DecompressError::InputExhausted);
        }

        let last_byte = *data.last().unwrap();
        if last_byte == 0 {
            return Err(DecompressError::CorruptSequences);
        }

        // The zeros above the end mark, and the mark itself.
        let initial_consumed = last_byte.leading_zeros() + 1;

        let ptr = if data.len() >= 8 { data.len() - 8 } else { 0 };

        let container = if data.len() >= 8 {
            primitives::read_u64_le_unaligned(data, ptr)
        } else {
            let mut val = 0u64;
            for (i, &b) in data.iter().enumerate() {
                val |= (b as u64) << (i * 8);
            }
            val
        };

        // A stream under 8 bytes sits in the low bytes of the container; the
        // empty top bytes count as consumed.
        let bits_consumed = if data.len() >= 8 {
            initial_consumed
        } else {
            64 - (data.len() as u32) * 8 + initial_consumed
        };

        let limit_ptr = if data.len() >= 8 { 8 } else { 0 };

        Ok(Self {
            data,
            container,
            bits_consumed,
            ptr,
            limit_ptr,
        })
    }

    /// Steps back over the fully read bytes, at most to the stream start, and
    /// reloads the container. Afterwards at most 7 bits are consumed, unless
    /// the start was reached.
    #[inline(always)]
    pub fn refill(&mut self) {
        if self.bits_consumed <= 7 || self.ptr == 0 {
            return;
        }
        let byte_shift = (self.bits_consumed >> 3) as usize;
        let actual_shift = byte_shift.min(self.ptr);
        self.ptr -= actual_shift;
        self.bits_consumed -= (actual_shift as u32) * 8;
        if self.ptr + 8 <= self.data.len() {
            self.container = primitives::read_u64_le_unaligned(self.data, self.ptr);
        } else {
            let mut val = 0u64;
            let avail = self.data.len() - self.ptr;
            for i in 0..avail {
                val |= (self.data[self.ptr + i] as u64) << (i * 8);
            }
            self.container = val;
        }
    }

    /// Refills, then reads `n` bits, at most 32. Fails with
    /// [`DecompressError::InputExhausted`] past the start of the stream.
    #[inline]
    pub fn read_bits(&mut self, n: u8) -> Result<u32, DecompressError> {
        debug_assert!(n <= 32);
        if n == 0 {
            return Ok(0);
        }
        self.refill();
        let avail = 64u32.saturating_sub(self.bits_consumed);
        if (n as u32) > avail {
            return Err(DecompressError::InputExhausted);
        }
        let result = ((self.container << self.bits_consumed) >> (64 - n as u32)) as u32;
        self.bits_consumed += n as u32;
        Ok(result)
    }

    /// Skips `n` bits, which the container must hold, then refills.
    #[inline(always)]
    pub fn consume_bits(&mut self, n: u8) {
        debug_assert!((n as u32) <= 64u32.saturating_sub(self.bits_consumed));
        self.bits_consumed += n as u32;
        self.refill();
    }

    /// Reads `n` bits (0 to 32) with no refill and no check. The caller makes
    /// sure the container holds them.
    #[inline(always)]
    pub fn read_bits_branchless(&mut self, n: u8) -> u32 {
        debug_assert!(n <= 32);
        // `>> 1 >> (63 - n)` is `>> (64 - n)` without a shift by 64 at
        // `n == 0`. `& 63` keeps the first shift in range.
        let result = ((self.container << (self.bits_consumed & 63)) >> 1 >> (63 - n as u32)) as u32;
        self.bits_consumed += n as u32;
        result
    }

    /// Refills without the stream-start handling of [`refill`](Self::refill),
    /// for streams of 8 bytes or more while the step stays inside the
    /// stream. Otherwise returns false and changes nothing.
    #[inline(always)]
    pub fn try_refill_fast(&mut self) -> bool {
        let byte_shift = (self.bits_consumed >> 3) as usize;
        if likely(byte_shift <= self.ptr && self.data.len() >= 8) {
            self.ptr -= byte_shift;
            self.bits_consumed -= (byte_shift as u32) * 8;
            // `ptr` starts at `len - 8` for long streams and only moves toward
            // zero, so any successful fast refill can still load eight bytes.
            debug_assert!(self.ptr + 8 <= self.data.len());
            self.container = primitives::read_u64_le_unaligned(self.data, self.ptr);
            return true;
        }
        false
    }

    /// [`try_refill_fast`](Self::try_refill_fast), falling back to
    /// [`refill`](Self::refill) near the stream start.
    #[inline(always)]
    pub fn refill_fast_or_regular(&mut self) {
        if unlikely(!self.try_refill_fast()) {
            self.refill();
        }
    }

    /// Byte offset of the current container load.
    #[inline]
    pub fn ptr(&self) -> usize {
        self.ptr
    }

    /// Lowest `ptr` at which the fast decode loops refill: 8, or 0 for
    /// streams under 8 bytes.
    #[inline]
    pub fn limit_ptr(&self) -> usize {
        self.limit_ptr
    }

    /// The next `n` bits (at most 32), without consuming them or refilling.
    /// The container must hold them.
    #[inline]
    pub fn peek_bits(&self, n: u8) -> u32 {
        debug_assert!(n <= 32);
        debug_assert!((n as u32) <= 64u32.saturating_sub(self.bits_consumed));
        if n == 0 {
            return 0;
        }
        ((self.container << self.bits_consumed) >> (64 - n as u32)) as u32
    }

    /// Bits left to read: the rest of the container and the bytes below
    /// `ptr`.
    #[inline]
    pub fn bits_remaining(&self) -> usize {
        64usize.saturating_sub(self.bits_consumed as usize) + self.ptr * 8
    }

    /// Whether every bit was read, and no more. Reads past the stream's
    /// start push the consumed count above 64, which this rejects, as C
    /// zstd's `BIT_endOfDStream` does.
    #[inline]
    pub fn is_exhausted(&self) -> bool {
        self.ptr == 0 && self.bits_consumed == 64
    }

    /// Whether nothing is left to read, including after reads past the start
    /// that [`is_exhausted`](Self::is_exhausted) rejects.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.bits_consumed >= 64 && self.ptr == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        assert!(ReverseBitReader::new(&[]).is_err());
    }

    #[test]
    fn zero_last_byte() {
        assert!(ReverseBitReader::new(&[0x00]).is_err());
    }

    #[test]
    fn sentinel_only_no_data() {
        let data = [0b0000_0001];
        let r = ReverseBitReader::new(&data).unwrap();
        assert_eq!(r.bits_remaining(), 0);
    }

    #[test]
    fn roundtrip_with_forward_writer() {
        use crate::bitstream::writer::BitWriter;

        let mut w = BitWriter::new();
        w.write_bits(0b101, 3);
        w.write_bits(0b1100_1010, 8);
        w.write_bits(0b1, 1);
        w.close_reverse_stream();
        let bytes = w.into_bytes();

        let mut r = ReverseBitReader::new(&bytes).unwrap();
        assert_eq!(r.read_bits(1).unwrap(), 0b1);
        assert_eq!(r.read_bits(8).unwrap(), 0b1100_1010);
        assert_eq!(r.read_bits(3).unwrap(), 0b101);
        assert_eq!(r.bits_remaining(), 0);
    }

    #[test]
    fn single_byte_with_data() {
        let data = [0b0000_1101];
        let mut r = ReverseBitReader::new(&data).unwrap();
        assert_eq!(r.read_bits(3).unwrap(), 0b101);
        assert_eq!(r.bits_remaining(), 0);
    }

    #[test]
    fn refill_fast_or_regular_falls_back_when_overconsumed() {
        let data = [0b0000_0001];
        let mut r = ReverseBitReader::new(&data).unwrap();
        r.refill_fast_or_regular();
        assert_eq!(r.ptr, 0);
        assert_eq!(r.bits_remaining(), 0);
    }

    #[test]
    fn multi_byte_stream() {
        use crate::bitstream::writer::BitWriter;

        let mut w = BitWriter::new();
        w.write_bits(0xFF, 8);
        w.write_bits(0x3, 2);
        w.close_reverse_stream();
        let bytes = w.into_bytes();

        let mut r = ReverseBitReader::new(&bytes).unwrap();
        assert_eq!(r.read_bits(2).unwrap(), 0x3);
        assert_eq!(r.read_bits(8).unwrap(), 0xFF);
    }
}
