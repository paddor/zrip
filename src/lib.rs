//! Fast, pure-Rust zstd compression.
//!
//! zrip implements zstd compression levels -7 through 4 (Fast and DFast strategies),
//! targeting high-speed compression for data transfers. It produces standard zstd
//! frames decompressible by any compliant decoder.
//!
//! # Quick start
//!
//! ```
//! let data = b"hello world, hello zstd compression!";
//!
//! // Compress at level 1 (fast)
//! let compressed = zrip::compress(data, 1).unwrap();
//!
//! // Decompress
//! let decompressed = zrip::decompress(&compressed).unwrap();
//! assert_eq!(&decompressed, data);
//! ```
//!
//! # Compression levels
//!
//! | Level | Strategy | Notes |
//! |-------|----------|-------|
//! | -7..=-1 | Fast | Fastest encode, lowest ratio |
//! | 1..=2 | Fast | Good balance for network transfers |
//! | 3..=4 | DFast | Better ratio, still fast |
//!
//! # Streaming
//!
//! [`FrameEncoder`] and [`FrameDecoder`] implement [`std::io::Write`] and
//! [`std::io::Read`] for streaming compression and decompression.
//!
//! ```
//! use std::io::{Write, Read};
//!
//! let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
//! encoder.write_all(b"streaming data").unwrap();
//! let compressed = encoder.finish().unwrap();
//!
//! let mut decoder = zrip::FrameDecoder::new(&compressed[..]);
//! let mut output = Vec::new();
//! decoder.read_to_end(&mut output).unwrap();
//! assert_eq!(&output, b"streaming data");
//! ```
//!
//! # Buffer reuse
//!
//! For repeated compression/decompression, [`CompressContext`] and
//! [`DecompressContext`] reuse internal buffers across calls, reducing allocation
//! overhead in hot loops.
//!
//! # Feature flags
//!
//! - **`std`** (default): enables `alloc` and standard library support.
//! - **`alloc`**: `no_std` with heap allocation (`Vec`, etc.).
//! - **`frame`** (default): frame header parsing/writing; implies `std`.
//! - **`dict_builder`**: COVER/FastCOVER dictionary training.
//! - **`nightly`**: `#[optimize]` attributes for hot paths.

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(optimize_attribute))]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod error;

#[allow(dead_code)]
pub(crate) mod bitstream;
#[allow(dead_code)]
pub(crate) mod fse;
#[allow(dead_code)]
pub(crate) mod hash;
#[allow(dead_code)]
pub(crate) mod huffman;
#[allow(dead_code)]
pub(crate) mod xxhash;

#[allow(dead_code)]
pub(crate) mod block;
pub mod frame;

#[allow(dead_code)]
pub mod decode;
#[allow(dead_code)]
pub mod encode;

#[cfg(feature = "alloc")]
pub mod dict;

#[allow(dead_code)]
pub(crate) mod simd;

pub use error::{CompressError, DecompressError, ZstdError};

#[cfg(feature = "alloc")]
pub use dict::Dictionary;
#[cfg(feature = "alloc")]
pub use encode::strategy::LevelParams;

/// Default safety limit for decompressed output.
///
/// Applied by [`decompress`] and [`FrameDecoder::new`]. Use
/// [`DecompressContext::decompress_with_limit`] or [`FrameDecoder::with_limit`]
/// for explicit control.
pub const DEFAULT_DECOMPRESS_LIMIT: usize = usize::MAX;

pub(crate) const LARGE_OUTPUT_THRESHOLD: usize = 512 * 1024;

/// Decompresses a zstd-compressed frame (or concatenated frames).
///
/// For explicit control over the output size limit, use
/// [`DecompressContext::decompress_with_limit`].
#[cfg(feature = "alloc")]
pub fn decompress(input: &[u8]) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    decode::decompress(input)
}

/// Decompresses a zstd frame compressed with a dictionary.
#[cfg(feature = "alloc")]
pub fn decompress_with_dict(
    input: &[u8],
    dict: &dict::Dictionary,
) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    decode::decompress_with_dict(input, Some(dict))
}

/// Decompresses into an existing `Vec`, appending the output. Returns bytes written.
///
/// Useful for reusing a pre-allocated buffer across multiple decompressions.
#[cfg(feature = "alloc")]
pub fn decompress_into(
    input: &[u8],
    output: &mut alloc::vec::Vec<u8>,
) -> Result<usize, DecompressError> {
    decode::decompress_into(input, output)
}

/// Compresses `input` into a zstd frame at the given level (-7..=4).
#[cfg(feature = "alloc")]
pub fn compress(input: &[u8], level: i32) -> Result<alloc::vec::Vec<u8>, CompressError> {
    encode::compress(input, level)
}

/// Compresses `input` using explicit [`LevelParams`].
#[cfg(feature = "alloc")]
pub fn compress_with_params(
    input: &[u8],
    params: &encode::strategy::LevelParams,
) -> Result<alloc::vec::Vec<u8>, CompressError> {
    encode::compress_with_params(input, params)
}

/// Compresses `input` at the given level using a pre-trained dictionary.
#[cfg(feature = "alloc")]
pub fn compress_with_dict(
    input: &[u8],
    level: i32,
    dict: &dict::Dictionary,
) -> Result<alloc::vec::Vec<u8>, CompressError> {
    encode::compress_with_dict(input, level, dict)
}

/// Compresses `input` into a caller-provided buffer, returning bytes written.
///
/// Use [`compress_bound`] to determine the required buffer size.
#[cfg(feature = "alloc")]
pub fn compress_into(input: &[u8], output: &mut [u8], level: i32) -> Result<usize, CompressError> {
    encode::compress_into(input, output, level)
}

/// Returns the maximum compressed size for a given input length.
///
/// The actual compressed output is almost always smaller. Use this to
/// pre-allocate the output buffer for [`compress_into`].
pub fn compress_bound(input_len: usize) -> usize {
    // Frame header: magic(4) + descriptor(1) + fcs(8 max) + window(1) = 14
    // Block headers: 3 bytes each
    // Content checksum: 4 bytes
    let num_blocks = input_len / frame::MAX_BLOCK_SIZE + 1;
    input_len + num_blocks * 3 + 18
}

#[cfg(feature = "std")]
pub use decode::context::DecompressContext;
#[cfg(feature = "std")]
pub use decode::streaming::FrameDecoder;
#[cfg(feature = "std")]
pub use encode::context::CompressContext;
#[cfg(feature = "std")]
pub use encode::streaming::FrameEncoder;
