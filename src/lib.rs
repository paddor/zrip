//! Fast, pure-Rust zstd compression.
//!
//! zrip implements zstd compression levels -7 through 4 (Fast and `DFast` strategies),
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
//! | -7 | Fast | Fastest; raw literals and long matches only, for LZ4's niche |
//! | -6..=-1 | Fast | Fast encode, lower ratio |
//! | 0 | | Library default (currently level 1) |
//! | 1..=2 | Fast | Good balance for network transfers |
//! | 3..=4 | `DFast` | Better ratio, still fast |
//!
//! Level -8, a separate zrip level in earlier versions, runs as -7.
//! [`MIN_LEVEL`], [`MAX_LEVEL`] and [`DEFAULT_LEVEL`] name the bounds and
//! the default.
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
//! [`push::PushEncoder`] and [`push::PushDecoder`] are the push-style
//! (sans-io) counterparts: feed input chunks, drain output into your own
//! buffers. They need only `alloc`.
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
//! Without `window_log`, LDM uses a 128 MiB window. One-shot calls shrink it to
//! the input.
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
//! - **`dict_builder`**: FastCOVER dictionary training.
//! - **`simd`** (default): runtime SIMD dispatch (AVX2+BMI2, NEON, SIMD128).
//! - **`paranoid`**: keeps any future `unsafe` out; no effect today.
//! - **`nightly`**: `#[optimize]` attributes for hot paths.

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub use zrip_core::error;
pub use zrip_core::frame;
pub use zrip_core::{CompressError, DecompressError, ZstdError};
pub use zrip_encode::adaptive;

#[cfg(feature = "alloc")]
pub use zrip_core::dict;
#[cfg(feature = "alloc")]
pub use zrip_core::dict::Dictionary;
#[cfg(feature = "alloc")]
pub use zrip_encode::strategy::{
    DEFAULT_LEVEL, LdmParams, LevelParams, MAX_LEVEL, MIN_LEVEL, Options,
};

/// Output limit of the calls that take none, such as [`decompress`]: no
/// limit.
pub const DEFAULT_DECOMPRESS_LIMIT: usize = usize::MAX;
pub use zrip_core::SAFE_DECOMPRESS_LIMIT;

#[doc(hidden)]
pub use zrip_decode as decode;
#[doc(hidden)]
pub use zrip_encode as encode;

/// Decompresses every frame in `input`, skipping skippable frames.
///
/// The output size is not limited. For untrusted input use
/// [`decompress_with_limit`] with [`SAFE_DECOMPRESS_LIMIT`].
#[cfg(feature = "alloc")]
pub fn decompress(input: &[u8]) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    zrip_decode::decompress(input)
}

/// [`decompress`] for frames compressed with `dict`.
#[cfg(feature = "alloc")]
pub fn decompress_with_dict(
    input: &[u8],
    dict: &zrip_core::dict::Dictionary,
) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    zrip_decode::decompress_with_dict(input, Some(dict))
}

/// [`decompress_with_dict`] with the output capped at `max_output_size`
/// bytes. Larger output fails with [`DecompressError::OutputTooSmall`].
#[cfg(feature = "alloc")]
pub fn decompress_with_dict_and_limit(
    input: &[u8],
    dict: &zrip_core::dict::Dictionary,
    max_output_size: usize,
) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    zrip_decode::decompress_with_dict_and_limit(input, Some(dict), max_output_size)
}

/// [`decompress`] with the output capped at `max_output_size` bytes. Larger
/// output fails with [`DecompressError::OutputTooSmall`].
#[cfg(feature = "alloc")]
pub fn decompress_with_limit(
    input: &[u8],
    max_output_size: usize,
) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    zrip_decode::decompress_with_limit(input, max_output_size)
}

/// [`decompress`], appending to `output`. Returns the number of bytes
/// appended. Reusing `output` saves the allocation.
#[cfg(feature = "alloc")]
pub fn decompress_into(
    input: &[u8],
    output: &mut alloc::vec::Vec<u8>,
) -> Result<usize, DecompressError> {
    zrip_decode::decompress_into(input, output)
}

/// Compresses `input` into one frame at `level` (see [`MIN_LEVEL`] and
/// [`MAX_LEVEL`]; 0 means [`DEFAULT_LEVEL`]).
///
/// The frame carries the content size and a checksum.
/// [`CompressContext`] reuses its tables across calls.
#[cfg(feature = "alloc")]
pub fn compress(input: &[u8], level: i32) -> Result<alloc::vec::Vec<u8>, CompressError> {
    zrip_encode::compress(input, level)
}

/// [`compress`] with explicit match finder parameters instead of a level.
/// Parameters larger than the input needs are reduced to fit it.
#[cfg(feature = "alloc")]
pub fn compress_with_params(
    input: &[u8],
    params: &zrip_encode::strategy::LevelParams,
) -> Result<alloc::vec::Vec<u8>, CompressError> {
    zrip_encode::compress_with_params(input, params)
}

/// [`compress`] with [`Options`], such as a larger window and long distance
/// matching.
#[cfg(feature = "alloc")]
pub fn compress_opts(
    input: &[u8],
    level: i32,
    opts: &Options,
) -> Result<alloc::vec::Vec<u8>, CompressError> {
    zrip_encode::compress_opts(input, level, opts)
}

/// [`compress`] with `dict`. Decoders need the same dictionary.
#[cfg(feature = "alloc")]
pub fn compress_with_dict(
    input: &[u8],
    level: i32,
    dict: &zrip_core::dict::Dictionary,
) -> Result<alloc::vec::Vec<u8>, CompressError> {
    zrip_encode::compress_with_dict(input, level, dict)
}

/// [`compress`] into `output`. Returns the frame size. Fails with
/// [`CompressError::OutputTooSmall`] if the frame does not fit;
/// [`compress_bound`] is always enough.
#[cfg(feature = "alloc")]
pub fn compress_into(input: &[u8], output: &mut [u8], level: i32) -> Result<usize, CompressError> {
    zrip_encode::compress_into(input, output, level)
}

/// Worst-case frame size for `input_len` bytes of input, with or without a
/// dictionary.
#[must_use]
pub fn compress_bound(input_len: usize) -> usize {
    let num_blocks = input_len / zrip_core::frame::MAX_BLOCK_SIZE + 1;
    // Magic (4), frame header descriptor (1), window descriptor (1),
    // dictionary ID (4), content size (8), checksum (4).
    input_len
        .saturating_add(num_blocks.saturating_mul(3))
        .saturating_add(22)
}

/// Push-style (sans-io) streaming: feed input chunks, drain output into
/// your own buffers. Needs only `alloc`.
#[cfg(feature = "alloc")]
pub mod push {
    pub use zrip_core::push::{Progress, Status};
    pub use zrip_decode::push::PushDecoder;
    pub use zrip_encode::push::{EndOp, PushEncoder};
}

#[cfg(feature = "alloc")]
pub use zrip_decode::context::DecompressContext;
#[cfg(feature = "std")]
pub use zrip_decode::streaming::FrameDecoder;
#[cfg(feature = "alloc")]
pub use zrip_encode::context::CompressContext;
#[cfg(feature = "std")]
pub use zrip_encode::streaming::FrameEncoder;
