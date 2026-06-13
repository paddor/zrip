pub mod decode;
pub mod encode;
pub mod table_builder;
pub(crate) mod unchecked;

#[derive(Clone, Copy)]
pub struct FseDecodeEntry {
    pub base_line: u16,
    pub num_bits: u8,
    pub symbol: u8,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct FseSeqDecodeEntry {
    pub base_line: u16,
    pub num_bits: u8,
    pub extra_bits: u8,
    pub baseline_value: u32,
}

impl FseSeqDecodeEntry {
    #[inline(always)]
    pub fn symbol_value(
        &self,
        reader: &mut crate::bitstream::reader_reverse::ReverseBitReader,
    ) -> u32 {
        let extra = reader.read_bits_branchless(self.extra_bits);
        self.baseline_value + extra
    }
}

pub fn promote_ll_table(table: &[FseDecodeEntry]) -> Vec<FseSeqDecodeEntry> {
    table
        .iter()
        .map(|e| FseSeqDecodeEntry {
            base_line: e.base_line,
            num_bits: e.num_bits,
            extra_bits: LL_BITS_TABLE[e.symbol as usize],
            baseline_value: LL_BASELINE_TABLE[e.symbol as usize],
        })
        .collect()
}

pub fn promote_ml_table(table: &[FseDecodeEntry]) -> Vec<FseSeqDecodeEntry> {
    table
        .iter()
        .map(|e| FseSeqDecodeEntry {
            base_line: e.base_line,
            num_bits: e.num_bits,
            extra_bits: ML_BITS_TABLE[e.symbol as usize],
            baseline_value: ML_BASELINE_TABLE[e.symbol as usize],
        })
        .collect()
}

pub fn promote_of_table(table: &[FseDecodeEntry]) -> Vec<FseSeqDecodeEntry> {
    table
        .iter()
        .map(|e| {
            let code = e.symbol;
            FseSeqDecodeEntry {
                base_line: e.base_line,
                num_bits: e.num_bits,
                extra_bits: code,
                baseline_value: 1u32 << code,
            }
        })
        .collect()
}

#[derive(Clone)]
pub struct FseEncodeEntry {
    pub delta_nb_bits: u32,
    pub delta_find_state: i16,
    pub num_bits_out: u8,
}

#[derive(Clone)]
pub struct FseTable {
    pub entries: &'static [FseDecodeEntry],
    pub accuracy_log: u8,
}

pub const MAX_SYMBOL: usize = 255;
pub const MAX_TABLE_LOG: u8 = 12;
pub const MIN_TABLE_LOG: u8 = 5;

pub const LL_MAX_SYMBOL: u8 = 35;
pub const ML_MAX_SYMBOL: u8 = 52;
pub const OF_MAX_SYMBOL: u8 = 31;

pub const LL_DEFAULT_ACCURACY: u8 = 6;
pub const ML_DEFAULT_ACCURACY: u8 = 6;
pub const OF_DEFAULT_ACCURACY: u8 = 5;

pub static LL_DEFAULT_DIST: [i16; 36] = [
    4, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 2, 1, 1, 1, 1, 1,
    -1, -1, -1, -1,
];

pub static ML_DEFAULT_DIST: [i16; 53] = [
    1, 4, 3, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1, -1, -1,
];

pub static OF_DEFAULT_DIST: [i16; 29] = [
    1, 1, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1,
];

pub static LL_BITS_TABLE: [u8; 36] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 3, 3, 4, 6, 7, 8, 9, 10, 11,
    12, 13, 14, 15, 16,
];

pub static LL_BASELINE_TABLE: [u32; 36] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 18, 20, 22, 24, 28, 32, 40, 48, 64,
    128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536,
];

pub static ML_BITS_TABLE: [u8; 53] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    1, 1, 1, 1, 2, 2, 3, 3, 4, 4, 5, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
];

pub static ML_BASELINE_TABLE: [u32; 53] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27,
    28, 29, 30, 31, 32, 33, 34, 35, 37, 39, 41, 43, 47, 51, 59, 67, 83, 99, 131, 259, 515, 1027,
    2051, 4099, 8195, 16387, 32771, 65539,
];
