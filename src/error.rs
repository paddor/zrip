#![forbid(unsafe_code)]

use core::fmt;

/// Unified error type wrapping both compression and decompression errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZstdError {
    Compress(CompressError),
    Decompress(DecompressError),
}

/// Error returned by compression functions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompressError {
    /// Output buffer passed to [`compress_into`](crate::compress_into) is too small.
    OutputTooSmall,
    /// Compression level is outside the supported range (-7..=4).
    InvalidLevel(i32),
    /// Dictionary bytes failed to parse.
    InvalidDictionary,
}

/// Error returned by decompression functions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecompressError {
    /// Frame magic number is not `0xFD2FB528`.
    BadMagic,
    /// Frame descriptor or field sizes are invalid.
    BadFrameHeader,
    /// Block header contains invalid values.
    BadBlockHeader,
    /// Block type field is reserved/unknown.
    BadBlockType,
    /// Literals section is malformed or truncated.
    CorruptLiterals,
    /// Sequences section is malformed, or decoded output exceeds block size.
    CorruptSequences,
    /// FSE table description is invalid.
    BadFseTable,
    /// Huffman weight table is malformed.
    BadHuffmanWeights,
    /// Huffman bitstream decoding failed.
    BadHuffmanStream,
    /// Requested window size exceeds the implementation limit.
    WindowTooLarge { requested: u64, max: u64 },
    /// Decompressed output would exceed the configured size limit.
    OutputTooSmall,
    /// Content checksum does not match the decompressed data.
    ChecksumMismatch { expected: u32, got: u32 },
    /// Frame requires dictionary ID `expected`, but `got` was provided.
    DictMismatch { expected: u32, got: u32 },
    /// Frame requires a dictionary but none was provided.
    DictRequired,
    /// Dictionary bytes failed to parse.
    InvalidDictionary,
    /// Input ended before the frame was complete.
    InputExhausted,
    /// Unexpected trailing bytes after a valid frame.
    ExtraBytes,
}

impl fmt::Display for ZstdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ZstdError::Compress(e) => write!(f, "compress: {e}"),
            ZstdError::Decompress(e) => write!(f, "decompress: {e}"),
        }
    }
}

impl fmt::Display for CompressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompressError::OutputTooSmall => write!(f, "output buffer too small"),
            CompressError::InvalidLevel(l) => write!(f, "invalid compression level: {l}"),
            CompressError::InvalidDictionary => write!(f, "invalid dictionary"),
        }
    }
}

impl fmt::Display for DecompressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecompressError::BadMagic => write!(f, "invalid frame magic number"),
            DecompressError::BadFrameHeader => write!(f, "malformed frame header"),
            DecompressError::BadBlockHeader => write!(f, "malformed block header"),
            DecompressError::BadBlockType => write!(f, "unknown block type"),
            DecompressError::CorruptLiterals => write!(f, "corrupt literals section"),
            DecompressError::CorruptSequences => write!(f, "corrupt sequences section"),
            DecompressError::BadFseTable => write!(f, "invalid FSE table description"),
            DecompressError::BadHuffmanWeights => write!(f, "invalid Huffman weights"),
            DecompressError::BadHuffmanStream => write!(f, "corrupt Huffman stream"),
            DecompressError::WindowTooLarge { requested, max } => {
                write!(f, "window size {requested} exceeds max {max}")
            }
            DecompressError::OutputTooSmall => write!(f, "output buffer too small"),
            DecompressError::ChecksumMismatch { expected, got } => {
                write!(
                    f,
                    "checksum mismatch: expected {expected:#010x}, got {got:#010x}"
                )
            }
            DecompressError::DictMismatch { expected, got } => {
                write!(f, "dictionary ID mismatch: expected {expected}, got {got}")
            }
            DecompressError::DictRequired => write!(f, "dictionary required but not provided"),
            DecompressError::InvalidDictionary => write!(f, "invalid dictionary format"),
            DecompressError::InputExhausted => write!(f, "unexpected end of input"),
            DecompressError::ExtraBytes => write!(f, "extra bytes after frame"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ZstdError {}
#[cfg(feature = "std")]
impl std::error::Error for CompressError {}
#[cfg(feature = "std")]
impl std::error::Error for DecompressError {}

impl From<CompressError> for ZstdError {
    fn from(e: CompressError) -> Self {
        ZstdError::Compress(e)
    }
}

impl From<DecompressError> for ZstdError {
    fn from(e: DecompressError) -> Self {
        ZstdError::Decompress(e)
    }
}
