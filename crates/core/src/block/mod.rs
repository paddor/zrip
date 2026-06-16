#![forbid(unsafe_code)]

use crate::error::DecompressError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockType {
    Raw,
    Rle,
    Compressed,
}

#[derive(Debug, Clone, Copy)]
pub struct BlockHeader {
    pub last_block: bool,
    pub block_type: BlockType,
    pub block_size: u32,
}

pub fn parse_block_header(data: &[u8]) -> Result<BlockHeader, DecompressError> {
    if data.len() < 3 {
        return Err(DecompressError::InputExhausted);
    }

    let raw = u32::from_le_bytes([data[0], data[1], data[2], 0]);

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
        assert_eq!(hdr.block_size, 0x010000 >> 3);
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
