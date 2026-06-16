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

#[doc(hidden)]
pub const LARGE_OUTPUT_THRESHOLD: usize = 512 * 1024;
