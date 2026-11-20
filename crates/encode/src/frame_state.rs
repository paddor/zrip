//! Sans-io frame state shared by [`FrameEncoder`](crate::streaming::FrameEncoder)
//! and [`PushEncoder`](crate::push::PushEncoder).
//!
//! Buffers input up to one block, encodes blocks with the window and match
//! tables carried across them, and appends frame bytes to a caller's `Vec`.
#![forbid(unsafe_code)]

use alloc::vec::Vec;

use crate::block_encoder::{self, BlockEncodeWorkspace};
use crate::context;
use crate::dfast;
use crate::fast;
#[cfg(feature = "ldm")]
use crate::ldm::LdmState;
use crate::rebase::reduce_hash_table;
use crate::strategy::{self, LevelParams, Strategy};
use crate::table::MatchTable;
use crate::write_frame_header_without_content_size;
use zrip_core::Sequence;
use zrip_core::dict::Dictionary;
use zrip_core::error::CompressError;
use zrip_core::frame::MAX_BLOCK_SIZE;
use zrip_core::huffman::encode::HuffmanEncodeTable;
use zrip_core::xxhash::Xxh64State;

/// Smallest growth step for the window once a frame spans blocks.
const GROW_STEP: usize = 8 << 20;

/// How many table entries a whole clear must zero per slot store before
/// rehashing a window to zero just the slots its positions went to pays.
/// Measured break-even: 16-23 entries per store.
const SLOT_CLEAR_RATIO: usize = 24;

/// Whether rehashing a `window`-byte window into `tables` tables to zero the
/// slots it filled beats clearing their `entries` entries whole.
pub(crate) fn slot_clear_pays(window: usize, tables: usize, entries: usize) -> bool {
    window.saturating_mul(tables * SLOT_CLEAR_RATIO) <= entries
}

/// Encoder state for one frame at a time, reused across frames.
pub(crate) struct EncodeState {
    params: LevelParams,
    rep_offsets: [u32; 3],
    hasher: Xxh64State,
    workspace: BlockEncodeWorkspace,
    dict: Option<Dictionary>,
    /// The next block is the first of a frame with a dictionary, and seeds
    /// the tables with it.
    first_block: bool,
    hash_table: MatchTable,
    hash_long: MatchTable,
    sequences: Vec<Sequence>,
    /// The window: dictionary, then input, then the next block's input from
    /// `block_start`. Input goes straight here, so blocks need no copy.
    window_buf: Vec<u8>,
    block_start: usize,
    /// The tables are clean or hold this frame's positions. Set by the
    /// frame's first block that searches for matches.
    tables_ready: bool,
    /// A copy of the last window that wrote the tables, while they still
    /// hold its positions and it was small: the next frame zeroes just the
    /// slots those positions went to instead of the whole tables.
    stale_window: Vec<u8>,
    /// Buffers of frames encoded in one pass, kept for the next.
    dict_hash: MatchTable,
    combined: Vec<u8>,
    #[cfg(feature = "ldm")]
    ldm_state: Option<LdmState>,
}

impl EncodeState {
    pub(crate) fn new(params: LevelParams, dict: Option<Dictionary>) -> Self {
        let (rep_offsets, first_block) = match &dict {
            Some(d) => (*d.rep_offsets(), true),
            None => ([1, 4, 8], false),
        };
        let mut window_buf = Vec::new();
        if let Some(ref d) = dict {
            window_buf.extend_from_slice(d.content());
        }
        let block_start = window_buf.len();
        #[cfg(feature = "ldm")]
        let ldm_state = params.ldm_params.as_ref().map(LdmState::new);
        Self {
            params,
            rep_offsets,
            hasher: Xxh64State::new(0),
            workspace: BlockEncodeWorkspace::new(),
            dict,
            first_block,
            // Allocated by the first block, so frames that never need them
            // (see `PushEncoder`) skip zeroing them.
            hash_table: MatchTable::new(),
            hash_long: MatchTable::new(),
            sequences: Vec::new(),
            window_buf,
            block_start,
            tables_ready: false,
            stale_window: Vec::new(),
            dict_hash: MatchTable::new(),
            combined: Vec::new(),
            #[cfg(feature = "ldm")]
            ldm_state,
        }
    }

    /// Encodes `input`, or the buffered input if `None`, as one whole frame
    /// with its size known, as the one-shot API does, but in this state's
    /// tables and buffers. Call [`Self::reset`] after it.
    pub(crate) fn encode_one_pass(
        &mut self,
        input: Option<&[u8]>,
        params: &LevelParams,
        out: &mut Vec<u8>,
    ) -> Result<(), CompressError> {
        let src = match input {
            Some(input) => input,
            None => &self.window_buf[self.block_start..],
        };
        #[cfg(feature = "ldm")]
        if params.ldm_params.is_some() {
            return crate::compress_frame(src, params, out);
        }
        // Whole clears follow: the tables no longer hold just the last
        // window's positions.
        self.stale_window.clear();
        let (dict_id, prefix, rep_offsets) = match &self.dict {
            Some(d) => {
                self.workspace.prev_ll = d
                    .ll_table()
                    .map(|(dt, al)| block_encoder::FseEncodeTable::from_decode_table(dt, al, 35));
                self.workspace.prev_of = d
                    .of_table()
                    .map(|(dt, al)| block_encoder::FseEncodeTable::from_decode_table(dt, al, 31));
                self.workspace.prev_ml = d
                    .ml_table()
                    .map(|(dt, al)| block_encoder::FseEncodeTable::from_decode_table(dt, al, 52));
                self.workspace.set_prev_huffman(
                    d.huf_table()
                        .and_then(|(dt, tl)| HuffmanEncodeTable::from_decode_table(dt, tl)),
                );
                (Some(d.id()), d.content(), *d.rep_offsets())
            }
            None => (None, &[][..], [1, 4, 8]),
        };
        context::compress_core(
            src,
            *params,
            dict_id,
            prefix,
            rep_offsets,
            &mut self.hash_table,
            &mut self.hash_long,
            &mut self.dict_hash,
            &mut self.sequences,
            out,
            &mut self.workspace,
            &mut self.combined,
            true,
        )
    }

    /// Starts a new frame, keeping allocations. Drops buffered input.
    pub(crate) fn reset(&mut self) {
        // The tables keep this frame's positions until the next frame uses
        // them. A small window is kept to zero just their slots then.
        if self.tables_ready {
            self.stale_window.clear();
            if self.slot_clear_pays() {
                self.stale_window.extend_from_slice(&self.window_buf);
            }
            self.tables_ready = false;
        }
        self.first_block = self.dict.is_some();
        self.rep_offsets = match &self.dict {
            Some(d) => *d.rep_offsets(),
            None => [1, 4, 8],
        };
        self.hasher = Xxh64State::new(0);
        self.workspace.set_prev_huffman(None);
        self.window_buf.clear();
        if let Some(ref d) = self.dict {
            self.window_buf.extend_from_slice(d.content());
        }
        self.block_start = self.window_buf.len();
        #[cfg(feature = "ldm")]
        if let Some(ref mut ldm) = self.ldm_state {
            ldm.reset();
        }
    }

    /// Whether the window's positions are few enough next to the table
    /// entries to zero their slots instead of the whole tables.
    fn slot_clear_pays(&self) -> bool {
        #[cfg(feature = "ldm")]
        if self.ldm_state.is_some() {
            return false;
        }
        let tables = if self.hash_long.is_empty() { 1 } else { 2 };
        slot_clear_pays(
            self.window_buf.len(),
            tables,
            self.hash_table.len() + self.hash_long.len(),
        )
    }

    /// Readies the tables for the frame's first match search: zero, at the
    /// sizes the parameters use. Zeroes the slots of the stale window's
    /// positions if there is one, else whole tables that are dirty.
    fn ready_tables(&mut self) {
        let p = &self.params;
        if !self.stale_window.is_empty() {
            let window = &self.stale_window;
            match p.strategy {
                Strategy::Fast => self
                    .hash_table
                    .clear_slots(|t| fast::zero_position_slots(window, p.hash_log, p.min_match, t)),
                Strategy::DFast => {
                    self.hash_table.clear_slots(|t| {
                        dfast::zero_short_slots(window, p.chain_log, p.min_match, t)
                    });
                    self.hash_long
                        .clear_slots(|t| dfast::zero_long_slots(window, p.hash_log, t));
                }
            }
            self.stale_window.clear();
        }
        match p.strategy {
            Strategy::Fast => self.hash_table.clear(1usize << p.hash_log),
            Strategy::DFast => {
                self.hash_table.clear(1usize << p.chain_log);
                self.hash_long.clear(1usize << p.hash_log);
            }
        }
        self.tables_ready = true;
    }

    /// Largest block this frame uses: 128 KiB, or the window if smaller.
    pub(crate) fn block_size(&self) -> usize {
        MAX_BLOCK_SIZE.min(1usize << self.params.window_log)
    }

    /// Bytes waiting for the next block.
    pub(crate) fn buffered(&self) -> usize {
        self.window_buf.len() - self.block_start
    }

    pub(crate) fn dict(&self) -> Option<&Dictionary> {
        self.dict.as_ref()
    }

    /// Takes as much of `input` as fits in the current block and returns how
    /// much it took.
    pub(crate) fn take_input(&mut self, input: &[u8]) -> usize {
        let n = (self.block_size() - self.buffered()).min(input.len());
        let needed = self.window_buf.len() + n;
        if needed > self.window_buf.capacity() {
            // Double up to one block, so short frames stay small. Past that,
            // jump toward the most the window holds (two windows and a block)
            // in big steps: each doubling step copies the window and faults
            // in fresh pages.
            let window = 1usize << self.params.window_log;
            let dict_len = self.dict.as_ref().map_or(0, |d| d.content().len());
            let most = (2 * window).max(dict_len + window) + self.block_size();
            let first = dict_len + self.block_size();
            let target = if needed <= first {
                (2 * self.window_buf.capacity()).min(first)
            } else {
                (2 * self.window_buf.capacity()).max(GROW_STEP).min(most)
            }
            .max(needed);
            self.window_buf
                .reserve_exact(target - self.window_buf.len());
        }
        self.window_buf.extend_from_slice(&input[..n]);
        self.hasher.update(&input[..n]);
        n
    }

    /// Appends a frame header without a content size.
    pub(crate) fn write_header(&self, out: &mut Vec<u8>) -> Result<(), CompressError> {
        write_frame_header_without_content_size(
            out,
            self.dict.as_ref().map(Dictionary::id),
            self.params.window_log,
        )
    }

    /// Appends the content checksum that ends the frame.
    pub(crate) fn write_checksum(&self, out: &mut Vec<u8>) {
        let checksum = (self.hasher.finish() & 0xFFFF_FFFF) as u32;
        out.extend_from_slice(&checksum.to_le_bytes());
    }

    /// Encodes the buffered input as one block and appends it to `out`.
    ///
    /// With nothing buffered, writes an empty last block if `last`, and
    /// nothing otherwise.
    pub(crate) fn encode_block(
        &mut self,
        last: bool,
        out: &mut Vec<u8>,
    ) -> Result<(), CompressError> {
        if self.buffered() == 0 {
            if last {
                block_encoder::encode_raw_block(&[], true, out)?;
            }
            return Ok(());
        }

        let seed_dict = self.first_block && self.dict.is_some();
        let plen = self.block_start;
        out.reserve(self.buffered() + 32);
        let raw = {
            let chunk = &self.window_buf[plen..];
            crate::skip_match_search(&self.params, chunk.len(), chunk)
        };
        if !raw && !self.tables_ready {
            self.ready_tables();
        }
        let chunk = &self.window_buf[plen..];

        if raw {
            block_encoder::encode_raw_block(chunk, last, out)?;
        } else {
            if seed_dict {
                match self.params.strategy {
                    Strategy::Fast => {
                        fast::prefill_hash_table(
                            &self.window_buf,
                            plen,
                            self.params.hash_log,
                            self.params.min_match,
                            &mut self.hash_table,
                        );
                    }
                    Strategy::DFast => {
                        dfast::prefill_hash_tables(
                            &self.window_buf,
                            plen,
                            self.params.hash_log,
                            self.params.chain_log,
                            self.params.min_match,
                            &mut self.hash_table,
                            &mut self.hash_long,
                        );
                    }
                }
            }

            #[cfg(feature = "ldm")]
            let used_ldm = if let Some(ref mut ldm) = self.ldm_state {
                ldm.compress_block(
                    &self.window_buf,
                    plen,
                    self.window_buf.len(),
                    1usize << self.params.window_log,
                    &self.params,
                    &self.rep_offsets,
                    &mut self.hash_table,
                    &mut self.hash_long,
                    &mut self.sequences,
                );
                true
            } else {
                false
            };
            #[cfg(not(feature = "ldm"))]
            let used_ldm = false;

            if !used_ldm {
                match self.params.strategy {
                    Strategy::Fast => {
                        fast::compress_fast_block(
                            &self.window_buf,
                            plen,
                            self.window_buf.len(),
                            &self.params,
                            &self.rep_offsets,
                            &mut self.hash_table,
                            &mut self.sequences,
                        );
                    }
                    Strategy::DFast => {
                        dfast::compress_dfast_block(
                            &self.window_buf,
                            plen,
                            self.window_buf.len(),
                            &self.params,
                            &self.rep_offsets,
                            &mut self.hash_table,
                            &mut self.hash_long,
                            &mut self.sequences,
                        );
                    }
                }
            }

            if self.params.force_raw_literals {
                block_encoder::encode_compressed_block_raw(
                    chunk,
                    &self.sequences,
                    &mut self.rep_offsets,
                    last,
                    out,
                    &mut self.workspace,
                )?;
            } else {
                block_encoder::encode_compressed_block(
                    chunk,
                    &self.sequences,
                    &mut self.rep_offsets,
                    last,
                    out,
                    &mut self.workspace,
                    strategy::block_policy(&self.params, chunk.len()),
                )?;
            }
        }

        // Past two windows, keep only the last window and move the table
        // positions down by what was dropped.
        let window_size = 1usize << self.params.window_log;
        if self.window_buf.len() > window_size * 2 {
            let shift = self.window_buf.len() - window_size;
            reduce_hash_table(&mut self.hash_table, shift as u32);
            if !self.hash_long.is_empty() {
                reduce_hash_table(&mut self.hash_long, shift as u32);
            }
            #[cfg(feature = "ldm")]
            if let Some(ref mut ldm) = self.ldm_state {
                ldm.reduce_positions(shift as u32);
            }
            self.window_buf.copy_within(shift.., 0);
            self.window_buf.truncate(window_size);
        }

        self.block_start = self.window_buf.len();
        self.first_block = false;
        Ok(())
    }
}
