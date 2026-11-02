#[cfg(feature = "alloc")]
use alloc::borrow::Cow;
#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::block_encoder::{self, BlockEncodeWorkspace};
use crate::fast::AttachedDict;
use crate::output::{OutputSink, SliceSink};
use crate::rebase::{self, PositionBase};
use crate::strategy::{self, LevelParams, Strategy};
use crate::{dfast, fast, skip_match_search, write_frame_header_with_checksum};
use zrip_core::Sequence;
use zrip_core::dict::Dictionary;
use zrip_core::error::CompressError;
use zrip_core::frame::MAX_BLOCK_SIZE;
use zrip_core::huffman::encode::HuffmanEncodeTable;
use zrip_core::xxhash::{Xxh64State, xxh64};

/// Largest Fast-strategy input that keeps the dictionary in its own table
/// instead of copying the prefilled table, as C zstd does for its fast
/// strategy.
const ATTACH_THRESHOLD: usize = 8192;

/// Pre-computed dictionary state for hot-loop compression.
///
/// Built once from a [`Dictionary`] + [`LevelParams`]. Caches the pre-filled
/// hash table(s) and a combined buffer with the dict prefix already loaded,
/// plus encode-side entropy tables built from the dict's decode tables.
pub(crate) struct PreparedDict {
    combined: Vec<u8>,
    hash_snapshot: Vec<u32>,
    hash_long_snapshot: Vec<u32>,
    /// Fast-strategy dictionary tables for attached inputs, one per minimum
    /// match the level uses, sized to the dictionary content.
    attached: Vec<(u32, Vec<u32>)>,
    attached_hash_log: u32,
    prefix_len: usize,
    rep_offsets: [u32; 3],
    dict_id: u32,
    huf_table: Option<HuffmanEncodeTable>,
    ll_table: Option<block_encoder::FseEncodeTable>,
    of_table: Option<block_encoder::FseEncodeTable>,
    ml_table: Option<block_encoder::FseEncodeTable>,
}

impl PreparedDict {
    pub fn new(dict: &Dictionary, level: i32, params: &LevelParams) -> Self {
        let prefix = dict.content();
        let prefix_len = prefix.len();

        let mut combined = Vec::with_capacity(prefix_len + MAX_BLOCK_SIZE);
        combined.extend_from_slice(prefix);

        let attached_hash_log = if prefix_len >= 2 {
            let src_log = strategy::src_log(prefix_len);
            (src_log + 1).clamp(
                strategy::HASH_LOG_MIN,
                params.hash_log.max(strategy::HASH_LOG_MIN),
            )
        } else {
            strategy::HASH_LOG_MIN
        };
        let mut attached: Vec<(u32, Vec<u32>)> = Vec::new();
        if params.strategy == Strategy::Fast {
            // Small inputs at negative levels search with a shorter match.
            for size in [1, ATTACH_THRESHOLD, MAX_BLOCK_SIZE] {
                let min_match = strategy::level_params_for_size(level, prefix_len + size)
                    .expect("level validated by the caller")
                    .min_match;
                if attached.iter().all(|(mls, _)| *mls != min_match) {
                    let mut table = vec![0u32; 1 << attached_hash_log];
                    fast::prefill_hash_table(
                        &combined,
                        prefix_len,
                        attached_hash_log,
                        min_match,
                        &mut table,
                    );
                    attached.push((min_match, table));
                }
            }
        }

        let (hash_snapshot, hash_long_snapshot) = match params.strategy {
            Strategy::Fast => {
                let hash_size = 1usize << params.hash_log;
                let mut hash_table = vec![0u32; hash_size];
                fast::prefill_hash_table(
                    &combined,
                    prefix_len,
                    params.hash_log,
                    params.min_match,
                    &mut hash_table,
                );
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

        let ll_table = dict
            .ll_table()
            .map(|(dt, al)| block_encoder::FseEncodeTable::from_decode_table(dt, al, 35));
        let of_table = dict
            .of_table()
            .map(|(dt, al)| block_encoder::FseEncodeTable::from_decode_table(dt, al, 31));
        let ml_table = dict
            .ml_table()
            .map(|(dt, al)| block_encoder::FseEncodeTable::from_decode_table(dt, al, 52));

        Self {
            combined,
            hash_snapshot,
            hash_long_snapshot,
            attached,
            attached_hash_log,
            prefix_len,
            rep_offsets: *dict.rep_offsets(),
            dict_id: dict.id(),
            huf_table,
            ll_table,
            of_table,
            ml_table,
        }
    }

    /// The attached dictionary table hashed for `min_match`.
    fn attached_table(&self, min_match: u32) -> Option<&[u32]> {
        self.attached
            .iter()
            .find(|(mls, _)| *mls == min_match)
            .map(|(_, table)| table.as_slice())
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
    level: i32,
    content_checksum: bool,
    /// The dictionary and expected input size to prepare it again on a
    /// level change.
    dict: Option<(Dictionary, usize)>,
    prepared: Option<PreparedDict>,
    hash_table: Vec<u32>,
    hash_long: Vec<u32>,
    dict_hash: Vec<u32>,
    small_hash: Vec<u32>,
    sequences: Vec<Sequence>,
    output: Vec<u8>,
    workspace: BlockEncodeWorkspace,
    combined: Vec<u8>,
}

impl CompressContext {
    /// Creates a new context for the given compression level (-7..=4; -8 runs as -7).
    pub fn new(level: i32) -> Result<Self, CompressError> {
        let params = strategy::level_params(level).ok_or(CompressError::InvalidLevel(level))?;
        let max_log = strategy::max_hash_log(level).expect("level validated above");
        let alloc_size = 1usize << max_log;
        let (hash_table, hash_long) = match params.strategy {
            Strategy::Fast => (vec![0u32; alloc_size], Vec::new()),
            Strategy::DFast => (vec![0u32; alloc_size], vec![0u32; alloc_size]),
        };
        Ok(Self {
            level,
            content_checksum: true,
            dict: None,
            prepared: None,
            hash_table,
            hash_long,
            dict_hash: Vec::new(),
            small_hash: Vec::new(),
            sequences: Vec::new(),
            output: Vec::new(),
            workspace: BlockEncodeWorkspace::new(),
            combined: Vec::new(),
        })
    }

    /// Creates a new context with a pre-loaded dictionary.
    ///
    /// The prepared hash table snapshot is built for the T0 (>256 KB)
    /// parameter tier. Inputs whose tiered params match T0's hash sizes
    /// use the fast snapshot-restore path; others fall back to per-call
    /// prefix hashing.
    ///
    /// Use [`with_dict_for_size`] to build the snapshot for a specific
    /// input size tier.
    pub fn with_dict(level: i32, dict: Dictionary) -> Result<Self, CompressError> {
        Self::with_dict_for_size(level, dict, usize::MAX)
    }

    /// Creates a new context with a pre-loaded dictionary, optimized for
    /// inputs of approximately `expected_size` bytes.
    ///
    /// The prepared hash table snapshot is built for the parameter tier
    /// matching `expected_size`. Inputs in the same tier use the fast
    /// snapshot-restore path.
    pub fn with_dict_for_size(
        level: i32,
        dict: Dictionary,
        expected_size: usize,
    ) -> Result<Self, CompressError> {
        let total_window = dict.content().len().saturating_add(expected_size);
        let params = strategy::level_params_for_size(level, total_window)
            .ok_or(CompressError::InvalidLevel(level))?;
        let prepared = PreparedDict::new(&dict, level, &params);
        let hash_table = vec![0u32; prepared.hash_snapshot.len()];
        let hash_long = vec![0u32; prepared.hash_long_snapshot.len()];
        Ok(Self {
            level,
            content_checksum: true,
            dict: Some((dict, expected_size)),
            prepared: Some(prepared),
            hash_table,
            hash_long,
            dict_hash: Vec::new(),
            small_hash: Vec::new(),
            sequences: Vec::new(),
            output: Vec::new(),
            workspace: BlockEncodeWorkspace::new(),
            combined: Vec::new(),
        })
    }

    /// The level of the next frame.
    pub const fn level(&self) -> i32 {
        self.level
    }

    /// Switches later frames to `level` (-7..=4; -8 runs as -7).
    ///
    /// Hash tables and buffers stay allocated and resize on use. A context
    /// with a dictionary prepares the dictionary again for the new level.
    pub fn set_level(&mut self, level: i32) -> Result<(), CompressError> {
        if level == self.level {
            return Ok(());
        }
        strategy::level_params(level).ok_or(CompressError::InvalidLevel(level))?;
        if let Some((dict, expected_size)) = &self.dict {
            let total_window = dict.content().len().saturating_add(*expected_size);
            let params = strategy::level_params_for_size(level, total_window)
                .expect("level validated above");
            self.prepared = Some(PreparedDict::new(dict, level, &params));
        }
        self.level = level;
        Ok(())
    }

    /// Enables or disables the standard Zstandard frame content checksum.
    ///
    /// Checksums are enabled by default. Disabling them is useful when an
    /// enclosing format already authenticates the decoded content.
    pub const fn set_content_checksum(&mut self, enabled: bool) {
        self.content_checksum = enabled;
    }

    /// Whether newly compressed frames include a content checksum.
    pub const fn content_checksum(&self) -> bool {
        self.content_checksum
    }

    /// Compresses `input` using the context's level and optional dictionary.
    pub fn compress(&mut self, input: &[u8]) -> Result<Cow<'_, [u8]>, CompressError> {
        let mut output = core::mem::take(&mut self.output);
        output.clear();
        let result = self.compress_to(input, &mut output);
        self.output = output;
        result?;
        Ok(self.take_or_borrow_output())
    }

    /// Compresses `input` into a caller-owned buffer while reusing this
    /// context's hash tables, workspace, and dictionary.
    ///
    /// Writes one frame to the start of `output` and returns its length.
    /// Unlike [`Self::compress`], the result can outlive the next use of this
    /// context without copying. `zrip::compress_bound(input.len())` bytes are
    /// always enough. Returns [`CompressError::OutputTooSmall`] if the frame
    /// does not fit; `output` then holds a partial frame.
    pub fn compress_into(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, CompressError> {
        let mut sink = SliceSink::new(output);
        self.compress_to(input, &mut sink)?;
        Ok(sink.pos())
    }

    /// Writes one frame for `input` to `output`.
    fn compress_to<S: OutputSink>(
        &mut self,
        input: &[u8],
        output: &mut S,
    ) -> Result<(), CompressError> {
        if self.prepared.is_some() {
            return self.compress_with_prepared(input, output);
        }
        let params = strategy::level_params_for_size(self.level, input.len())
            .expect("level validated at construction");
        compress_core(
            input,
            params,
            None,
            &[],
            [1u32, 4, 8],
            &mut self.hash_table,
            &mut self.hash_long,
            &mut self.dict_hash,
            &mut self.sequences,
            output,
            &mut self.workspace,
            &mut self.combined,
            self.content_checksum,
        )
    }

    /// Compresses `input` using an ad-hoc dictionary (overrides the stored one).
    pub fn compress_with_dict(
        &mut self,
        input: &[u8],
        dict: &Dictionary,
    ) -> Result<Cow<'_, [u8]>, CompressError> {
        let params =
            strategy::level_params_with_prefix(self.level, dict.content().len(), input.len())
                .expect("level validated at construction");
        self.workspace.prev_ll = dict
            .ll_table()
            .map(|(dt, al)| block_encoder::FseEncodeTable::from_decode_table(dt, al, 35));
        self.workspace.prev_of = dict
            .of_table()
            .map(|(dt, al)| block_encoder::FseEncodeTable::from_decode_table(dt, al, 31));
        self.workspace.prev_ml = dict
            .ml_table()
            .map(|(dt, al)| block_encoder::FseEncodeTable::from_decode_table(dt, al, 52));
        self.workspace.prev_huffman = dict
            .huf_table()
            .and_then(|(dt, tl)| HuffmanEncodeTable::from_decode_table(dt, tl));
        self.output.clear();
        compress_core(
            input,
            params,
            Some(dict.id()),
            dict.content(),
            *dict.rep_offsets(),
            &mut self.hash_table,
            &mut self.hash_long,
            &mut self.dict_hash,
            &mut self.sequences,
            &mut self.output,
            &mut self.workspace,
            &mut self.combined,
            self.content_checksum,
        )?;
        Ok(self.take_or_borrow_output())
    }

    fn compress_with_prepared<S: OutputSink>(
        &mut self,
        input: &[u8],
        output: &mut S,
    ) -> Result<(), CompressError> {
        let prep = self.prepared.as_ref().unwrap();
        let params = strategy::level_params_with_prefix(self.level, prep.prefix_len, input.len())
            .expect("level validated at construction");

        let dict_id = prep.dict_id;
        let prefix_len = prep.prefix_len;
        let snapshot_matches = match params.strategy {
            Strategy::Fast => (1usize << params.hash_log) == prep.hash_snapshot.len(),
            Strategy::DFast => {
                (1usize << params.chain_log) == prep.hash_snapshot.len()
                    && (1usize << params.hash_log) == prep.hash_long_snapshot.len()
            }
        };
        // A one-block input also attaches when the snapshot is sized for
        // another tier: that beats prefilling a table per call.
        let attached = (params.strategy == Strategy::Fast
            && !input.is_empty()
            && input.len() <= MAX_BLOCK_SIZE
            && (input.len() <= ATTACH_THRESHOLD || !snapshot_matches))
            .then(|| prep.attached_table(params.min_match))
            .flatten();
        let use_attached = attached.is_some();

        if !use_attached && !snapshot_matches {
            return self.compress_with_dict_fallback(input, dict_id, prefix_len, output);
        }

        {
            let prep = self.prepared.as_mut().unwrap();
            if !use_attached {
                // The per-call prefix path resizes these tables for its own
                // parameter tier, so restore length and contents together.
                self.hash_table.clear();
                self.hash_table.extend_from_slice(&prep.hash_snapshot);
                if !prep.hash_long_snapshot.is_empty() {
                    self.hash_long.clear();
                    self.hash_long.extend_from_slice(&prep.hash_long_snapshot);
                }
            }
            prep.combined.truncate(prep.prefix_len);
            prep.combined.extend_from_slice(input);
        }

        let prep = self.prepared.as_ref().unwrap();

        if use_attached {
            self.workspace.prev_huffman = if params.force_raw_literals {
                None
            } else {
                prep.huf_table.clone()
            };
        } else if let Some(ref huf) = prep.huf_table {
            self.workspace.prev_huffman = Some(huf.clone());
        } else {
            self.workspace.prev_huffman = None;
        }
        self.workspace.prev_ll = prep.ll_table.clone();
        self.workspace.prev_of = prep.of_table.clone();
        self.workspace.prev_ml = prep.ml_table.clone();

        output.reserve(input.len() + 32);
        write_frame_header_with_checksum(
            output,
            input.len(),
            Some(dict_id),
            params.window_log,
            self.content_checksum,
        )?;

        if input.is_empty() {
            block_encoder::encode_raw_block(&[], true, output)?;
        } else if use_attached {
            let input_hash_log = if input.len() >= 2 {
                let src_log = strategy::src_log(input.len());
                params.hash_log.min(src_log).max(strategy::HASH_LOG_MIN)
            } else {
                strategy::HASH_LOG_MIN
            };
            let input_hash_size = 1usize << input_hash_log;
            self.small_hash.resize(input_hash_size, 0);
            self.small_hash.fill(0);

            let mut rep_offsets = prep.rep_offsets;
            let dict = AttachedDict {
                table: prep.attached_table(params.min_match).unwrap(),
                hash_log: prep.attached_hash_log,
                prefix_len,
            };
            let input_params = LevelParams {
                hash_log: input_hash_log,
                ..params
            };

            fast::compress_fast_block_attached(
                &prep.combined,
                prefix_len + input.len(),
                &input_params,
                &prep.rep_offsets,
                dict,
                &mut self.small_hash,
                &mut self.sequences,
            );

            if params.force_raw_literals {
                block_encoder::encode_compressed_block_raw(
                    input,
                    &self.sequences,
                    &mut rep_offsets,
                    true,
                    output,
                    &mut self.workspace,
                )?;
            } else {
                block_encoder::encode_compressed_block(
                    input,
                    &self.sequences,
                    &mut rep_offsets,
                    true,
                    output,
                    &mut self.workspace,
                    strategy::block_policy(&params, input.len()),
                )?;
            }
        } else {
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
                        output,
                        &mut self.workspace,
                    )?;
                } else {
                    block_encoder::encode_compressed_block(
                        input,
                        &self.sequences,
                        &mut rep_offsets,
                        true,
                        output,
                        &mut self.workspace,
                        strategy::block_policy(&params, input.len()),
                    )?;
                }
            } else {
                let mut positions = PositionBase::new(params.window_log);
                let mut offset = 0;
                while offset < input.len() {
                    let chunk_size = (input.len() - offset).min(MAX_BLOCK_SIZE);
                    let is_last = offset + chunk_size >= input.len();
                    let (start, end) = (prefix_len + offset, prefix_len + offset + chunk_size);
                    if let Some(shift) = positions.advance(start, end) {
                        rebase::reduce_hash_table(&mut self.hash_table, shift);
                        rebase::reduce_hash_table(&mut self.hash_long, shift);
                    }
                    let base = positions.base();
                    let src = &combined[base..];
                    match params.strategy {
                        Strategy::Fast => {
                            fast::compress_fast_block(
                                src,
                                start - base,
                                end - base,
                                &params,
                                &rep_offsets,
                                &mut self.hash_table,
                                &mut self.sequences,
                            );
                        }
                        Strategy::DFast => {
                            dfast::compress_dfast_block(
                                src,
                                start - base,
                                end - base,
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
                            output,
                            &mut self.workspace,
                        )?;
                    } else {
                        block_encoder::encode_compressed_block(
                            &input[offset..offset + chunk_size],
                            &self.sequences,
                            &mut rep_offsets,
                            is_last,
                            output,
                            &mut self.workspace,
                            strategy::block_policy(&params, input.len()),
                        )?;
                    }
                    offset += chunk_size;
                }
            }
        }

        if self.content_checksum {
            let hash = xxh64(input, 0);
            let checksum = (hash & 0xFFFF_FFFF) as u32;
            output.extend_from_slice(&checksum.to_le_bytes())?;
        }

        Ok(())
    }

    fn compress_with_dict_fallback<S: OutputSink>(
        &mut self,
        input: &[u8],
        dict_id: u32,
        prefix_len: usize,
        output: &mut S,
    ) -> Result<(), CompressError> {
        let prep = self.prepared.as_ref().unwrap();
        let rep_offsets = prep.rep_offsets;
        let prefix = &prep.combined[..prefix_len];

        let params = strategy::level_params_with_prefix(self.level, prefix_len, input.len())
            .expect("level validated at construction");
        self.workspace.prev_huffman = prep.huf_table.clone();
        self.workspace.prev_ll = prep.ll_table.clone();
        self.workspace.prev_of = prep.of_table.clone();
        self.workspace.prev_ml = prep.ml_table.clone();
        compress_core(
            input,
            params,
            Some(dict_id),
            prefix,
            rep_offsets,
            &mut self.hash_table,
            &mut self.hash_long,
            &mut self.dict_hash,
            &mut self.sequences,
            output,
            &mut self.workspace,
            &mut self.combined,
            self.content_checksum,
        )
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
fn compress_core<S: OutputSink>(
    input: &[u8],
    params: LevelParams,
    dict_id: Option<u32>,
    prefix: &[u8],
    init_rep_offsets: [u32; 3],
    hash_table: &mut Vec<u32>,
    hash_long: &mut Vec<u32>,
    dict_hash: &mut Vec<u32>,
    sequences: &mut Vec<Sequence>,
    output: &mut S,
    workspace: &mut BlockEncodeWorkspace,
    combined: &mut Vec<u8>,
    content_checksum: bool,
) -> Result<(), CompressError> {
    let hash_size = match params.strategy {
        Strategy::Fast => 1usize << params.hash_log,
        Strategy::DFast => 1usize << params.chain_log,
    };
    let long_size = 1usize << params.hash_log;

    if prefix.is_empty() {
        workspace.prev_huffman = None;
        workspace.prev_ll = None;
        workspace.prev_of = None;
        workspace.prev_ml = None;
    }

    output.reserve(input.len() + 32);
    write_frame_header_with_checksum(
        output,
        input.len(),
        dict_id,
        params.window_log,
        content_checksum,
    )?;

    // Paths that walk the input block by block hash each block right after
    // compressing it, while its bytes are still in cache.
    let mut checksum = content_checksum.then(|| Xxh64State::new(0));
    let mut hashed_blocks = false;

    if input.is_empty() {
        block_encoder::encode_raw_block(&[], true, output)?;
    } else {
        let has_prefix = !prefix.is_empty();
        let mut rep_offsets = init_rep_offsets;
        let mut offset = 0;
        let mut positions = PositionBase::new(params.window_log);

        if hash_table.len() != hash_size {
            hash_table.resize(hash_size, 0);
        }

        match params.strategy {
            Strategy::Fast => {
                if has_prefix && input.len() <= MAX_BLOCK_SIZE {
                    if dict_hash.len() != hash_size {
                        dict_hash.resize(hash_size, 0);
                    }
                    fast::compress_fast_with_prefix_reuse(
                        input,
                        &params,
                        &rep_offsets,
                        prefix,
                        dict_hash,
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
                        )?;
                    } else {
                        block_encoder::encode_compressed_block(
                            input,
                            sequences,
                            &mut rep_offsets,
                            true,
                            output,
                            workspace,
                            strategy::block_policy(&params, input.len()),
                        )?;
                    }
                } else if has_prefix {
                    combined.clear();
                    combined.reserve(prefix.len() + input.len());
                    combined.extend_from_slice(prefix);
                    combined.extend_from_slice(input);
                    let plen = prefix.len();
                    fast::prefill_hash_table(
                        combined,
                        plen,
                        params.hash_log,
                        params.min_match,
                        hash_table,
                    );

                    while offset < input.len() {
                        let chunk_size = (input.len() - offset).min(MAX_BLOCK_SIZE);
                        let is_last = offset + chunk_size >= input.len();
                        let (start, end) = (plen + offset, plen + offset + chunk_size);
                        if let Some(shift) = positions.advance(start, end) {
                            rebase::reduce_hash_table(hash_table, shift);
                        }
                        let base = positions.base();
                        fast::compress_fast_block(
                            &combined[base..],
                            start - base,
                            end - base,
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
                            )?;
                        } else {
                            block_encoder::encode_compressed_block(
                                &input[offset..offset + chunk_size],
                                sequences,
                                &mut rep_offsets,
                                is_last,
                                output,
                                workspace,
                                strategy::block_policy(&params, input.len()),
                            )?;
                        }
                        offset += chunk_size;
                    }
                } else {
                    hash_table.fill(0);
                    while offset < input.len() {
                        let chunk_size = (input.len() - offset).min(MAX_BLOCK_SIZE);
                        let block_end = offset + chunk_size;
                        let is_last = block_end >= input.len();
                        let block = &input[offset..block_end];

                        if let Some(shift) = positions.advance(offset, block_end) {
                            rebase::reduce_hash_table(hash_table, shift);
                        }
                        if skip_match_search(&params, input.len(), block) {
                            block_encoder::encode_raw_block(block, is_last, output)?;
                        } else {
                            let base = positions.base();
                            fast::compress_fast_block(
                                &input[base..],
                                offset - base,
                                block_end - base,
                                &params,
                                &rep_offsets,
                                hash_table,
                                sequences,
                            );
                            if params.force_raw_literals {
                                block_encoder::encode_compressed_block_raw(
                                    block,
                                    sequences,
                                    &mut rep_offsets,
                                    is_last,
                                    output,
                                    workspace,
                                )?;
                            } else {
                                block_encoder::encode_compressed_block(
                                    block,
                                    sequences,
                                    &mut rep_offsets,
                                    is_last,
                                    output,
                                    workspace,
                                    strategy::block_policy(&params, input.len()),
                                )?;
                            }
                        }
                        if let Some(c) = checksum.as_mut() {
                            c.update(block);
                        }
                        offset = block_end;
                    }
                    hashed_blocks = true;
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
                        strategy::block_policy(&params, input.len()),
                    )?;
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
                        let (start, end) = (plen + offset, plen + offset + chunk_size);
                        if let Some(shift) = positions.advance(start, end) {
                            rebase::reduce_hash_table(hash_table, shift);
                            rebase::reduce_hash_table(hash_long, shift);
                        }
                        let base = positions.base();
                        dfast::compress_dfast_block(
                            &combined[base..],
                            start - base,
                            end - base,
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
                            strategy::block_policy(&params, input.len()),
                        )?;
                        offset += chunk_size;
                    }
                } else {
                    hash_table.fill(0);
                    hash_long.fill(0);
                    while offset < input.len() {
                        let chunk_size = (input.len() - offset).min(MAX_BLOCK_SIZE);
                        let block_end = offset + chunk_size;
                        let is_last = block_end >= input.len();
                        let block = &input[offset..block_end];

                        if let Some(shift) = positions.advance(offset, block_end) {
                            rebase::reduce_hash_table(hash_table, shift);
                            rebase::reduce_hash_table(hash_long, shift);
                        }
                        if skip_match_search(&params, input.len(), block) {
                            block_encoder::encode_raw_block(block, is_last, output)?;
                        } else {
                            let base = positions.base();
                            dfast::compress_dfast_block(
                                &input[base..],
                                offset - base,
                                block_end - base,
                                &params,
                                &rep_offsets,
                                hash_table,
                                hash_long,
                                sequences,
                            );
                            block_encoder::encode_compressed_block(
                                block,
                                sequences,
                                &mut rep_offsets,
                                is_last,
                                output,
                                workspace,
                                strategy::block_policy(&params, input.len()),
                            )?;
                        }
                        if let Some(c) = checksum.as_mut() {
                            c.update(block);
                        }
                        offset = block_end;
                    }
                    hashed_blocks = true;
                }
            }
        }
    }

    if let Some(c) = checksum {
        let hash = if hashed_blocks {
            c.finish()
        } else {
            xxh64(input, 0)
        };
        output.extend_from_slice(&((hash & 0xFFFF_FFFF) as u32).to_le_bytes())?;
    }

    Ok(())
}
