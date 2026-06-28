//! Fast, pure-Rust zstd compression.
//!
//! zrip implements zstd compression levels -8 through 4 (Fast and `DFast` strategies),
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
//! | -8 | Fast | Always raw literals, closest to LZ4 speed |
//! | -7..=-1 | Fast | Fastest encode, lowest ratio |
//! | 0 | | Library default (currently level 1) |
//! | 1..=2 | Fast | Good balance for network transfers |
//! | 3..=4 | `DFast` | Better ratio, still fast |
//!
//! # Streaming
//!
//! [`FrameEncoder`] and [`FrameDecoder`] implement [`std::io::Write`] and
//! [`std::io::Read`] for streaming compression and decompression.
//!
//! ```no_run
//! # #[cfg(feature = "std")]
//! # {
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
//! # }
//! ```
//!
//! # Long distance matching
//!
//! For data with long-range repetitions, enable LDM via [`Options`]:
//!
//! ```no_run
//! # #[cfg(feature = "ldm")]
//! # {
//! let opts = zrip::Options::default().window_log(24).ldm(true);
//! let compressed = zrip::compress_opts(b"data with long-range repeats", 1, &opts).unwrap();
//! # }
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
//! - **`ldm`** (default): long distance matching for large-window compression.
//! - **`dict_builder`**: COVER/FastCOVER dictionary training.
//! - **`nightly`**: `#[optimize]` attributes for hot paths.

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "paranoid", forbid(unsafe_code))]

#[cfg(feature = "alloc")]
extern crate alloc;

pub use zrip_core::error;
pub use zrip_core::frame;
pub use zrip_core::{CompressError, DecompressError, ZstdError};

#[cfg(feature = "alloc")]
pub use zrip_core::dict;
#[cfg(feature = "alloc")]
pub use zrip_core::dict::Dictionary;
#[cfg(feature = "alloc")]
pub use zrip_encode::strategy::{DEFAULT_LEVEL, LdmParams, LevelParams, Options};

pub const DEFAULT_DECOMPRESS_LIMIT: usize = usize::MAX;
pub use zrip_core::SAFE_DECOMPRESS_LIMIT;

#[doc(hidden)]
pub use zrip_decode as decode;
#[doc(hidden)]
pub use zrip_encode as encode;

#[cfg(feature = "alloc")]
pub fn decompress(input: &[u8]) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    zrip_decode::decompress(input)
}

#[cfg(feature = "alloc")]
pub fn decompress_with_dict(
    input: &[u8],
    dict: &zrip_core::dict::Dictionary,
) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    zrip_decode::decompress_with_dict(input, Some(dict))
}

#[cfg(feature = "alloc")]
pub fn decompress_with_limit(
    input: &[u8],
    max_output_size: usize,
) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    zrip_decode::decompress_with_limit(input, max_output_size)
}

#[cfg(feature = "alloc")]
pub fn decompress_into(
    input: &[u8],
    output: &mut alloc::vec::Vec<u8>,
) -> Result<usize, DecompressError> {
    zrip_decode::decompress_into(input, output)
}

#[cfg(feature = "alloc")]
pub fn compress(input: &[u8], level: i32) -> Result<alloc::vec::Vec<u8>, CompressError> {
    zrip_encode::compress(input, level)
}

#[cfg(feature = "alloc")]
pub fn compress_with_params(
    input: &[u8],
    params: &zrip_encode::strategy::LevelParams,
) -> Result<alloc::vec::Vec<u8>, CompressError> {
    zrip_encode::compress_with_params(input, params)
}

#[cfg(feature = "alloc")]
pub fn compress_opts(
    input: &[u8],
    level: i32,
    opts: &Options,
) -> Result<alloc::vec::Vec<u8>, CompressError> {
    zrip_encode::compress_opts(input, level, opts)
}

#[cfg(feature = "alloc")]
pub fn compress_with_dict(
    input: &[u8],
    level: i32,
    dict: &zrip_core::dict::Dictionary,
) -> Result<alloc::vec::Vec<u8>, CompressError> {
    zrip_encode::compress_with_dict(input, level, dict)
}

#[cfg(feature = "alloc")]
pub fn compress_into(input: &[u8], output: &mut [u8], level: i32) -> Result<usize, CompressError> {
    zrip_encode::compress_into(input, output, level)
}

#[must_use]
pub fn compress_bound(input_len: usize) -> usize {
    let num_blocks = input_len / zrip_core::frame::MAX_BLOCK_SIZE + 1;
    input_len
        .saturating_add(num_blocks.saturating_mul(3))
        .saturating_add(18)
}

#[cfg(feature = "std")]
pub use zrip_decode::context::DecompressContext;
#[cfg(feature = "std")]
pub use zrip_decode::streaming::FrameDecoder;
#[cfg(feature = "std")]
pub use zrip_encode::context::CompressContext;
#[cfg(feature = "std")]
pub use zrip_encode::streaming::FrameEncoder;
