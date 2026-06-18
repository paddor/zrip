//! Zstd frame format constants.

#![forbid(unsafe_code)]

pub mod header;

/// Zstd frame magic number (`0xFD2F_B528`).
pub const ZSTD_MAGIC: u32 = 0xFD2F_B528;

/// Minimum frame header size in bytes.
pub const MIN_FRAME_HEADER_SIZE: usize = 2;

/// Maximum uncompressed block size (128 KiB).
pub const MAX_BLOCK_SIZE: usize = 128 * 1024;

/// Maximum window size (128 MiB).
pub const MAX_WINDOW_SIZE: u64 = 1 << 27;
