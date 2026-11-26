//! zstd dictionaries: parsing, and FastCOVER training with the
//! `dict_builder` feature.

#![forbid(unsafe_code)]

/// Empty: zrip trains with FastCOVER only. Kept so code that names the
/// module still builds.
#[cfg(feature = "dict_builder")]
#[doc(hidden)]
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
use crate::fse::{
    FseDecodeEntry, LL_MAX_ACCURACY_LOG, LL_MAX_SYMBOL, ML_MAX_ACCURACY_LOG, ML_MAX_SYMBOL,
    OF_MAX_ACCURACY_LOG, OF_MAX_SYMBOL,
};
use crate::huffman::HuffmanDecodeEntry;
use crate::huffman::weights::{build_huffman_decode_table, parse_huffman_weights};

/// Magic number at the start of a zstd dictionary.
pub const DICT_MAGIC: u32 = 0xEC30_A437;

/// Least dictionary size, in bytes, that training builds: smaller sizes
/// count as this one. C zstd's `ZDICT_DICTSIZE_MIN`, which rejects them.
#[cfg(feature = "dict_builder")]
pub const MIN_DICT_SIZE: usize = 256;

/// A pre-trained zstd dictionary for improved compression of small data.
///
/// Load from raw bytes with [`Dictionary::from_bytes`], or train with
/// `train_dict_fastcover` (requires `dict_builder` feature).
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

        // Entropy tables in format order: Huffman, then the OF, ML, LL FSE
        // tables.
        let huf_table = parse_dict_huffman(&data[pos..])?;
        pos += huf_table.1;
        let huf_decode = if huf_table.0.is_some() {
            huf_table.0
        } else {
            None
        };

        let (of_table, of_consumed) =
            parse_dict_fse_checked(&data[pos..], OF_MAX_SYMBOL, OF_MAX_ACCURACY_LOG)?;
        pos += of_consumed;

        let (ml_table, ml_consumed) =
            parse_dict_fse_checked(&data[pos..], ML_MAX_SYMBOL, ML_MAX_ACCURACY_LOG)?;
        pos += ml_consumed;

        let (ll_table, ll_consumed) =
            parse_dict_fse_checked(&data[pos..], LL_MAX_SYMBOL, LL_MAX_ACCURACY_LOG)?;
        pos += ll_consumed;

        // Three repeat offsets, then the content.
        if pos + 12 > data.len() {
            return Err(DecompressError::InvalidDictionary);
        }

        let rep1 = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        let rep2 = u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]);
        let rep3 =
            u32::from_le_bytes([data[pos + 8], data[pos + 9], data[pos + 10], data[pos + 11]]);
        pos += 12;

        // As C zstd: each repeat offset reaches into the content.
        let content = data[pos..].to_vec();
        if [rep1, rep2, rep3]
            .iter()
            .any(|&rep| rep == 0 || rep as usize > content.len())
        {
            return Err(DecompressError::InvalidDictionary);
        }

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
fn parse_dict_fse_checked(
    data: &[u8],
    max_symbol: u8,
    max_accuracy_log: u8,
) -> Result<(Option<(Vec<FseDecodeEntry>, u8)>, usize), DecompressError> {
    if data.is_empty() {
        return Err(DecompressError::InvalidDictionary);
    }

    let mut reader = BitReader::new(data);
    let (distribution, accuracy_log) = parse_fse_table_description(&mut reader, max_symbol)?;
    if accuracy_log > max_accuracy_log {
        return Err(DecompressError::InvalidDictionary);
    }
    let consumed = reader.bytes_consumed();
    let table = build_decode_table(&distribution, accuracy_log)
        .map_err(|_| DecompressError::InvalidDictionary)?;
    Ok((Some((table, accuracy_log)), consumed))
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    use super::*;
    use crate::fse::table_builder::serialize_fse_table_description;

    fn build_dict_with_of_accuracy(accuracy_log: u8) -> Vec<u8> {
        let mut dict = Vec::new();
        dict.extend_from_slice(&DICT_MAGIC.to_le_bytes());
        dict.extend_from_slice(&1u32.to_le_bytes());
        // Huffman table: header byte 0 triggers FSE path with compressed_size=0 → error.
        // Use header byte 128 (direct mode, 1 symbol with weight in upper nibble).
        dict.push(128);
        dict.push(0x10); // symbol 0, weight = 1

        // OF FSE table with the specified accuracy_log.
        // Distribution: symbol 0 gets all probability.
        let table_size = 1i16 << accuracy_log;
        let mut dist = vec![0i16; 32];
        dist[0] = table_size;
        let of_bytes = serialize_fse_table_description(&dist, accuracy_log);
        dict.extend_from_slice(&of_bytes);

        // ML FSE table: accuracy_log = 6, symbol 0 gets all probability.
        let mut ml_dist = vec![0i16; 53];
        ml_dist[0] = 1 << 6;
        let ml_bytes = serialize_fse_table_description(&ml_dist, 6);
        dict.extend_from_slice(&ml_bytes);

        // LL FSE table: accuracy_log = 6, symbol 0 gets all probability.
        let mut ll_dist = vec![0i16; 36];
        ll_dist[0] = 1 << 6;
        let ll_bytes = serialize_fse_table_description(&ll_dist, 6);
        dict.extend_from_slice(&ll_bytes);

        // Rep offsets: [1, 4, 8]
        dict.extend_from_slice(&1u32.to_le_bytes());
        dict.extend_from_slice(&4u32.to_le_bytes());
        dict.extend_from_slice(&8u32.to_le_bytes());

        // Content: the repeat offsets reach 8 bytes back.
        dict.extend_from_slice(b"contents");
        dict
    }

    #[test]
    fn dict_rejects_oversized_of_accuracy() {
        let dict_bytes = build_dict_with_of_accuracy(10);
        let result = Dictionary::from_bytes(&dict_bytes);
        assert!(
            result.is_err(),
            "accuracy_log=10 should be rejected for OF (max=8)"
        );
    }

    #[test]
    fn dict_rejects_repeat_offsets_past_content() {
        // C zstd: `rep == 0 || rep > dictContentSize` is a corrupt dictionary.
        let valid = build_dict_with_of_accuracy(8);
        let content_len = b"contents".len();
        let reps_at = valid.len() - content_len - 12;
        for (slot, rep) in [(0, 9u32), (1, 9), (2, 9), (0, 0), (1, u32::MAX)] {
            let mut dict = valid.clone();
            dict[reps_at + 4 * slot..][..4].copy_from_slice(&rep.to_le_bytes());
            assert!(
                Dictionary::from_bytes(&dict).is_err(),
                "repeat offset {rep} in slot {slot} must be rejected"
            );
        }
        // The last repeat offset may reach the content's first byte.
        let mut dict = valid.clone();
        dict[reps_at..][..4].copy_from_slice(&(content_len as u32).to_le_bytes());
        assert!(Dictionary::from_bytes(&dict).is_ok());
    }

    #[test]
    fn dict_accepts_valid_of_accuracy() {
        let dict_bytes = build_dict_with_of_accuracy(8);
        let result = Dictionary::from_bytes(&dict_bytes);
        assert!(result.is_ok(), "accuracy_log=8 should be accepted for OF");
    }
}

#[cfg(feature = "dict_builder")]
/// Trains a dictionary from sample data using the FastCOVER algorithm.
///
/// `dict_size` is the size of the dictionary in bytes, header and tables
/// included. The dictionary fills it unless the samples are too few. Sizes
/// under [`MIN_DICT_SIZE`] count as `MIN_DICT_SIZE`.
pub fn train_dict_fastcover(
    samples: &[&[u8]],
    dict_size: usize,
    params: fastcover::FastCoverParams,
) -> Dictionary {
    let content = fastcover::select_segments(samples, dict_size, &params);
    let dict_bytes = finalize::finalize_dictionary(&content, samples, dict_size);
    Dictionary::from_bytes(&dict_bytes).expect("finalized dictionary must be valid")
}
