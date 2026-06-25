#![forbid(unsafe_code)]

use std::io::{self, Write};

use crate::block_encoder::{self, BlockEncodeWorkspace};
use crate::dfast;
use crate::fast;
#[cfg(feature = "ldm")]
use crate::ldm::LdmState;
use crate::strategy::{self, LevelParams, Strategy};
use zrip_core::Sequence;
use zrip_core::dict::Dictionary;
use zrip_core::error::CompressError;
use zrip_core::frame::{MAX_BLOCK_SIZE, ZSTD_MAGIC};
use zrip_core::xxhash::Xxh64State;

/// Streaming zstd compressor implementing [`Write`].
///
/// Buffers input until a full block (128 KiB) is ready, then compresses
/// and writes it to the underlying writer. Call [`finish`](Self::finish)
/// to flush the final block, write the content checksum, and recover the
/// writer.
///
/// Internal buffers (hash tables, sequence scratch, block encoder workspace)
/// are allocated once and reused across blocks. To reuse them across
/// multiple frames, call [`reset`](Self::reset) instead of `finish`:
///
/// ```
/// use std::io::Write;
///
/// let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
/// encoder.write_all(b"first frame").unwrap();
/// let first = encoder.reset(Vec::new()).unwrap();   // reuses buffers
/// encoder.write_all(b"second frame").unwrap();
/// let second = encoder.finish().unwrap();
/// ```
pub struct FrameEncoder<W: Write> {
    inner: W,
    params: LevelParams,
    buffer: Vec<u8>,
    rep_offsets: [u32; 3],
    hasher: Xxh64State,
    header_written: bool,
    finished: bool,
    workspace: BlockEncodeWorkspace,
    dict: Option<Dictionary>,
    first_block: bool,
    hash_table: Vec<u32>,
    hash_long: Vec<u32>,
    sequences: Vec<Sequence>,
    block_out: Vec<u8>,
    window_buf: Vec<u8>,
    #[cfg(feature = "ldm")]
    ldm_state: Option<LdmState>,
}

impl<W: Write> FrameEncoder<W> {
    /// Creates a new streaming encoder at the given level (-7..=4).
    pub fn new(writer: W, level: i32) -> Result<Self, CompressError> {
        let params = strategy::level_params(level).ok_or(CompressError::InvalidLevel(level))?;
        Self::from_params(writer, params, None)
    }

    /// Creates a new streaming encoder with [`Options`](strategy::Options)
    /// for large windows and/or LDM.
    pub fn with_options(
        writer: W,
        level: i32,
        opts: &strategy::Options,
    ) -> Result<Self, CompressError> {
        let mut params = strategy::level_params(level).ok_or(CompressError::InvalidLevel(level))?;
        strategy::apply_options(&mut params, opts);
        Self::from_params(writer, params, None)
    }

    /// Creates a new streaming encoder with a dictionary at the given level (-7..=4).
    pub fn with_dict(writer: W, level: i32, dict: Dictionary) -> Result<Self, CompressError> {
        let params = strategy::level_params(level).ok_or(CompressError::InvalidLevel(level))?;
        Self::from_params(writer, params, Some(dict))
    }

    #[allow(clippy::unnecessary_wraps)]
    fn from_params(
        writer: W,
        params: LevelParams,
        dict: Option<Dictionary>,
    ) -> Result<Self, CompressError> {
        let (hash_table, hash_long) = alloc_hash_tables(&params);
        let (rep_offsets, first_block) = match &dict {
            Some(d) => (*d.rep_offsets(), true),
            None => ([1, 4, 8], false),
        };
        let mut window_buf = Vec::new();
        if let Some(ref d) = dict {
            window_buf.extend_from_slice(d.content());
        }
        #[cfg(feature = "ldm")]
        let ldm_state = params.ldm_params.as_ref().map(LdmState::new);
        Ok(Self {
            inner: writer,
            params,
            buffer: Vec::new(),
            rep_offsets,
            hasher: Xxh64State::new(0),
            header_written: false,
            finished: false,
            workspace: BlockEncodeWorkspace::new(),
            dict,
            first_block,
            hash_table,
            hash_long,
            sequences: Vec::new(),
            block_out: Vec::new(),
            window_buf,
            #[cfg(feature = "ldm")]
            ldm_state,
        })
    }

    /// Flushes remaining data, writes the content checksum, and returns the inner writer.
    pub fn finish(mut self) -> Result<W, io::Error> {
        self.finish_frame()?;
        Ok(self.inner)
    }

    /// Finishes the current frame and installs `new_writer` for the next one.
    ///
    /// Returns the previous writer containing the completed frame. All
    /// internal buffers (hash tables, workspace, block scratch) stay
    /// allocated and are reused for the next frame.
    pub fn reset(&mut self, new_writer: W) -> Result<W, io::Error> {
        self.finish_frame()?;
        let old = core::mem::replace(&mut self.inner, new_writer);
        self.header_written = false;
        self.finished = false;
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
        #[cfg(feature = "ldm")]
        if let Some(ref mut ldm) = self.ldm_state {
            ldm.reset();
        }
        Ok(old)
    }

    fn finish_frame(&mut self) -> io::Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;

        if !self.header_written {
            self.write_header()?;
        }

        self.flush_block(true)?;

        let hash = self.hasher.finish();
        let checksum = (hash & 0xFFFF_FFFF) as u32;
        self.inner.write_all(&checksum.to_le_bytes())?;
        Ok(())
    }

    fn write_header(&mut self) -> io::Result<()> {
        self.header_written = true;

        self.inner.write_all(&ZSTD_MAGIC.to_le_bytes())?;

        let window_log = self.params.window_log;

        let dict_id_flag = if let Some(ref dict) = self.dict {
            let id = dict.id();
            if id <= 0xFF {
                1u8
            } else if id <= 0xFFFF {
                2
            } else {
                3
            }
        } else {
            0
        };

        let descriptor = 0x04u8 | dict_id_flag;
        self.inner.write_all(&[descriptor])?;

        let mantissa = 0u8;
        let exponent = (window_log - 10) as u8;
        let window_descriptor = (exponent << 3) | mantissa;
        self.inner.write_all(&[window_descriptor])?;

        if let Some(ref dict) = self.dict {
            let id = dict.id();
            match dict_id_flag {
                1 => self.inner.write_all(&[id as u8])?,
                2 => self.inner.write_all(&(id as u16).to_le_bytes())?,
                3 => self.inner.write_all(&id.to_le_bytes())?,
                _ => unreachable!(),
            }
        }

        Ok(())
    }

    fn flush_block(&mut self, last: bool) -> io::Result<()> {
        if self.buffer.is_empty() && last {
            self.block_out.clear();
            block_encoder::encode_raw_block(&[], true, &mut self.block_out);
            self.inner.write_all(&self.block_out)?;
            return Ok(());
        }

        if self.buffer.is_empty() {
            return Ok(());
        }

        let chunk = core::mem::take(&mut self.buffer);
        let seed_dict = self.first_block && self.dict.is_some();

        let plen = self.window_buf.len();
        self.window_buf.extend_from_slice(&chunk);

        self.block_out.clear();
        self.block_out.reserve(chunk.len() + 32);
        if crate::block_looks_incompressible(&chunk) {
            block_encoder::encode_raw_block(&chunk, last, &mut self.block_out);
        } else {
            if seed_dict {
                match self.params.strategy {
                    Strategy::Fast => {
                        fast::prefill_hash_table(
                            &self.window_buf,
                            plen,
                            self.params.hash_log,
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
                    &chunk,
                    &self.sequences,
                    &mut self.rep_offsets,
                    last,
                    &mut self.block_out,
                    &mut self.workspace,
                );
            } else {
                block_encoder::encode_compressed_block(
                    &chunk,
                    &self.sequences,
                    &mut self.rep_offsets,
                    last,
                    &mut self.block_out,
                    &mut self.workspace,
                );
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

        self.first_block = false;
        self.inner.write_all(&self.block_out)?;
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

fn reduce_hash_table(table: &mut [u32], shift: u32) {
    for entry in table.iter_mut() {
        if *entry < shift {
            *entry = 0;
        } else {
            *entry -= shift;
        }
    }
}

impl<W: Write> Write for FrameEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.finished {
            return Err(io::Error::other("encoder already finished"));
        }

        if !self.header_written {
            self.write_header()?;
        }

        self.hasher.update(buf);

        let mut consumed = 0;
        while consumed < buf.len() {
            let space = MAX_BLOCK_SIZE - self.buffer.len();
            let n = space.min(buf.len() - consumed);
            self.buffer.extend_from_slice(&buf[consumed..consumed + n]);
            consumed += n;

            if self.buffer.len() >= MAX_BLOCK_SIZE {
                self.flush_block(false)?;
            }
        }

        Ok(consumed)
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.buffer.is_empty() {
            self.flush_block(false)?;
        }
        self.inner.flush()
    }
}
