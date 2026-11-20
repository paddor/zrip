//! Huffman coding of literals: tree descriptions, decode tables,
//! single- and 4-stream decoding, and the encoder.

pub mod decode;
pub(crate) mod decode_4stream;
pub mod encode;
pub(crate) mod primitives;
pub mod weights;

/// Largest literal byte value.
pub const MAX_SYMBOL_VALUE: usize = 255;
/// Longest code the encoder writes: 11 bits, per RFC 8878.
pub const MAX_BITS: u8 = 11;
/// Largest decode table log: 12, as C zstd's decoder (`HUF_TABLELOG_MAX`)
/// takes. RFC 8878 allows 11.
pub const MAX_TABLE_LOG: u8 = 12;
/// Largest table log of the fast decode loops, which read five codes per
/// refill. Deeper tables decode four codes per refill.
pub const FAST_TABLE_LOG: u8 = 11;
/// Decode tables hold at least this many entries, and `1 << table_log`
/// when that is more. The fixed size lets the fast loops index a
/// `[HuffmanDecodeEntry; N]` without bounds checks. Entries past
/// `1 << table_log` are unused.
pub const DECODE_TABLE_SIZE: usize = 1 << FAST_TABLE_LOG;

/// Bits every two-symbol lookup reads, whatever the tree's depth: C zstd's
/// `HUF_DECODER_FAST_TABLELOG` for trees of at most 11 bits.
pub const PAIR_TABLE_LOG: u8 = 11;
/// Rows of a two-symbol table.
pub const PAIR_TABLE_SIZE: usize = 1 << PAIR_TABLE_LOG;

/// One decode table cell: the symbol whose code prefixes this index.
#[derive(Clone, Copy, Default)]
pub struct HuffmanDecodeEntry {
    /// Decoded byte.
    pub symbol: u8,
    /// Code length in bits, the bits this symbol consumes.
    pub num_bits: u8,
}
