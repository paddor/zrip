#![forbid(unsafe_code)]

use crate::error::DecompressError;

pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit_pos: u8,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            bit_pos: 0,
        }
    }

    #[inline]
    pub fn bits_consumed(&self) -> usize {
        self.pos * 8 + self.bit_pos as usize
    }

    #[inline]
    pub fn bits_remaining(&self) -> usize {
        self.data.len() * 8 - self.bits_consumed()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.bits_remaining() == 0
    }

    #[inline]
    pub fn bytes_consumed(&self) -> usize {
        if self.bit_pos == 0 {
            self.pos
        } else {
            self.pos + 1
        }
    }

    pub fn read_bits(&mut self, n: u8) -> Result<u32, DecompressError> {
        debug_assert!(n <= 25);
        if n == 0 {
            return Ok(0);
        }
        if self.bits_remaining() < n as usize {
            return Err(DecompressError::InputExhausted);
        }

        let mut result = 0u32;
        let mut bits_left = n;
        let mut bit_offset = 0u8;

        while bits_left > 0 {
            let avail = 8 - self.bit_pos;
            let take = bits_left.min(avail);
            let byte = self.data[self.pos] as u32;
            let mask = (1u32 << take) - 1;
            let bits = (byte >> self.bit_pos) & mask;
            result |= bits << bit_offset;

            bit_offset += take;
            bits_left -= take;
            self.bit_pos += take;
            if self.bit_pos == 8 {
                self.bit_pos = 0;
                self.pos += 1;
            }
        }

        Ok(result)
    }

    pub fn read_bits_u16(&mut self, n: u8) -> Result<u16, DecompressError> {
        self.read_bits(n).map(|v| v as u16)
    }

    pub fn peek_bits(&self, n: u8) -> Result<u32, DecompressError> {
        let mut copy = Self {
            data: self.data,
            pos: self.pos,
            bit_pos: self.bit_pos,
        };
        copy.read_bits(n)
    }

    pub fn align_to_byte(&mut self) {
        if self.bit_pos != 0 {
            self.bit_pos = 0;
            self.pos += 1;
        }
    }

    pub fn remaining_bytes(&self) -> &'a [u8] {
        if self.bit_pos == 0 {
            &self.data[self.pos..]
        } else {
            &self.data[self.pos + 1..]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_single_bits() {
        let data = [0b10110100u8];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(1).unwrap(), 0);
        assert_eq!(r.read_bits(1).unwrap(), 0);
        assert_eq!(r.read_bits(1).unwrap(), 1);
        assert_eq!(r.read_bits(1).unwrap(), 0);
        assert_eq!(r.read_bits(1).unwrap(), 1);
        assert_eq!(r.read_bits(1).unwrap(), 1);
        assert_eq!(r.read_bits(1).unwrap(), 0);
        assert_eq!(r.read_bits(1).unwrap(), 1);
        assert!(r.is_empty());
    }

    #[test]
    fn read_multi_bit() {
        let data = [0xFF, 0x01];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(8).unwrap(), 0xFF);
        assert_eq!(r.read_bits(8).unwrap(), 0x01);
    }

    #[test]
    fn read_cross_byte() {
        let data = [0b11010110, 0b10110001];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(4).unwrap(), 0b0110);
        assert_eq!(r.read_bits(8).unwrap(), 0b00011101);
        assert_eq!(r.read_bits(4).unwrap(), 0b1011);
    }

    #[test]
    fn read_zero_bits() {
        let data = [0xFF];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(0).unwrap(), 0);
        assert_eq!(r.bits_consumed(), 0);
    }

    #[test]
    fn exhaustion() {
        let data = [0xFF];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(8).unwrap(), 0xFF);
        assert!(r.read_bits(1).is_err());
    }
}
