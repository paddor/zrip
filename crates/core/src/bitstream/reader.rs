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

        // One little-endian load covers `bit_pos + n <= 32` bits. Near the
        // end of the data, the missing bytes read as zero; the check above
        // guarantees the requested bits are all present.
        let word = match self.data.get(self.pos..self.pos + 8) {
            Some(bytes) => u64::from_le_bytes(bytes.try_into().unwrap()),
            None => {
                let mut buf = [0u8; 8];
                let tail = &self.data[self.pos..];
                buf[..tail.len()].copy_from_slice(tail);
                u64::from_le_bytes(buf)
            }
        };
        let result = ((word >> self.bit_pos) & ((1u64 << n) - 1)) as u32;
        let total = self.bit_pos as usize + n as usize;
        self.pos += total / 8;
        self.bit_pos = (total % 8) as u8;
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
        let data = [0b1011_0100_u8];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(1).unwrap(), 0);
        assert_eq!(r.read_bits(1).unwrap(), 0);
        assert_eq!(r.read_bits(1).unwrap(), 1);
        assert_eq!(r.read_bits(1).unwrap(), 0);
        assert_eq!(r.read_bits(1).unwrap(), 1);
        assert_eq!(r.read_bits(1).unwrap(), 1);
        assert_eq!(r.read_bits(1).unwrap(), 0);
        assert_eq!(r.read_bits(1).unwrap(), 1);
        assert_eq!(r.bits_remaining(), 0);
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
        let data = [0b1101_0110, 0b1011_0001];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(4).unwrap(), 0b0110);
        assert_eq!(r.read_bits(8).unwrap(), 0b0001_1101);
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
