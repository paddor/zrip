pub mod decode;
pub(crate) mod decode_4stream;
pub mod encode;
pub(crate) mod primitives;
pub mod weights;

pub const MAX_SYMBOL_VALUE: usize = 255;
pub const MAX_BITS: u8 = 11;
pub const MAX_TABLE_LOG: u8 = 11;

#[derive(Clone, Copy, Default)]
pub struct HuffmanDecodeEntry {
    pub symbol: u8,
    pub num_bits: u8,
}
