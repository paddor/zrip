#![forbid(unsafe_code)]

use alloc::vec::Vec;

use crate::dict::DICT_MAGIC;
use crate::fse::{LL_DEFAULT_ACCURACY, ML_DEFAULT_ACCURACY, OF_DEFAULT_ACCURACY};
use crate::fse::{LL_DEFAULT_DIST, ML_DEFAULT_DIST, OF_DEFAULT_DIST};

pub fn finalize_dictionary(content: &[u8], samples: &[&[u8]], dict_size: usize) -> Vec<u8> {
    let content_len = content.len().min(dict_size.saturating_sub(256));

    let rep_offsets = compute_rep_offsets(content, samples);
    let dict_id = compute_dict_id(content, samples);

    let mut dict = Vec::with_capacity(dict_size);

    // Magic
    dict.extend_from_slice(&DICT_MAGIC.to_le_bytes());
    // Dict ID
    dict.extend_from_slice(&dict_id.to_le_bytes());

    // Huffman table: minimal 2-symbol table
    dict.push(128 + 1); // 1 explicit symbol
    dict.push(0x11); // weight[0] = 1, padding = 1

    // FSE tables: serialize the default distributions
    let of_bytes = crate::fse::table_builder::serialize_fse_table_description(
        &OF_DEFAULT_DIST,
        OF_DEFAULT_ACCURACY,
    );
    dict.extend_from_slice(&of_bytes);
    let ml_bytes = crate::fse::table_builder::serialize_fse_table_description(
        &ML_DEFAULT_DIST,
        ML_DEFAULT_ACCURACY,
    );
    dict.extend_from_slice(&ml_bytes);
    let ll_bytes = crate::fse::table_builder::serialize_fse_table_description(
        &LL_DEFAULT_DIST,
        LL_DEFAULT_ACCURACY,
    );
    dict.extend_from_slice(&ll_bytes);

    // Repeat offsets
    dict.extend_from_slice(&rep_offsets[0].to_le_bytes());
    dict.extend_from_slice(&rep_offsets[1].to_le_bytes());
    dict.extend_from_slice(&rep_offsets[2].to_le_bytes());

    // Content
    let content_start = content.len().saturating_sub(content_len);
    dict.extend_from_slice(&content[content_start..]);

    dict
}

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

    let mut rep = [1u32, 4, 8];
    let mut found = 0usize;
    for &(offset, _) in &offset_scores {
        if offset != rep[0] && offset != rep[1] && offset != rep[2] {
            rep[found] = offset;
            found += 1;
            if found == 3 {
                break;
            }
        }
    }

    rep
}

fn compute_dict_id(content: &[u8], samples: &[&[u8]]) -> u32 {
    let mut h = 0x811c_9dc5_u32;
    for &b in content {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    for &sample in samples.iter().take(10) {
        for &b in sample {
            h ^= b as u32;
            h = h.wrapping_mul(0x0100_0193);
        }
    }
    if h == 0 {
        h = 1;
    }
    h
}
