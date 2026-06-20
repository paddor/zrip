#[cfg(feature = "alloc")]
use alloc::borrow::Cow;
#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::block_encoder::{self, BlockEncodeWorkspace};
use crate::strategy::{self, LevelParams, Strategy};
use crate::{block_looks_incompressible, clamp_params_to_src_size, dfast, fast};
use zrip_core::Sequence;
use zrip_core::dict::Dictionary;
use zrip_core::error::CompressError;
use zrip_core::frame::{MAX_BLOCK_SIZE, ZSTD_MAGIC};
use zrip_core::huffman::encode::HuffmanEncodeTable;
use zrip_core::xxhash::xxh64;

/// Pre-computed dictionary state for hot-loop compression.
///
/// Built once from a [`Dictionary`] + [`LevelParams`]. Caches the pre-filled
/// hash table(s) and a combined buffer with the dict prefix already loaded,
/// plus encode-side entropy tables built from the dict's decode tables.
pub(crate) struct PreparedDict {
    combined: Vec<u8>,
    hash_snapshot: Vec<u32>,
    hash_long_snapshot: Vec<u32>,
    prefix_len: usize,
    rep_offsets: [u32; 3],
    dict_id: u32,
    huf_table: Option<HuffmanEncodeTable>,
}

impl PreparedDict {
    pub fn new(dict: &Dictionary, params: &LevelParams) -> Self {
        let prefix = dict.content();
        let prefix_len = prefix.len();

        let mut combined = Vec::with_capacity(prefix_len + MAX_BLOCK_SIZE);
        combined.extend_from_slice(prefix);

        let (hash_snapshot, hash_long_snapshot) = match params.strategy {
            Strategy::Fast => {
                let hash_size = 1usize << params.hash_log;
                let mut hash_table = vec![0u32; hash_size];
                fast::prefill_hash_table(&combined, prefix_len, params.hash_log, &mut hash_table);
                (hash_table, Vec::new())
            }
            Strategy::DFast => {
                let short_size = 1usize << params.chain_log;
                let long_size = 1usize << params.hash_log;
                let mut hash_short = vec![0u32; short_size];
                let mut hash_long = vec![0u32; long_size];
                dfast::prefill_hash_tables(
                    &combined,
                    prefix_len,
                    params.hash_log,
                    params.chain_log,
                    params.min_match,
                    &mut hash_short,
                    &mut hash_long,
                );
                (hash_short, hash_long)
            }
        };

        let huf_table = dict
            .huf_table()
            .and_then(|(dt, tl)| HuffmanEncodeTable::from_decode_table(dt, tl));

        Self {
            combined,
            hash_snapshot,
            hash_long_snapshot,
            prefix_len,
            rep_offsets: *dict.rep_offsets(),
            dict_id: dict.id(),
            huf_table,
        }
    }
}

/// Reusable compression context that amortizes hash table and buffer allocations.
///
/// Holds internal state (hash tables, output buffer, block encoder workspace)
/// across calls. Useful when compressing many small inputs in a loop.
///
/// ```
/// let mut ctx = zrip::CompressContext::new(1).unwrap();
/// for i in 0..10 {
///     let data = format!("message {i}").repeat(100);
///     let compressed = ctx.compress(data.as_bytes()).unwrap();
///     assert!(compressed.len() < data.len());
/// }
/// ```
pub struct CompressContext {
    params: LevelParams,
    prepared: Option<PreparedDict>,
    hash_table: Vec<u32>,
    hash_long: Vec<u32>,
    sequences: Vec<Sequence>,
    output: Vec<u8>,
    workspace: BlockEncodeWorkspace,
    combined: Vec<u8>,
}

impl CompressContext {
    /// Creates a new context for the given compression level (-7..=4).
    pub fn new(level: i32) -> Result<Self, CompressError> {
        let params = strategy::level_params(level).ok_or(CompressError::InvalidLevel(level))?;
        let (hash_table, hash_long) = match params.strategy {
            Strategy::Fast => (vec![0u32; 1usize << params.hash_log], Vec::new()),
            Strategy::DFast => (
                vec![0u32; 1usize << params.chain_log],
                vec![0u32; 1usize << params.hash_log],
            ),
        };
        Ok(Self {
            params,
            prepared: None,
            hash_table,
            hash_long,
            sequences: Vec::new(),
            output: Vec::new(),
            workspace: BlockEncodeWorkspace::new(),
            combined: Vec::new(),
        })
    }

    /// Creates a new context with a pre-loaded dictionary.
    pub fn with_dict(level: i32, dict: Dictionary) -> Result<Self, CompressError> {
        let params = strategy::level_params(level).ok_or(CompressError::InvalidLevel(level))?;
        let prepared = PreparedDict::new(&dict, &params);
        let hash_table = vec![0u32; prepared.hash_snapshot.len()];
        let hash_long = vec![0u32; prepared.hash_long_snapshot.len()];
        Ok(Self {
            params,
            prepared: Some(prepared),
            hash_table,
            hash_long,
            sequences: Vec::new(),
            output: Vec::new(),
            workspace: BlockEncodeWorkspace::new(),
            combined: Vec::new(),
        })
    }

    /// Compresses `input` using the context's level and optional dictionary.
    pub fn compress(&mut self, input: &[u8]) -> Result<Cow<'_, [u8]>, CompressError> {
        if self.prepared.is_some() {
            return self.compress_with_prepared(input);
        }
        compress_core(
            input,
            self.params,
            None,
            &[],
            [1u32, 4, 8],
            &mut self.hash_table,
            &mut self.hash_long,
            &mut self.sequences,
            &mut self.output,
            &mut self.workspace,
            &mut self.combined,
        )?;
        Ok(self.take_or_borrow_output())
    }

    /// Compresses `input` using an ad-hoc dictionary (overrides the stored one).
    pub fn compress_with_dict(
        &mut self,
        input: &[u8],
        dict: &Dictionary,
    ) -> Result<Cow<'_, [u8]>, CompressError> {
        compress_core(
            input,
            self.params,
            Some(dict.id()),
            dict.content(),
            *dict.rep_offsets(),
            &mut self.hash_table,
            &mut self.hash_long,
            &mut self.sequences,
            &mut self.output,
            &mut self.workspace,
            &mut self.combined,
        )?;
        Ok(self.take_or_borrow_output())
    }

    fn compress_with_prepared(&mut self, input: &[u8]) -> Result<Cow<'_, [u8]>, CompressError> {
        let mut params = self.params;
        clamp_params_to_src_size(&mut params, input.len());

        let prep = self.prepared.as_ref().unwrap();
        let snapshot_matches = match params.strategy {
            Strategy::Fast => (1usize << params.hash_log) == prep.hash_snapshot.len(),
            Strategy::DFast => {
                (1usize << params.chain_log) == prep.hash_snapshot.len()
                    && (1usize << params.hash_log) == prep.hash_long_snapshot.len()
            }
        };
        let dict_id = prep.dict_id;
        let prefix_len = prep.prefix_len;

        if !snapshot_matches {
            return self.compress_with_dict_fallback(input, dict_id, prefix_len);
        }

        let prep = self.prepared.as_mut().unwrap();
        self.hash_table.copy_from_slice(&prep.hash_snapshot);
        if !prep.hash_long_snapshot.is_empty() {
            self.hash_long.copy_from_slice(&prep.hash_long_snapshot);
        }

        prep.combined.truncate(prep.prefix_len);
        prep.combined.extend_from_slice(input);

        if let Some(ref huf) = prep.huf_table {
            self.workspace.prev_huffman = Some(huf.clone());
        } else {
            self.workspace.prev_huffman = None;
        }

        self.output.clear();
        self.output.reserve(input.len() + 32);
        self.output.extend_from_slice(&ZSTD_MAGIC.to_le_bytes());

        let fcs_size = if input.len() <= 255 {
            1
        } else if input.len() <= 0xFFFF + 256 {
            2
        } else if input.len() <= 0xFFFF_FFFF {
            4
        } else {
            8
        };
        let fcs_flag = match fcs_size {
            1 => 0,
            2 => 1,
            4 => 2,
            8 => 3,
            _ => unreachable!(),
        };

        let dict_id = prep.dict_id;
        let dict_id_flag = if dict_id <= 0xFF {
            1u8
        } else if dict_id <= 0xFFFF {
            2
        } else {
            3
        };
        let descriptor = 0x20 | 0x04 | (fcs_flag << 6) | dict_id_flag;
        self.output.push(descriptor);
        match dict_id_flag {
            1 => self.output.push(dict_id as u8),
            2 => self
                .output
                .extend_from_slice(&(dict_id as u16).to_le_bytes()),
            3 => self.output.extend_from_slice(&dict_id.to_le_bytes()),
            _ => unreachable!(),
        }

        match fcs_size {
            1 => self.output.push(input.len() as u8),
            2 => {
                let v = (input.len() - 256) as u16;
                self.output.extend_from_slice(&v.to_le_bytes());
            }
            4 => self
                .output
                .extend_from_slice(&(input.len() as u32).to_le_bytes()),
            8 => self
                .output
                .extend_from_slice(&(input.len() as u64).to_le_bytes()),
            _ => unreachable!(),
        }

        if input.is_empty() {
            block_encoder::encode_raw_block(&[], true, &mut self.output);
        } else {
            let prefix_len = prep.prefix_len;
            let combined = &prep.combined;
            let mut rep_offsets = prep.rep_offsets;

            if input.len() <= MAX_BLOCK_SIZE {
                match params.strategy {
                    Strategy::Fast => {
                        fast::compress_fast_block(
                            combined,
                            prefix_len,
                            prefix_len + input.len(),
                            &params,
                            &rep_offsets,
                            &mut self.hash_table,
                            &mut self.sequences,
                        );
                    }
                    Strategy::DFast => {
                        dfast::compress_dfast_block(
                            combined,
                            prefix_len,
                            prefix_len + input.len(),
                            &params,
                            &rep_offsets,
                            &mut self.hash_table,
                            &mut self.hash_long,
                            &mut self.sequences,
                        );
                    }
                }
                if params.force_raw_literals {
                    block_encoder::encode_compressed_block_raw(
                        input,
                        &self.sequences,
                        &mut rep_offsets,
                        true,
                        &mut self.output,
                        &mut self.workspace,
                    );
                } else {
                    block_encoder::encode_compressed_block(
                        input,
                        &self.sequences,
                        &mut rep_offsets,
                        true,
                        &mut self.output,
                        &mut self.workspace,
                    );
                }
            } else {
                let mut offset = 0;
                while offset < input.len() {
                    let chunk_size = (input.len() - offset).min(MAX_BLOCK_SIZE);
                    let is_last = offset + chunk_size >= input.len();
                    match params.strategy {
                        Strategy::Fast => {
                            fast::compress_fast_block(
                                combined,
                                prefix_len + offset,
                                prefix_len + offset + chunk_size,
                                &params,
                                &rep_offsets,
                                &mut self.hash_table,
                                &mut self.sequences,
                            );
                        }
                        Strategy::DFast => {
                            dfast::compress_dfast_block(
                                combined,
                                prefix_len + offset,
                                prefix_len + offset + chunk_size,
                                &params,
                                &rep_offsets,
                                &mut self.hash_table,
                                &mut self.hash_long,
                                &mut self.sequences,
                            );
                        }
                    }
                    if params.force_raw_literals {
                        block_encoder::encode_compressed_block_raw(
                            &input[offset..offset + chunk_size],
                            &self.sequences,
                            &mut rep_offsets,
                            is_last,
                            &mut self.output,
                            &mut self.workspace,
                        );
                    } else {
                        block_encoder::encode_compressed_block(
                            &input[offset..offset + chunk_size],
                            &self.sequences,
                            &mut rep_offsets,
                            is_last,
                            &mut self.output,
                            &mut self.workspace,
                        );
                    }
                    offset += chunk_size;
                }
            }
        }

        let hash = xxh64(input, 0);
        let checksum = (hash & 0xFFFF_FFFF) as u32;
        self.output.extend_from_slice(&checksum.to_le_bytes());

        Ok(self.take_or_borrow_output())
    }

    fn compress_with_dict_fallback(
        &mut self,
        input: &[u8],
        dict_id: u32,
        prefix_len: usize,
    ) -> Result<Cow<'_, [u8]>, CompressError> {
        let prep = self.prepared.as_ref().unwrap();
        let rep_offsets = prep.rep_offsets;
        let prefix = &prep.combined[..prefix_len];

        compress_core(
            input,
            self.params,
            Some(dict_id),
            prefix,
            rep_offsets,
            &mut self.hash_table,
            &mut self.hash_long,
            &mut self.sequences,
            &mut self.output,
            &mut self.workspace,
            &mut self.combined,
        )?;
        Ok(self.take_or_borrow_output())
    }

    fn take_or_borrow_output(&mut self) -> Cow<'_, [u8]> {
        if self.output.len() >= zrip_core::LARGE_OUTPUT_THRESHOLD {
            Cow::Owned(core::mem::take(&mut self.output))
        } else {
            Cow::Borrowed(&self.output)
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::unnecessary_wraps)]
fn compress_core(
    input: &[u8],
    params: LevelParams,
    dict_id: Option<u32>,
    prefix: &[u8],
    init_rep_offsets: [u32; 3],
    hash_table: &mut Vec<u32>,
    hash_long: &mut Vec<u32>,
    sequences: &mut Vec<Sequence>,
    output: &mut Vec<u8>,
    workspace: &mut BlockEncodeWorkspace,
    combined: &mut Vec<u8>,
) -> Result<(), CompressError> {
    let mut params = params;
    clamp_params_to_src_size(&mut params, input.len());
    let hash_size = match params.strategy {
        Strategy::Fast => 1usize << params.hash_log,
        Strategy::DFast => 1usize << params.chain_log,
    };
    let long_size = 1usize << params.hash_log;

    workspace.prev_huffman = None;

    output.clear();
    output.reserve(input.len() + 32);
    output.extend_from_slice(&ZSTD_MAGIC.to_le_bytes());

    let fcs_size = if input.len() <= 255 {
        1
    } else if input.len() <= 0xFFFF + 256 {
        2
    } else if input.len() <= 0xFFFF_FFFF {
        4
    } else {
        8
    };
    let fcs_flag = match fcs_size {
        1 => 0,
        2 => 1,
        4 => 2,
        8 => 3,
        _ => unreachable!(),
    };

    if let Some(did) = dict_id {
        let dict_id_flag = if did <= 0xFF {
            1u8
        } else if did <= 0xFFFF {
            2
        } else {
            3
        };
        let descriptor = 0x20 | 0x04 | (fcs_flag << 6) | dict_id_flag;
        output.push(descriptor);
        match dict_id_flag {
            1 => output.push(did as u8),
            2 => output.extend_from_slice(&(did as u16).to_le_bytes()),
            3 => output.extend_from_slice(&did.to_le_bytes()),
            _ => unreachable!(),
        }
    } else {
        let descriptor = 0x20 | 0x04 | (fcs_flag << 6);
        output.push(descriptor);
    }

    match fcs_size {
        1 => output.push(input.len() as u8),
        2 => {
            let v = (input.len() - 256) as u16;
            output.extend_from_slice(&v.to_le_bytes());
        }
        4 => output.extend_from_slice(&(input.len() as u32).to_le_bytes()),
        8 => output.extend_from_slice(&(input.len() as u64).to_le_bytes()),
        _ => unreachable!(),
    }

    if input.is_empty() {
        block_encoder::encode_raw_block(&[], true, output);
    } else {
        let has_prefix = !prefix.is_empty();
        let mut rep_offsets = init_rep_offsets;
        let mut offset = 0;

        if hash_table.len() != hash_size {
            hash_table.resize(hash_size, 0);
        }

        match params.strategy {
            Strategy::Fast => {
                if has_prefix && input.len() <= MAX_BLOCK_SIZE {
                    fast::compress_fast_with_prefix_reuse(
                        input,
                        &params,
                        &rep_offsets,
                        prefix,
                        hash_table,
                        sequences,
                        combined,
                    );
                    if params.force_raw_literals {
                        block_encoder::encode_compressed_block_raw(
                            input,
                            sequences,
                            &mut rep_offsets,
                            true,
                            output,
                            workspace,
                        );
                    } else {
                        block_encoder::encode_compressed_block(
                            input,
                            sequences,
                            &mut rep_offsets,
                            true,
                            output,
                            workspace,
                        );
                    }
                } else if has_prefix {
                    combined.clear();
                    combined.reserve(prefix.len() + input.len());
                    combined.extend_from_slice(prefix);
                    combined.extend_from_slice(input);
                    let plen = prefix.len();
                    fast::prefill_hash_table(combined, plen, params.hash_log, hash_table);

                    while offset < input.len() {
                        let chunk_size = (input.len() - offset).min(MAX_BLOCK_SIZE);
                        let is_last = offset + chunk_size >= input.len();
                        fast::compress_fast_block(
                            combined,
                            plen + offset,
                            plen + offset + chunk_size,
                            &params,
                            &rep_offsets,
                            hash_table,
                            sequences,
                        );
                        if params.force_raw_literals {
                            block_encoder::encode_compressed_block_raw(
                                &input[offset..offset + chunk_size],
                                sequences,
                                &mut rep_offsets,
                                is_last,
                                output,
                                workspace,
                            );
                        } else {
                            block_encoder::encode_compressed_block(
                                &input[offset..offset + chunk_size],
                                sequences,
                                &mut rep_offsets,
                                is_last,
                                output,
                                workspace,
                            );
                        }
                        offset += chunk_size;
                    }
                } else {
                    hash_table.fill(0);
                    while offset < input.len() {
                        let chunk_size = (input.len() - offset).min(MAX_BLOCK_SIZE);
                        let block_end = offset + chunk_size;
                        let is_last = block_end >= input.len();

                        if block_looks_incompressible(&input[offset..block_end]) {
                            block_encoder::encode_raw_block(
                                &input[offset..block_end],
                                is_last,
                                output,
                            );
                        } else {
                            fast::compress_fast_block(
                                input,
                                offset,
                                block_end,
                                &params,
                                &rep_offsets,
                                hash_table,
                                sequences,
                            );
                            if params.force_raw_literals {
                                block_encoder::encode_compressed_block_raw(
                                    &input[offset..block_end],
                                    sequences,
                                    &mut rep_offsets,
                                    is_last,
                                    output,
                                    workspace,
                                );
                            } else {
                                block_encoder::encode_compressed_block(
                                    &input[offset..block_end],
                                    sequences,
                                    &mut rep_offsets,
                                    is_last,
                                    output,
                                    workspace,
                                );
                            }
                        }
                        offset = block_end;
                    }
                }
            }
            Strategy::DFast => {
                if hash_long.len() != long_size {
                    hash_long.resize(long_size, 0);
                }
                if has_prefix && input.len() <= MAX_BLOCK_SIZE {
                    dfast::compress_dfast_with_prefix_reuse(
                        input,
                        &params,
                        &rep_offsets,
                        prefix,
                        hash_table,
                        hash_long,
                        sequences,
                        combined,
                    );
                    block_encoder::encode_compressed_block(
                        input,
                        sequences,
                        &mut rep_offsets,
                        true,
                        output,
                        workspace,
                    );
                } else if has_prefix {
                    combined.clear();
                    combined.reserve(prefix.len() + input.len());
                    combined.extend_from_slice(prefix);
                    combined.extend_from_slice(input);
                    let plen = prefix.len();
                    dfast::prefill_hash_tables(
                        combined,
                        plen,
                        params.hash_log,
                        params.chain_log,
                        params.min_match,
                        hash_table,
                        hash_long,
                    );

                    while offset < input.len() {
                        let chunk_size = (input.len() - offset).min(MAX_BLOCK_SIZE);
                        let is_last = offset + chunk_size >= input.len();
                        dfast::compress_dfast_block(
                            combined,
                            plen + offset,
                            plen + offset + chunk_size,
                            &params,
                            &rep_offsets,
                            hash_table,
                            hash_long,
                            sequences,
                        );
                        block_encoder::encode_compressed_block(
                            &input[offset..offset + chunk_size],
                            sequences,
                            &mut rep_offsets,
                            is_last,
                            output,
                            workspace,
                        );
                        offset += chunk_size;
                    }
                } else {
                    hash_table.fill(0);
                    hash_long.fill(0);
                    while offset < input.len() {
                        let chunk_size = (input.len() - offset).min(MAX_BLOCK_SIZE);
                        let block_end = offset + chunk_size;
                        let is_last = block_end >= input.len();

                        if block_looks_incompressible(&input[offset..block_end]) {
                            block_encoder::encode_raw_block(
                                &input[offset..block_end],
                                is_last,
                                output,
                            );
                        } else {
                            dfast::compress_dfast_block(
                                input,
                                offset,
                                block_end,
                                &params,
                                &rep_offsets,
                                hash_table,
                                hash_long,
                                sequences,
                            );
                            block_encoder::encode_compressed_block(
                                &input[offset..block_end],
                                sequences,
                                &mut rep_offsets,
                                is_last,
                                output,
                                workspace,
                            );
                        }
                        offset = block_end;
                    }
                }
            }
        }
    }

    let hash = xxh64(input, 0);
    let checksum = (hash & 0xFFFF_FFFF) as u32;
    output.extend_from_slice(&checksum.to_le_bytes());

    Ok(())
}
