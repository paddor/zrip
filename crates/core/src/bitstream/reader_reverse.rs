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
    pub data: &'a [u8],
    pub container: u64,
    pub bits_consumed: u32,
    pub ptr: usize,
    pub limit_ptr: usize,
}

impl<'a> ReverseBitReader<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, DecompressError> {
        if data.is_empty() {
            return Err(DecompressError::InputExhausted);
        }

        let last_byte = *data.last().unwrap();
        if last_byte == 0 {
            return Err(DecompressError::CorruptSequences);
        }

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
                val |= (primitives::get_byte_unchecked(self.data, self.ptr + i) as u64) << (i * 8);
            }
            self.container = val;
        }
    }

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

    #[inline]
    pub fn read_bits_unchecked(&mut self, n: u8) -> u32 {
        debug_assert!(n <= 32);
        if n == 0 {
            return 0;
        }
        self.refill();
        debug_assert!((n as u32) <= 64u32.saturating_sub(self.bits_consumed));
        let result = ((self.container << self.bits_consumed) >> (64 - n as u32)) as u32;
        self.bits_consumed += n as u32;
        result
    }

    #[inline(always)]
    pub fn consume_bits(&mut self, n: u8) {
        debug_assert!((n as u32) <= 64u32.saturating_sub(self.bits_consumed));
        self.bits_consumed += n as u32;
        self.refill();
    }

    #[inline(always)]
    pub fn read_bits_fast(&mut self, n: u8) -> u32 {
        debug_assert!((n as u32) <= 64u32.saturating_sub(self.bits_consumed));
        if n == 0 {
            return 0;
        }
        let result = ((self.container << self.bits_consumed) >> (64 - n as u32)) as u32;
        self.bits_consumed += n as u32;
        result
    }

    #[inline(always)]
    pub fn read_bits_branchless(&mut self, n: u8) -> u32 {
        debug_assert!(n <= 32);
        let result = ((self.container << (self.bits_consumed & 63)) >> 1 >> (63 - n as u32)) as u32;
        self.bits_consumed += n as u32;
        result
    }

    #[inline(always)]
    pub fn refill_fast(&mut self) {
        let byte_shift = (self.bits_consumed >> 3) as usize;
        if byte_shift > self.ptr || self.ptr - byte_shift + 8 > self.data.len() {
            return;
        }
        self.ptr -= byte_shift;
        self.bits_consumed -= (byte_shift as u32) * 8;
        self.container = primitives::read_u64_le_unaligned(self.data, self.ptr);
    }

    #[inline(always)]
    pub fn try_refill_fast(&mut self) -> bool {
        let byte_shift = (self.bits_consumed >> 3) as usize;
        if likely(byte_shift <= self.ptr && self.data.len() >= 8) {
            let new_ptr = self.ptr - byte_shift;
            if likely(new_ptr <= self.data.len() - 8) {
                self.ptr = new_ptr;
                self.bits_consumed -= (byte_shift as u32) * 8;
                self.container = primitives::read_u64_le_unaligned(self.data, self.ptr);
                return true;
            }
        }
        false
    }

    #[inline(always)]
    pub fn refill_fast_or_regular(&mut self) {
        if unlikely(!self.try_refill_fast()) {
            self.refill();
        }
    }

    #[inline]
    pub fn peek_bits(&self, n: u8) -> u32 {
        debug_assert!(n <= 32);
        debug_assert!((n as u32) <= 64u32.saturating_sub(self.bits_consumed));
        if n == 0 {
            return 0;
        }
        ((self.container << self.bits_consumed) >> (64 - n as u32)) as u32
    }

    #[inline]
    pub fn bits_remaining(&self) -> usize {
        64usize.saturating_sub(self.bits_consumed as usize) + self.ptr * 8
    }

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

#[cfg(all(test, miri, not(feature = "paranoid")))]
mod ub_tests {
    use super::*;

    #[test]
    fn public_refill_fast_underflows_on_short_stream() {
        // Issue: refill_fast is a safe public method, but its requirements
        // (enough consumed bits and at least eight readable bytes after the new
        // pointer) are enforced only with debug_asserts. On this one-byte stream,
        // byte_shift is 8 and ptr is 0, so release builds wrap the subtraction
        // and miri reports the resulting out-of-bounds read_u64_le_unaligned.
        let data = [0b0000_0001];
        let mut reader = ReverseBitReader::new(&data).unwrap();
        reader.refill_fast();
    }
}
