#[cfg(feature = "alloc")]
use alloc::borrow::Cow;
#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::decode::sequences::Sequence;
use crate::dict::Dictionary;
use crate::encode::block_encoder::{self, BlockEncodeWorkspace};
use crate::encode::strategy::{self, LevelParams, Strategy};
use crate::encode::{block_looks_incompressible, clamp_params_to_src_size, dfast, fast};
use crate::error::CompressError;
use crate::frame::{MAX_BLOCK_SIZE, ZSTD_MAGIC};
use crate::xxhash::xxh64;

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
    dict: Option<Dictionary>,
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
        let hash_size = 1usize << params.hash_log;
        let (hash_table, hash_long) = match params.strategy {
            Strategy::Fast => (vec![0u32; hash_size], Vec::new()),
            Strategy::DFast => (vec![0u32; hash_size], vec![0u32; hash_size]),
        };
        Ok(Self {
            params,
            dict: None,
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
        let mut ctx = Self::new(level)?;
        ctx.dict = Some(dict);
        Ok(ctx)
    }

    /// Compresses `input` using the context's level and optional dictionary.
    pub fn compress(&mut self, input: &[u8]) -> Result<Cow<'_, [u8]>, CompressError> {
        let (dict_id, prefix, init_rep) = if let Some(ref d) = self.dict {
            (Some(d.id()), d.content(), *d.rep_offsets())
        } else {
            (None, &[] as &[u8], [1u32, 4, 8])
        };
        compress_core(
            input,
            self.params,
            dict_id,
            prefix,
            init_rep,
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

    fn take_or_borrow_output(&mut self) -> Cow<'_, [u8]> {
        if self.output.len() >= crate::LARGE_OUTPUT_THRESHOLD {
            Cow::Owned(core::mem::take(&mut self.output))
        } else {
            Cow::Borrowed(&self.output)
        }
    }
}

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
    let hash_size = 1usize << params.hash_log;

    workspace.prev_huffman = None;

    output.clear();
    output.reserve(input.len() + 32);
    output.extend_from_slice(&ZSTD_MAGIC.to_le_bytes());

    let fcs_size = if input.len() <= 255 {
        1
    } else if input.len() <= 0xFFFF + 256 {
        2
    } else if input.len() <= 0xFFFFFFFF {
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
            Strategy::DFast => {
                if hash_long.len() != hash_size {
                    hash_long.resize(hash_size, 0);
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
    let checksum = (hash & 0xFFFFFFFF) as u32;
    output.extend_from_slice(&checksum.to_le_bytes());

    Ok(())
}
