//! [`CompressContext`], which reuses tables and buffers across frames, and
//! the dictionary state it prepares once.
#[cfg(feature = "alloc")]
use alloc::borrow::Cow;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::block_encoder::{self, BlockEncodeWorkspace};
use crate::fast::AttachedDict;
use crate::output::{OutputSink, SliceSink};
use crate::rebase::{self, PositionBase};
use crate::strategy::{self, LevelParams, Strategy};
use crate::table::MatchTable;
use crate::{dfast, fast, skip_match_search, write_frame_header_with_checksum};
use zrip_core::Sequence;
use zrip_core::dict::Dictionary;
use zrip_core::error::CompressError;
use zrip_core::frame::MAX_BLOCK_SIZE;
use zrip_core::huffman::encode::HuffmanEncodeTable;
use zrip_core::xxhash::{Xxh64State, xxh64};

/// Largest input that keeps the dictionary in its own tables instead of
/// copying the prefilled ones, as C zstd does for its fast and dfast
/// strategies. Past it, copying the tables costs less than the dictionary
/// probes.
const ATTACH_THRESHOLD: usize = 8192;

/// Largest DFast input that attaches the dictionary when its prefilled
/// tables are sized for other inputs.
const DFAST_ATTACH_LIMIT: usize = 16 * 1024;

/// Dictionary tables for attached inputs at one minimum match, sized to the
/// dictionary content: the Fast table, or the DFast short and long tables.
/// Empty until built.
struct Attached {
    min_match: u32,
    table: Vec<u32>,
    long: Vec<u32>,
}

/// The log of an attached table for a `prefix_len`-byte dictionary, at most
/// `cap`.
fn attached_log(prefix_len: usize, cap: u32) -> u32 {
    if prefix_len >= 2 {
        (strategy::src_log(prefix_len) + 1)
            .clamp(strategy::HASH_LOG_MIN, cap.max(strategy::HASH_LOG_MIN))
    } else {
        strategy::HASH_LOG_MIN
    }
}

/// Pre-computed dictionary state for hot-loop compression.
///
/// Built once from a [`Dictionary`] + [`LevelParams`]. Holds a combined
/// buffer with the dict prefix already loaded and encode-side entropy tables
/// built from the dict's decode tables. The prefilled hash tables are built
/// by the first input that uses them, so a context that compresses only
/// inputs of other sizes never builds them.
pub(crate) struct PreparedDict {
    /// The dictionary content, then the current input.
    combined: Vec<u8>,
    /// The parameters the snapshots are prefilled for.
    params: LevelParams,
    /// Empty until built.
    hash_snapshot: Vec<u32>,
    hash_long_snapshot: Vec<u32>,
    /// Dictionary tables for attached inputs, one set per minimum match the
    /// level uses.
    attached: Vec<Attached>,
    /// Logs of the attached Fast or DFast short tables, and of the DFast
    /// long tables.
    attached_hash_log: u32,
    attached_long_log: u32,
    prefix_len: usize,
    rep_offsets: [u32; 3],
    dict_id: u32,
    huf_table: Option<HuffmanEncodeTable>,
    ll_table: Option<block_encoder::FseEncodeTable>,
    of_table: Option<block_encoder::FseEncodeTable>,
    ml_table: Option<block_encoder::FseEncodeTable>,
}

impl PreparedDict {
    /// Prepares `dict` for `level`, with snapshots for `params`. Builds the
    /// entropy tables now and the hash tables on first use.
    pub fn new(dict: &Dictionary, level: i32, params: &LevelParams) -> Self {
        let prefix = dict.content();
        let prefix_len = prefix.len();

        // Inputs that take the per-call prefix path never use it, so it grows
        // to the input on first use.
        let mut combined = Vec::with_capacity(prefix_len);
        combined.extend_from_slice(prefix);

        let (attached_hash_log, attached_long_log) = match params.strategy {
            Strategy::Fast => (attached_log(prefix_len, params.hash_log), 0),
            Strategy::DFast => (
                attached_log(prefix_len, params.chain_log),
                attached_log(prefix_len, params.hash_log),
            ),
        };
        let mut attached: Vec<Attached> = Vec::new();
        // Small inputs at negative levels search with a shorter match.
        for size in [1, ATTACH_THRESHOLD, MAX_BLOCK_SIZE] {
            let min_match = strategy::level_params_for_size(level, prefix_len + size)
                .expect("level validated by the caller")
                .min_match;
            if attached.iter().all(|a| a.min_match != min_match) {
                attached.push(Attached {
                    min_match,
                    table: Vec::new(),
                    long: Vec::new(),
                });
            }
        }

        let huf_table = dict
            .huf_table()
            .and_then(|(dt, tl)| HuffmanEncodeTable::from_decode_table(dt, tl));

        // Largest codes: 35 for literal lengths, 31 for offsets, 52 for
        // match lengths.
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
            params: *params,
            hash_snapshot: Vec::new(),
            hash_long_snapshot: Vec::new(),
            attached,
            attached_hash_log,
            attached_long_log,
            prefix_len,
            rep_offsets: *dict.rep_offsets(),
            dict_id: dict.id(),
            huf_table,
            ll_table,
            of_table,
            ml_table,
        }
    }

    /// Whether `params` use the table sizes the snapshots are built for.
    fn snapshot_matches(&self, params: &LevelParams) -> bool {
        let p = &self.params;
        match params.strategy {
            Strategy::Fast => params.hash_log == p.hash_log,
            Strategy::DFast => params.chain_log == p.chain_log && params.hash_log == p.hash_log,
        }
    }

    /// The tables prefilled with the dictionary, built on first use.
    fn snapshots(&mut self) -> (&[u32], &[u32]) {
        if self.hash_snapshot.is_empty() {
            let p = &self.params;
            // The buffer may still hold the last input after the prefix, and
            // the Fast prefill hashes up to the end of what it is given.
            let prefix = &self.combined[..self.prefix_len];
            let mut hash_table = MatchTable::new();
            match p.strategy {
                Strategy::Fast => {
                    fast::prefill_hash_table(
                        prefix,
                        self.prefix_len,
                        p.hash_log,
                        p.min_match,
                        &mut hash_table,
                    );
                }
                Strategy::DFast => {
                    let mut hash_long = MatchTable::new();
                    dfast::prefill_hash_tables(
                        prefix,
                        self.prefix_len,
                        p.hash_log,
                        p.chain_log,
                        p.min_match,
                        &mut hash_table,
                        &mut hash_long,
                    );
                    self.hash_long_snapshot = hash_long.into_vec();
                }
            }
            self.hash_snapshot = hash_table.into_vec();
        }
        (&self.hash_snapshot, &self.hash_long_snapshot)
    }

    /// Whether the level uses attached tables hashed for `min_match`.
    fn has_attached(&self, min_match: u32) -> bool {
        self.attached.iter().any(|a| a.min_match == min_match)
    }

    /// Builds the attached tables hashed for `min_match` if they are missing.
    fn build_attached(&mut self, min_match: u32) {
        // Only the prefix, as in `snapshots`.
        let prefix = &self.combined[..self.prefix_len];
        let (hash_log, long_log) = (self.attached_hash_log, self.attached_long_log);
        let strategy = self.params.strategy;
        if let Some(a) = self
            .attached
            .iter_mut()
            .find(|a| a.min_match == min_match && a.table.is_empty())
        {
            let mut table = MatchTable::new();
            match strategy {
                Strategy::Fast => {
                    fast::prefill_hash_table(prefix, prefix.len(), hash_log, min_match, &mut table);
                }
                Strategy::DFast => {
                    let mut long = MatchTable::new();
                    dfast::prefill_hash_tables(
                        prefix,
                        prefix.len(),
                        long_log,
                        hash_log,
                        min_match,
                        &mut table,
                        &mut long,
                    );
                    a.long = long.into_vec();
                }
            }
            a.table = table.into_vec();
        }
    }

    /// The attached dictionary tables hashed for `min_match`, once built.
    fn attached(&self, min_match: u32) -> Option<&Attached> {
        self.attached.iter().find(|a| a.min_match == min_match)
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
    hash_table: MatchTable,
    hash_long: MatchTable,
    dict_hash: MatchTable,
    /// The input's own tables beside attached dictionary tables: the Fast
    /// or DFast short table, and the DFast long table.
    small_hash: MatchTable,
    small_long: MatchTable,
    sequences: Vec<Sequence>,
    output: Vec<u8>,
    workspace: BlockEncodeWorkspace,
    combined: Vec<u8>,
}

impl CompressContext {
    /// Creates a new context for the given compression level (-7..=4; -8 runs as -7).
    ///
    /// It allocates nothing up front: the first frame sizes the hash tables
    /// and buffers for its input.
    pub fn new(level: i32) -> Result<Self, CompressError> {
        strategy::level_params(level).ok_or(CompressError::InvalidLevel(level))?;
        Ok(Self {
            level,
            content_checksum: true,
            dict: None,
            prepared: None,
            hash_table: MatchTable::new(),
            hash_long: MatchTable::new(),
            dict_hash: MatchTable::new(),
            small_hash: MatchTable::new(),
            small_long: MatchTable::new(),
            sequences: Vec::new(),
            output: Vec::new(),
            workspace: BlockEncodeWorkspace::new(),
            combined: Vec::new(),
        })
    }

    /// Creates a new context with a pre-loaded dictionary.
    ///
    /// The prepared hash table snapshot is for the large-input parameter
    /// tier, built by the first input that uses it. Inputs whose tiered
    /// params match its hash sizes use the fast snapshot-restore path.
    /// Inputs up to 8 KiB, and some other one-block inputs, search the
    /// dictionary's own tables instead; the rest fall back to per-call
    /// prefix hashing.
    ///
    /// Use [`with_dict_for_size`](Self::with_dict_for_size) to build the
    /// snapshot for a specific input size tier.
    pub fn with_dict(level: i32, dict: Dictionary) -> Result<Self, CompressError> {
        Self::with_dict_for_size(level, dict, usize::MAX)
    }

    /// Creates a new context with a pre-loaded dictionary, optimized for
    /// inputs of approximately `expected_size` bytes.
    ///
    /// The prepared hash table snapshot is for the parameter tier matching
    /// `expected_size`, built by the first input that uses it. Inputs in
    /// the same tier use the fast snapshot-restore path.
    pub fn with_dict_for_size(
        level: i32,
        dict: Dictionary,
        expected_size: usize,
    ) -> Result<Self, CompressError> {
        let total_window = dict.content().len().saturating_add(expected_size);
        let params = strategy::level_params_for_size(level, total_window)
            .ok_or(CompressError::InvalidLevel(level))?;
        let prepared = PreparedDict::new(&dict, level, &params);
        Ok(Self {
            level,
            content_checksum: true,
            dict: Some((dict, expected_size)),
            prepared: Some(prepared),
            hash_table: MatchTable::new(),
            hash_long: MatchTable::new(),
            dict_hash: MatchTable::new(),
            small_hash: MatchTable::new(),
            small_long: MatchTable::new(),
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
    ///
    /// The frame borrows the context's output buffer. A frame of 512 KiB or
    /// more takes the buffer instead, and the next call allocates a new one.
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
        self.workspace.set_prev_huffman(
            dict.huf_table()
                .and_then(|(dt, tl)| HuffmanEncodeTable::from_decode_table(dt, tl)),
        );
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

    /// Writes one frame with the prepared dictionary: attached dictionary
    /// tables, a restored snapshot, or else per-call prefix hashing.
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
        let snapshot_matches = prep.snapshot_matches(&params);
        // An input the snapshot is not sized for also attaches, which beats
        // prefilling tables per call: any one-block input for Fast, and for
        // DFast, which probes two dictionary tables per position, up to
        // C zstd's cutoff for its dfast strategy.
        let other_tier = !snapshot_matches
            && match params.strategy {
                Strategy::Fast => true,
                Strategy::DFast => input.len() <= DFAST_ATTACH_LIMIT,
            };
        let use_attached = !input.is_empty()
            && input.len() <= MAX_BLOCK_SIZE
            && (input.len() <= ATTACH_THRESHOLD || other_tier)
            && prep.has_attached(params.min_match);

        if !use_attached && !snapshot_matches {
            return self.compress_with_dict_fallback(input, dict_id, prefix_len, output);
        }

        {
            let prep = self.prepared.as_mut().unwrap();
            if use_attached {
                prep.build_attached(params.min_match);
            } else {
                // The per-call prefix path resizes these tables for its own
                // parameter tier, so restore length and contents together.
                let (short, long) = prep.snapshots();
                self.hash_table.copy_from(short);
                if !long.is_empty() {
                    self.hash_long.copy_from(long);
                }
            }
            prep.combined.truncate(prep.prefix_len);
            prep.combined.extend_from_slice(input);
        }

        let prep = self.prepared.as_ref().unwrap();

        if use_attached {
            self.workspace
                .set_prev_huffman(if params.force_raw_literals {
                    None
                } else {
                    prep.huf_table.clone()
                });
        } else if let Some(ref huf) = prep.huf_table {
            self.workspace.set_prev_huffman(Some(huf.clone()));
        } else {
            self.workspace.set_prev_huffman(None);
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
            // The input's own tables, sized to it.
            let input_log = |cap: u32| {
                if input.len() >= 2 {
                    cap.min(strategy::src_log(input.len()))
                        .max(strategy::HASH_LOG_MIN)
                } else {
                    strategy::HASH_LOG_MIN
                }
            };
            let input_params = LevelParams {
                hash_log: input_log(params.hash_log),
                chain_log: input_log(params.chain_log),
                ..params
            };
            let mut rep_offsets = prep.rep_offsets;
            let tables = prep.attached(params.min_match).unwrap();
            let block_end = prefix_len + input.len();
            match params.strategy {
                Strategy::Fast => {
                    self.small_hash.clear(1usize << input_params.hash_log);
                    let dict = AttachedDict {
                        table: &tables.table,
                        hash_log: prep.attached_hash_log,
                        prefix_len,
                    };
                    fast::compress_fast_block_attached(
                        &prep.combined,
                        block_end,
                        &input_params,
                        &prep.rep_offsets,
                        dict,
                        &mut self.small_hash,
                        &mut self.sequences,
                    );
                }
                Strategy::DFast => {
                    self.small_hash.clear(1usize << input_params.chain_log);
                    self.small_long.clear(1usize << input_params.hash_log);
                    let dict = dfast::AttachedDfastDict {
                        short: &tables.table,
                        long: &tables.long,
                        short_log: prep.attached_hash_log,
                        long_log: prep.attached_long_log,
                        prefix_len,
                    };
                    dfast::compress_dfast_block_attached(
                        &prep.combined,
                        block_end,
                        &input_params,
                        &prep.rep_offsets,
                        dict,
                        &mut self.small_hash,
                        &mut self.small_long,
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
        self.workspace.set_prev_huffman(prep.huf_table.clone());
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

/// Writes one frame for `input` after an optional dictionary `prefix`, in
/// the caller's tables and buffers. The caller sets the workspace's
/// dictionary entropy tables; without a prefix they are cleared here.
#[allow(clippy::too_many_arguments, clippy::unnecessary_wraps)]
pub(crate) fn compress_core<S: OutputSink>(
    input: &[u8],
    params: LevelParams,
    dict_id: Option<u32>,
    prefix: &[u8],
    init_rep_offsets: [u32; 3],
    hash_table: &mut MatchTable,
    hash_long: &mut MatchTable,
    dict_hash: &mut MatchTable,
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
        workspace.set_prev_huffman(None);
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

        match params.strategy {
            Strategy::Fast => {
                if has_prefix && input.len() <= MAX_BLOCK_SIZE {
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
                    hash_table.clear(hash_size);
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
                    hash_table.clear(hash_size);
                    hash_long.clear(long_size);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dict() -> Dictionary {
        let fixture = include_bytes!("../../../tests/fixtures/corpus_dict_roundtrip.bin");
        let len = u32::from_le_bytes([fixture[0], fixture[1], fixture[2], fixture[3]]) as usize;
        Dictionary::from_bytes(&fixture[4..4 + len]).unwrap()
    }

    /// Blocks reuse a dictionary's sequence tables only when they hold every
    /// code the block needs. Encoding a code a table lacks writes garbage.
    #[test]
    fn dictionary_tables_lacking_a_code_are_not_reused() {
        use zrip_core::fse::table_builder::serialize_fse_table_description;
        use zrip_core::fse::{LL_DEFAULT_ACCURACY, LL_DEFAULT_DIST};
        use zrip_core::fse::{ML_DEFAULT_ACCURACY, ML_DEFAULT_DIST};
        // Offset codes 0 to 10 only, as for offsets up to 2 KiB.
        const OF_DIST: [i16; 11] = [1, 1, 1, 1, 3, 3, 3, 3, 6, 6, 4];
        let mut raw = Vec::new();
        raw.extend_from_slice(&zrip_core::dict::DICT_MAGIC.to_le_bytes());
        raw.extend_from_slice(&7u32.to_le_bytes());
        raw.extend_from_slice(&[128 + 1, 0x11]);
        raw.extend(serialize_fse_table_description(&OF_DIST, 5));
        raw.extend(serialize_fse_table_description(
            &ML_DEFAULT_DIST,
            ML_DEFAULT_ACCURACY,
        ));
        raw.extend(serialize_fse_table_description(
            &LL_DEFAULT_DIST,
            LL_DEFAULT_ACCURACY,
        ));
        for rep in [1u32, 4, 8] {
            raw.extend_from_slice(&rep.to_le_bytes());
        }
        raw.extend((0..1024u32).map(|i| (i * 7 % 251) as u8));
        let dict = Dictionary::from_bytes(&raw).unwrap();
        assert!(dict.of_table().is_some());

        // Noise, then a copy from 5000 bytes back: offset code 12.
        let mut x = 0x2545_f491u32;
        let mut input: Vec<u8> = (0..5000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        input.extend_from_within(..2048);
        for level in [1, 3] {
            let frames = [
                CompressContext::with_dict(level, dict.clone())
                    .unwrap()
                    .compress(&input)
                    .unwrap()
                    .into_owned(),
                crate::compress_with_dict(&input, level, &dict).unwrap(),
            ];
            for frame in &frames {
                let out = zrip::decompress_with_dict(frame, &dict).unwrap();
                assert!(out == input, "L{level}: frame decodes to another input");
            }
        }
    }

    /// Tables built while a frame's input follows the dictionary in the
    /// buffer match tables built from the dictionary alone.
    #[test]
    fn dictionary_tables_ignore_buffered_input() {
        let dict = dict();
        let input: Vec<u8> = (0..4096u32)
            .map(|i| (i.wrapping_mul(0x9e37_79b1) >> 24) as u8)
            .collect();
        for level in -7..=4 {
            let params = strategy::level_params(level).unwrap();
            let mut clean = PreparedDict::new(&dict, level, &params);
            let mut stale = PreparedDict::new(&dict, level, &params);
            stale.combined.extend_from_slice(&input);
            assert!(clean.snapshots() == stale.snapshots(), "L{level} snapshots");
            let mlss: Vec<u32> = clean.attached.iter().map(|a| a.min_match).collect();
            for mls in mlss {
                clean.build_attached(mls);
                stale.build_attached(mls);
                let (c, s) = (clean.attached(mls).unwrap(), stale.attached(mls).unwrap());
                assert!(
                    c.table == s.table && c.long == s.long,
                    "L{level} attached tables for min match {mls}"
                );
            }
        }
    }
}
