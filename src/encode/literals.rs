#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

pub fn encode_raw_literals(literals: &[u8], output: &mut Vec<u8>) {
    let size = literals.len();

    if size <= 31 {
        output.push((size as u8) << 3);
    } else if size <= 4095 {
        let b0 = 0x04 | ((size as u8 & 0x0F) << 4);
        let b1 = (size >> 4) as u8;
        output.push(b0);
        output.push(b1);
    } else {
        let b0 = 0x0C | ((size as u8 & 0x0F) << 4);
        let b1 = (size >> 4) as u8;
        let b2 = (size >> 12) as u8;
        output.push(b0);
        output.push(b1);
        output.push(b2);
    }

    output.extend_from_slice(literals);
}

pub fn encode_rle_literals(byte: u8, count: usize, output: &mut Vec<u8>) {
    let size = count;

    if size <= 31 {
        output.push(0x01 | (size as u8) << 3);
    } else if size <= 4095 {
        let b0 = 0x05 | ((size as u8 & 0x0F) << 4);
        let b1 = (size >> 4) as u8;
        output.push(b0);
        output.push(b1);
    } else {
        let b0 = 0x0D | ((size as u8 & 0x0F) << 4);
        let b1 = (size >> 4) as u8;
        let b2 = (size >> 12) as u8;
        output.push(b0);
        output.push(b1);
        output.push(b2);
    }

    output.push(byte);
}
