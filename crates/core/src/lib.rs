#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(optimize_attribute))]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod error;
pub mod hint;

pub mod bitstream;
pub mod fse;
pub mod hash;
pub mod huffman;
pub mod xxhash;

pub mod block;
pub mod frame;

pub mod simd;

pub mod sequence;
pub use sequence::Sequence;

#[cfg(feature = "alloc")]
pub mod dict;

pub use error::{CompressError, DecompressError, ZstdError};

pub const DEFAULT_DECOMPRESS_LIMIT: usize = usize::MAX;

/// A conservative output limit (128 MiB) suitable for decompressing untrusted input.
///
/// Use this with [`decompress_with_limit`](crate) or [`DecompressContext::decompress_with_limit`]
/// when processing data from untrusted sources to bound memory usage.
pub const SAFE_DECOMPRESS_LIMIT: usize = 128 * 1024 * 1024;

#[doc(hidden)]
pub const LARGE_OUTPUT_THRESHOLD: usize = 512 * 1024;
