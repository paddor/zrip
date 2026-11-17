//! Sans-io frame state shared by [`FrameEncoder`](crate::streaming::FrameEncoder)
//! and [`PushEncoder`](crate::push::PushEncoder).
//!
//! Buffers input up to one block, encodes blocks with the window and match
//! tables carried across them, and appends frame bytes to a caller's `Vec`.
#![forbid(unsafe_code)]

use alloc::vec;
use alloc::vec::Vec;

use crate::block_encoder::{self, BlockEncodeWorkspace};
use crate::dfast;
use crate::fast;
#[cfg(feature = "ldm")]
use crate::ldm::LdmState;
use crate::rebase::reduce_hash_table;
use crate::strategy::{self, LevelParams, Strategy};
use crate::write_frame_header_without_content_size;
use zrip_core::Sequence;
use zrip_core::dict::Dictionary;
use zrip_core::error::CompressError;
use zrip_core::frame::MAX_BLOCK_SIZE;
use zrip_core::xxhash::Xxh64State;

/// Smallest growth step for the window once a frame spans blocks.
const GROW_STEP: usize = 8 << 20;

pub(crate) struct EncodeState {
    params: LevelParams,
    rep_offsets: [u32; 3],
    hasher: Xxh64State,
    workspace: BlockEncodeWorkspace,
    dict: Option<Dictionary>,
    first_block: bool,
    hash_table: Vec<u32>,
    hash_long: Vec<u32>,
    sequences: Vec<Sequence>,
    /// The window: dictionary, then input, then the next block's input from
    /// `block_start`. Input goes straight here, so blocks need no copy.
    window_buf: Vec<u8>,
    block_start: usize,
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
            hash_table: Vec::new(),
            hash_long: Vec::new(),
            sequences: Vec::new(),
            window_buf,
            block_start,
            #[cfg(feature = "ldm")]
            ldm_state,
        }
    }

    /// Starts a new frame, keeping allocations. Drops buffered input.
    pub(crate) fn reset(&mut self) {
        self.first_block = self.dict.is_some();
        self.rep_offsets = match &self.dict {
            Some(d) => *d.rep_offsets(),
            None => [1, 4, 8],
        };
        self.hasher = Xxh64State::new(0);
        self.workspace.prev_huffman = None;
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

    /// Largest block this frame uses: 128 KiB, or the window if smaller.
    pub(crate) fn block_size(&self) -> usize {
        MAX_BLOCK_SIZE.min(1usize << self.params.window_log)
    }

    /// Bytes waiting for the next block.
    pub(crate) fn buffered(&self) -> usize {
        self.window_buf.len() - self.block_start
    }

    /// The input waiting for the next block.
    pub(crate) fn buffered_input(&self) -> &[u8] {
        &self.window_buf[self.block_start..]
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
            // Room for one block first, so short frames stay small. Past
            // that, jump toward the most the window holds (two windows and a
            // block) in big steps: each doubling step copies the window and
            // faults in fresh pages.
            let window = 1usize << self.params.window_log;
            let dict_len = self.dict.as_ref().map_or(0, |d| d.content().len());
            let most = (2 * window).max(dict_len + window) + self.block_size();
            let first = dict_len + self.block_size();
            let target = if needed <= first {
                first
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

        if self.hash_table.is_empty() {
            (self.hash_table, self.hash_long) = alloc_hash_tables(&self.params);
        }
        let seed_dict = self.first_block && self.dict.is_some();
        let plen = self.block_start;
        let chunk = &self.window_buf[plen..];

        out.reserve(chunk.len() + 32);
        if crate::skip_match_search(&self.params, chunk.len(), chunk) {
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
            } else if plen == 0 {
                self.hash_table.fill(0);
                if !self.hash_long.is_empty() {
                    self.hash_long.fill(0);
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

fn alloc_hash_tables(params: &LevelParams) -> (Vec<u32>, Vec<u32>) {
    match params.strategy {
        Strategy::Fast => (vec![0u32; 1usize << params.hash_log], Vec::new()),
        Strategy::DFast => (
            vec![0u32; 1usize << params.chain_log],
            vec![0u32; 1usize << params.hash_log],
        ),
    }
}
