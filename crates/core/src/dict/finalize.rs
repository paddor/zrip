//! Turns trained content into a zstd dictionary: header, entropy tables,
//! repeat offsets, content.

#![forbid(unsafe_code)]

use alloc::vec;
use alloc::vec::Vec;

use crate::dict::{DICT_MAGIC, MIN_DICT_SIZE};
use crate::fse::{LL_DEFAULT_ACCURACY, ML_DEFAULT_ACCURACY, OF_DEFAULT_ACCURACY};
use crate::fse::{LL_DEFAULT_DIST, ML_DEFAULT_DIST, OF_DEFAULT_DIST};
use crate::huffman::encode::HuffmanEncodeTable;

/// Least content a dictionary holds: the largest default repeat offset.
const MIN_CONTENT: usize = 8;

/// Magic, dictionary ID and the three repeat offsets.
const FIXED_HEADER: usize = 4 + 4 + 12;

/// Builds a dictionary of at most `dict_size` bytes, header and tables
/// included, from the end of `content`. Sizes under [`MIN_DICT_SIZE`] count
/// as `MIN_DICT_SIZE`. The Huffman table follows the samples' literals; the
/// FSE tables are the predefined ones.
pub fn finalize_dictionary(content: &[u8], samples: &[&[u8]], dict_size: usize) -> Vec<u8> {
    let dict_size = dict_size.max(MIN_DICT_SIZE);
    let fse_tables = default_fse_tables();

    // As C zstd's `ZDICT_finalizeDictionary`: the Huffman table follows all
    // content that fits beside the fixed header and the FSE tables, then the
    // content is cut to what fits beside the Huffman table too. The table
    // codes every byte, so it holds for the shorter content.
    let tables = FIXED_HEADER + fse_tables.len();
    let candidate = &content[content.len() - content.len().min(dict_size - tables)..];
    // Every byte value counts once more, so the table codes any literals
    // and frames can always reuse it.
    let huf_table = literal_table(candidate, samples);
    let huf = huf_table.weights_description();
    let room = dict_size - tables - huf.len();
    debug_assert!(
        room >= MIN_CONTENT,
        "MIN_DICT_SIZE holds the largest tables"
    );

    let content_len = content.len().min(room);
    let content_start = content.len() - content_len;
    // As C zstd, content shorter than the largest default repeat offset is
    // padded with zeros in front, so every repeat offset reaches into it.
    let padded;
    let kept = if content_len < MIN_CONTENT {
        let mut bytes = vec![0u8; MIN_CONTENT - content_len];
        bytes.extend_from_slice(&content[content_start..]);
        padded = bytes;
        &padded[..]
    } else {
        &content[content_start..]
    };

    let rep_offsets = compute_rep_offsets(kept, samples);
    let dict_id = compute_dict_id(content);

    let mut dict = Vec::with_capacity(dict_size);

    // Magic
    dict.extend_from_slice(&DICT_MAGIC.to_le_bytes());
    // Dict ID
    dict.extend_from_slice(&dict_id.to_le_bytes());

    // Huffman table for the literals the samples leave when matched against
    // the content.
    dict.extend_from_slice(huf);

    // FSE tables
    dict.extend_from_slice(&fse_tables);

    // Repeat offsets
    dict.extend_from_slice(&rep_offsets[0].to_le_bytes());
    dict.extend_from_slice(&rep_offsets[1].to_le_bytes());
    dict.extend_from_slice(&rep_offsets[2].to_le_bytes());

    // Content
    dict.extend_from_slice(kept);

    debug_assert!(dict.len() <= dict_size);
    dict
}

/// The OF, ML and LL tables' descriptions of the predefined distributions.
fn default_fse_tables() -> Vec<u8> {
    use crate::fse::table_builder::serialize_fse_table_description as describe;
    let mut out = describe(&OF_DEFAULT_DIST, OF_DEFAULT_ACCURACY);
    out.extend_from_slice(&describe(&ML_DEFAULT_DIST, ML_DEFAULT_ACCURACY));
    out.extend_from_slice(&describe(&LL_DEFAULT_DIST, LL_DEFAULT_ACCURACY));
    out
}

/// Shortest match the literal count takes, as at levels 3 and 4.
const LITERAL_MIN_MATCH: usize = 5;

/// Nearly flat counts whose table has a description, as C zstd's
/// `ZDICT_flatLit`. A flat table over all 256 bytes has none: every weight
/// is the same, which the FSE-compressed form cannot code, and the direct
/// form holds only 128 weights.
const FLAT_LITERALS: [u32; 256] = {
    let mut counts = [2; 256];
    counts[0] = 4;
    counts[253] = 1;
    counts[254] = 1;
    counts
};

/// A Huffman table with a code for every byte, weighted by the literals of
/// `samples` compressed with `content` as their dictionary. Samples whose
/// literals are too few or too flat for a describable table get
/// [`FLAT_LITERALS`]'s, as in C zstd.
fn literal_table(content: &[u8], samples: &[&[u8]]) -> HuffmanEncodeTable {
    let counts = literal_counts(content, samples);
    HuffmanEncodeTable::from_histogram(&counts)
        .or_else(|| HuffmanEncodeTable::from_histogram(&FLAT_LITERALS))
        .expect("flat literal counts build a table")
}

/// Byte counts, plus one each, of what greedy matching leaves as literals in
/// each sample: matches of at least [`LITERAL_MIN_MATCH`] bytes in `content`
/// or earlier in the same sample. C zstd counts them by compressing the
/// samples with the dictionary.
fn literal_counts(content: &[u8], samples: &[&[u8]]) -> [u32; 256] {
    const CONTENT_LOG: u32 = 16;
    const SAMPLE_LOG: u32 = 12;
    let hash = |b: &[u8], log: u32| {
        let v = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        (v.wrapping_mul(0x9E37_79B1) >> (32 - log)) as usize
    };
    let common = |a: &[u8], b: &[u8]| a.iter().zip(b).take_while(|(x, y)| x == y).count();

    let mut in_content = alloc::vec![usize::MAX; 1 << CONTENT_LOG];
    for i in 0..content.len().saturating_sub(LITERAL_MIN_MATCH - 1) {
        in_content[hash(&content[i..], CONTENT_LOG)] = i;
    }

    let mut counts = [1u32; 256];
    let mut in_sample = alloc::vec![usize::MAX; 1 << SAMPLE_LOG];
    for sample in samples {
        in_sample.fill(usize::MAX);
        let mut i = 0;
        while i < sample.len() {
            let mut len = 0;
            if i + LITERAL_MIN_MATCH <= sample.len() {
                let rest = &sample[i..];
                let c = in_content[hash(rest, CONTENT_LOG)];
                if c != usize::MAX {
                    len = common(rest, &content[c..]);
                }
                let h = hash(rest, SAMPLE_LOG);
                let p = in_sample[h];
                if p != usize::MAX {
                    len = len.max(common(rest, &sample[p..]));
                }
                in_sample[h] = i;
            }
            if len >= LITERAL_MIN_MATCH {
                i += len;
            } else {
                counts[usize::from(sample[i])] = counts[usize::from(sample[i])].saturating_add(1);
                i += 1;
            }
        }
    }
    counts
}

/// Repeat offsets: the distances back from the end of `content` at which
/// sample starts match most (4 bytes or more), in place of the defaults
/// `[1, 4, 8]`. C zstd always writes the defaults. `content` is what the
/// dictionary keeps, at least [`MIN_CONTENT`] bytes, so each offset reaches
/// into it.
fn compute_rep_offsets(content: &[u8], samples: &[&[u8]]) -> [u32; 3] {
    let max_offset = content.len().min(1024);
    if max_offset == 0 {
        return [1, 4, 8];
    }

    // For each offset, count total match bytes across samples
    let mut offset_scores = Vec::with_capacity(max_offset);
    let content_len = content.len();

    for offset in 1..=max_offset as u32 {
        let mut total_matches = 0u32;

        for &sample in samples.iter().take(50) {
            let check_len = sample.len().min(128);
            let start_in_content = content_len - offset as usize;
            let comparable = check_len.min(content_len - start_in_content);
            let content_slice = &content[start_in_content..start_in_content + comparable];
            let sample_slice = &sample[..comparable];

            let mut m = 0;
            while m < comparable && content_slice[m] == sample_slice[m] {
                m += 1;
            }
            if m >= 4 {
                total_matches += m as u32;
            }
        }

        if total_matches > 0 {
            offset_scores.push((offset, total_matches));
        }
    }

    offset_scores.sort_unstable_by_key(|e| core::cmp::Reverse(e.1));

    // The three best offsets, then the defaults they leave out.
    let mut rep = [0u32; 3];
    let mut found = 0usize;
    for offset in offset_scores
        .iter()
        .map(|&(offset, _)| offset)
        .chain([1, 4, 8])
    {
        if found == 3 {
            break;
        }
        if !rep[..found].contains(&offset) {
            rep[found] = offset;
            found += 1;
        }
    }
    rep
}

/// As C zstd (`ZDICT_finalizeDictionary`): XXH64 of `content`, mapped into
/// `32768..2^31`. RFC 8878 reserves the IDs below and above.
fn compute_dict_id(content: &[u8]) -> u32 {
    let id = crate::xxhash::xxh64(content, 0) % ((1 << 31) - 32768) + 32768;
    id as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_content_is_padded_to_the_repeat_offsets() {
        for len in 0..12usize {
            let content: Vec<u8> = (1..=len as u8).collect();
            let dict = finalize_dictionary(&content, &[b"sample bytes"], 4096);
            let parsed = crate::dict::Dictionary::from_bytes(&dict).expect("valid dictionary");
            assert_eq!(parsed.content().len(), len.max(MIN_CONTENT));
            assert!(parsed.content().ends_with(&content));
        }
    }

    #[test]
    fn repeat_offsets_stay_inside_the_kept_content() {
        // A 512-byte dictionary keeps less than 512 bytes of content. The
        // sample matches 600 bytes before the content's end, outside of them.
        let content: Vec<u8> = (0..2048u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        let sample = &content[2048 - 600..2048 - 500];
        let dict = finalize_dictionary(&content, &[sample], 512);
        let parsed = crate::dict::Dictionary::from_bytes(&dict).expect("valid dictionary");
        assert_eq!(dict.len(), 512);
        assert!(parsed.content().len() < 512);
        assert_eq!(*parsed.rep_offsets(), [1, 4, 8]);
    }

    #[test]
    fn dictionaries_fill_the_size_asked_for() {
        // As C zstd: the content takes what the header and tables leave.
        // Sizes under the least count as the least.
        let content: Vec<u8> = (0..65536u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        let samples = [&content[1000..1300], &content[40_000..40_200]];
        for size in [0, 1, 16, 64, 127, 255, 256, 257, 300, 1024, 16384] {
            let dict = finalize_dictionary(&content, &samples, size);
            assert_eq!(dict.len(), size.max(MIN_DICT_SIZE), "size {size}");
            let parsed = crate::dict::Dictionary::from_bytes(&dict).expect("valid dictionary");
            assert!(content.ends_with(parsed.content()), "size {size}");
        }
    }

    #[test]
    fn the_largest_tables_fit_the_least_size() {
        // A Huffman description takes at most 128 bytes: one header byte
        // below 128, then that many bytes.
        let largest = FIXED_HEADER + default_fse_tables().len() + 128;
        assert!(largest + MIN_CONTENT <= MIN_DICT_SIZE, "{largest}");
    }

    #[test]
    fn flat_literal_counts_build_a_table() {
        assert!(HuffmanEncodeTable::from_histogram(&[1; 256]).is_none());
        let table = HuffmanEncodeTable::from_histogram(&FLAT_LITERALS).unwrap();
        assert!(table.estimate_bits(&[1; 256]).is_some(), "codes every byte");
    }

    #[test]
    fn samples_without_literals_get_the_flat_table() {
        let content: Vec<u8> = (0..4096u32).map(|i| (i * 7 % 251) as u8).collect();
        let flat = HuffmanEncodeTable::from_histogram(&FLAT_LITERALS).unwrap();
        for samples in [&[][..], &[&b""[..]], &[&content[100..200]]] {
            let table = literal_table(&content, samples);
            assert_eq!(table.weights_description(), flat.weights_description());
        }
    }
}
