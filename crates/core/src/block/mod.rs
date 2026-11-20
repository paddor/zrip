//! Block header parsing.

#![forbid(unsafe_code)]

use crate::error::DecompressError;

/// Block type, from bits 1-2 of the block header. Value 3 is reserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockType {
    /// Stored as is: `block_size` bytes.
    Raw,
    /// One byte repeated `block_size` times.
    Rle,
    /// Literals and sequences sections, `block_size` bytes.
    Compressed,
}

/// A parsed 3-byte block header.
#[derive(Debug, Clone, Copy)]
pub struct BlockHeader {
    /// Whether this is the last block of the frame.
    pub last_block: bool,
    /// How the body is coded.
    pub block_type: BlockType,
    /// Content size for raw and RLE blocks (the RLE body is one byte), body
    /// size for compressed ones. Not checked against a limit here.
    pub block_size: u32,
}

/// Parses the 3-byte block header at the start of `data`.
pub fn parse_block_header(data: &[u8]) -> Result<BlockHeader, DecompressError> {
    if data.len() < 3 {
        return Err(DecompressError::InputExhausted);
    }

    let raw = u32::from_le_bytes([data[0], data[1], data[2], 0]);

    // Little-endian: bit 0 last block, bits 1-2 type, bits 3-23 size.
    let last_block = (raw & 1) != 0;
    let block_type_bits = (raw >> 1) & 0x03;
    let block_size = raw >> 3;

    let block_type = match block_type_bits {
        0 => BlockType::Raw,
        1 => BlockType::Rle,
        2 => BlockType::Compressed,
        _ => return Err(DecompressError::BadBlockType),
    };

    Ok(BlockHeader {
        last_block,
        block_type,
        block_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_raw_block() {
        let data = [0x00, 0x00, 0x01];
        let hdr = parse_block_header(&data).unwrap();
        assert!(!hdr.last_block);
        assert_eq!(hdr.block_type, BlockType::Raw);
        assert_eq!(hdr.block_size, 0x0001_0000 >> 3);
    }

    #[test]
    fn parse_last_compressed() {
        let data = [0x05, 0x00, 0x00];
        let hdr = parse_block_header(&data).unwrap();
        assert!(hdr.last_block);
        assert_eq!(hdr.block_type, BlockType::Compressed);
        assert_eq!(hdr.block_size, 0);
    }
}
