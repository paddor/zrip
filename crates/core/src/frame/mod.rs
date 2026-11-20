//! Zstd frame format: constants, header parsing, and frame size scanning.

#![forbid(unsafe_code)]

pub mod header;

use crate::error::DecompressError;

/// Zstd frame magic number (`0xFD2F_B528`).
pub const ZSTD_MAGIC: u32 = 0xFD2F_B528;

/// Minimum frame header size in bytes.
pub const MIN_FRAME_HEADER_SIZE: usize = 2;

/// Maximum uncompressed block size (128 KiB).
pub const MAX_BLOCK_SIZE: usize = 128 * 1024;

/// Maximum window size (128 MiB).
pub const MAX_WINDOW_SIZE: u64 = 1 << 27;

/// Returns the size of the first frame in `input`, from magic number to
/// checksum, without decoding it.
///
/// Walks the frame header and block headers, like C zstd's
/// `ZSTD_findFrameCompressedSize`. Skippable frames count as frames. Use it
/// to split concatenated frames or to check that a buffer holds exactly one
/// frame.
///
/// Returns [`DecompressError::InputExhausted`] if `input` ends inside the
/// frame, and the usual header errors for a malformed one. Block contents
/// are not checked.
///
/// ```
/// let a = zrip::compress(b"first", 1).unwrap();
/// let b = zrip::compress(b"second", 1).unwrap();
/// let both = [&a[..], &b[..]].concat();
/// assert_eq!(zrip::frame::find_frame_compressed_size(&both), Ok(a.len()));
/// ```
pub fn find_frame_compressed_size(input: &[u8]) -> Result<usize, DecompressError> {
    if input.len() < 4 {
        return Err(DecompressError::InputExhausted);
    }
    let magic = u32::from_le_bytes([input[0], input[1], input[2], input[3]]);
    // Skippable frame: magic `0x184D2A50` to `0x184D2A5F`, a 4-byte size,
    // then that many bytes.
    if (magic & 0xFFFF_FFF0) == 0x184D_2A50 {
        if input.len() < 8 {
            return Err(DecompressError::InputExhausted);
        }
        let size = u32::from_le_bytes([input[4], input[5], input[6], input[7]]) as usize;
        let total = size.checked_add(8).ok_or(DecompressError::InputExhausted)?;
        return if total <= input.len() {
            Ok(total)
        } else {
            Err(DecompressError::InputExhausted)
        };
    }
    if magic != ZSTD_MAGIC {
        return Err(DecompressError::BadMagic);
    }

    // Size the header from its descriptor first, so a truncated header
    // reads as truncated input rather than a bad header.
    let descriptor = *input.get(4).ok_or(DecompressError::InputExhausted)?;
    let single_segment = descriptor & 0x20 != 0;
    // Magic and descriptor, then the window byte, dict ID, and content size
    // fields the descriptor selects.
    let header_size = 5
        + usize::from(!single_segment)
        + [0, 1, 2, 4][usize::from(descriptor & 0x03)]
        + match descriptor >> 6 {
            0 => usize::from(single_segment),
            1 => 2,
            2 => 4,
            _ => 8,
        };
    if input.len() < header_size {
        return Err(DecompressError::InputExhausted);
    }
    let header = header::parse_frame_header(&input[..header_size])?;
    let mut pos = header.header_size;
    loop {
        let block = crate::block::parse_block_header(input.get(pos..).unwrap_or(&[]))?;
        let size = block.block_size as usize;
        if size > MAX_BLOCK_SIZE {
            return Err(DecompressError::BlockTooLarge);
        }
        // 3-byte block header, then the body: one byte for RLE.
        let body = match block.block_type {
            crate::block::BlockType::Rle => 1,
            _ => size,
        };
        pos += 3 + body;
        if pos > input.len() {
            return Err(DecompressError::InputExhausted);
        }
        if block.last_block {
            break;
        }
    }
    if header.content_checksum {
        pos += 4;
        if pos > input.len() {
            return Err(DecompressError::InputExhausted);
        }
    }
    Ok(pos)
}
