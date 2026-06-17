#![forbid(unsafe_code)]

#[cfg(feature = "dict_builder")]
pub mod cover;
#[cfg(feature = "dict_builder")]
pub mod fastcover;
#[cfg(feature = "dict_builder")]
pub mod finalize;

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::bitstream::reader::BitReader;
use crate::error::DecompressError;
use crate::fse::table_builder::{build_decode_table, parse_fse_table_description};
use crate::fse::{FseDecodeEntry, LL_MAX_SYMBOL, ML_MAX_SYMBOL, OF_MAX_SYMBOL};
use crate::huffman::HuffmanDecodeEntry;
use crate::huffman::weights::{build_huffman_decode_table, parse_huffman_weights};

pub const DICT_MAGIC: u32 = 0xEC30A437;

/// A pre-trained zstd dictionary for improved compression of small data.
///
/// Load from raw bytes with [`Dictionary::from_bytes`], or train with
/// [`train_dict_fastcover`] (requires `dict_builder` feature).
#[cfg(feature = "alloc")]
#[derive(Clone)]
pub struct Dictionary {
    id: u32,
    content: Vec<u8>,
    huf_table: Option<(Vec<HuffmanDecodeEntry>, u8)>,
    of_table: Option<(Vec<FseDecodeEntry>, u8)>,
    ml_table: Option<(Vec<FseDecodeEntry>, u8)>,
    ll_table: Option<(Vec<FseDecodeEntry>, u8)>,
    rep_offsets: [u32; 3],
}

#[cfg(feature = "alloc")]
impl Dictionary {
    /// Parses a dictionary from its raw byte representation.
    pub fn from_bytes(data: &[u8]) -> Result<Self, DecompressError> {
        if data.len() < 8 {
            return Err(DecompressError::InvalidDictionary);
        }

        let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        if magic != DICT_MAGIC {
            return Err(DecompressError::InvalidDictionary);
        }

        let id = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        if id == 0 {
            return Err(DecompressError::InvalidDictionary);
        }
        let mut pos = 8;

        let huf_table = parse_dict_huffman(&data[pos..])?;
        pos += huf_table.1;
        let huf_decode = if huf_table.0.is_some() {
            huf_table.0
        } else {
            None
        };

        let (of_table, of_consumed) = parse_dict_fse(&data[pos..], OF_MAX_SYMBOL)?;
        pos += of_consumed;

        let (ml_table, ml_consumed) = parse_dict_fse(&data[pos..], ML_MAX_SYMBOL)?;
        pos += ml_consumed;

        let (ll_table, ll_consumed) = parse_dict_fse(&data[pos..], LL_MAX_SYMBOL)?;
        pos += ll_consumed;

        if pos + 12 > data.len() {
            return Err(DecompressError::InvalidDictionary);
        }

        let rep1 = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        let rep2 = u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]);
        let rep3 =
            u32::from_le_bytes([data[pos + 8], data[pos + 9], data[pos + 10], data[pos + 11]]);
        pos += 12;

        if rep1 == 0 || rep2 == 0 || rep3 == 0 {
            return Err(DecompressError::InvalidDictionary);
        }

        let content = data[pos..].to_vec();

        Ok(Self {
            id,
            content,
            huf_table: huf_decode,
            of_table,
            ml_table,
            ll_table,
            rep_offsets: [rep1, rep2, rep3],
        })
    }

    /// Returns the dictionary ID embedded in the header.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Returns the raw content segment used as match-finding history prefix.
    pub fn content(&self) -> &[u8] {
        &self.content
    }

    /// Returns the three initial repeat offsets stored in the dictionary.
    pub fn rep_offsets(&self) -> &[u32; 3] {
        &self.rep_offsets
    }

    /// Returns the Huffman decode table and its log2 size, if present.
    pub fn huf_table(&self) -> Option<(&[HuffmanDecodeEntry], u8)> {
        self.huf_table.as_ref().map(|(t, l)| (t.as_slice(), *l))
    }

    /// Returns the offset-code FSE decode table and accuracy log, if present.
    pub fn of_table(&self) -> Option<(&[FseDecodeEntry], u8)> {
        self.of_table.as_ref().map(|(t, l)| (t.as_slice(), *l))
    }

    /// Returns the match-length FSE decode table and accuracy log, if present.
    pub fn ml_table(&self) -> Option<(&[FseDecodeEntry], u8)> {
        self.ml_table.as_ref().map(|(t, l)| (t.as_slice(), *l))
    }

    /// Returns the literal-length FSE decode table and accuracy log, if present.
    pub fn ll_table(&self) -> Option<(&[FseDecodeEntry], u8)> {
        self.ll_table.as_ref().map(|(t, l)| (t.as_slice(), *l))
    }
}

#[cfg(feature = "alloc")]
#[allow(clippy::type_complexity)]
fn parse_dict_huffman(
    data: &[u8],
) -> Result<(Option<(Vec<HuffmanDecodeEntry>, u8)>, usize), DecompressError> {
    if data.is_empty() {
        return Err(DecompressError::InvalidDictionary);
    }

    let (weights, consumed) = parse_huffman_weights(data)?;
    if weights.is_empty() {
        return Ok((None, consumed));
    }
    let (table, table_log) = build_huffman_decode_table(&weights)?;
    Ok((Some((table, table_log)), consumed))
}

#[cfg(feature = "alloc")]
#[allow(clippy::type_complexity)]
fn parse_dict_fse(
    data: &[u8],
    max_symbol: u8,
) -> Result<(Option<(Vec<FseDecodeEntry>, u8)>, usize), DecompressError> {
    if data.is_empty() {
        return Err(DecompressError::InvalidDictionary);
    }

    let mut reader = BitReader::new(data);
    let (distribution, accuracy_log) = parse_fse_table_description(&mut reader, max_symbol)?;
    let consumed = reader.bytes_consumed();
    let table = build_decode_table(&distribution, accuracy_log)
        .map_err(|_| DecompressError::InvalidDictionary)?;
    Ok((Some((table, accuracy_log)), consumed))
}

#[cfg(feature = "dict_builder")]
/// Trains a dictionary from sample data using the FastCOVER algorithm.
pub fn train_dict_fastcover(
    samples: &[&[u8]],
    dict_size: usize,
    params: fastcover::FastCoverParams,
) -> Dictionary {
    let content = fastcover::select_segments(samples, dict_size, &params);
    let dict_bytes = finalize::finalize_dictionary(&content, samples, dict_size);
    Dictionary::from_bytes(&dict_bytes).expect("finalized dictionary must be valid")
}
