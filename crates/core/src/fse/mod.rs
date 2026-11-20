//! FSE (tANS) entropy coding: table descriptions, encode and decode tables,
//! and the predefined sequence tables of RFC 8878.

pub mod decode;
pub mod encode;
pub mod table_builder;

/// One decode table cell: the symbol of this state and how to reach the
/// next one.
#[derive(Clone, Copy)]
pub struct FseDecodeEntry {
    /// Next state before the read bits are added.
    pub base_line: u16,
    /// Bits to read for the next state.
    pub num_bits: u8,
    /// Symbol this state decodes.
    pub symbol: u8,
}

/// Sequence decode table cell: an [`FseDecodeEntry`] with the symbol
/// turned into a value base and extra-bit count. 8 bytes, fields in
/// declared order.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct FseSeqDecodeEntry {
    /// Next state before the read bits are added.
    pub base_line: u16,
    /// Bits to read for the next state.
    pub num_bits: u8,
    /// Extra bits added to `baseline_value`. For offsets, the code itself.
    pub extra_bits: u8,
    /// Value before the extra bits. For offsets, `1 << code`.
    pub baseline_value: u32,
}

/// Largest symbol an FSE table can hold.
pub const MAX_SYMBOL: usize = 255;
/// Largest accuracy log a table description may give (C zstd's
/// `FSE_MAX_TABLELOG`).
pub const MAX_TABLE_LOG: u8 = 12;
/// Smallest accuracy log. Descriptions store `log - 5` in 4 bits.
pub const MIN_TABLE_LOG: u8 = 5;

/// Largest literal length accuracy log RFC 8878 allows.
pub const LL_MAX_ACCURACY_LOG: u8 = 9;
/// Largest match length accuracy log RFC 8878 allows.
pub const ML_MAX_ACCURACY_LOG: u8 = 9;
/// Largest offset accuracy log RFC 8878 allows.
pub const OF_MAX_ACCURACY_LOG: u8 = 8;

/// Largest literal length code.
pub const LL_MAX_SYMBOL: u8 = 35;
/// Largest match length code.
pub const ML_MAX_SYMBOL: u8 = 52;
/// Largest offset code.
pub const OF_MAX_SYMBOL: u8 = 31;

/// Accuracy log of the predefined literal length table.
pub const LL_DEFAULT_ACCURACY: u8 = 6;
/// Accuracy log of the predefined match length table.
pub const ML_DEFAULT_ACCURACY: u8 = 6;
/// Accuracy log of the predefined offset table.
pub const OF_DEFAULT_ACCURACY: u8 = 5;

/// Predefined literal length distribution (RFC 8878). -1 is a "less than
/// one" probability: one cell.
pub static LL_DEFAULT_DIST: [i16; 36] = [
    4, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 2, 1, 1, 1, 1, 1,
    -1, -1, -1, -1,
];

/// Predefined match length distribution (RFC 8878).
pub static ML_DEFAULT_DIST: [i16; 53] = [
    1, 4, 3, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1, -1, -1,
];

/// Predefined offset distribution (RFC 8878), for codes 0 to 28.
pub static OF_DEFAULT_DIST: [i16; 29] = [
    1, 1, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1,
];

/// Extra bits per literal length code.
pub static LL_BITS_TABLE: [u8; 36] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 3, 3, 4, 6, 7, 8, 9, 10, 11,
    12, 13, 14, 15, 16,
];

/// Literal length per code before its extra bits are added.
pub static LL_BASELINE_TABLE: [u32; 36] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 18, 20, 22, 24, 28, 32, 40, 48, 64,
    128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536,
];

/// Extra bits per match length code.
pub static ML_BITS_TABLE: [u8; 53] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    1, 1, 1, 1, 2, 2, 3, 3, 4, 4, 5, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
];

/// Match length per code before its extra bits are added.
pub static ML_BASELINE_TABLE: [u32; 53] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27,
    28, 29, 30, 31, 32, 33, 34, 35, 37, 39, 41, 43, 47, 51, 59, 67, 83, 99, 131, 259, 515, 1027,
    2051, 4099, 8195, 16387, 32771, 65539,
];

/// Largest of the three sequence accuracy logs.
const fn max_seq_accuracy() -> u8 {
    let mut m = LL_MAX_ACCURACY_LOG;
    if ML_MAX_ACCURACY_LOG > m {
        m = ML_MAX_ACCURACY_LOG;
    }
    if OF_MAX_ACCURACY_LOG > m {
        m = OF_MAX_ACCURACY_LOG;
    }
    m
}

/// Cells in every sequence decode table, whatever its log: room for the
/// largest. The fixed size lets masked lookups skip bounds checks.
pub const FSE_SEQ_TABLE_CAPACITY: usize = 1 << max_seq_accuracy();
/// Keeps a state index inside [`FSE_SEQ_TABLE_CAPACITY`].
pub const FSE_SEQ_TABLE_MASK: u32 = FSE_SEQ_TABLE_CAPACITY as u32 - 1;
