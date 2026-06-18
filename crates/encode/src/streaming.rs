#![forbid(unsafe_code)]

use std::io::{self, Write};

use crate::block_encoder::{self, BlockEncodeWorkspace};
use crate::dfast;
use crate::fast;
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
    combined: Vec<u8>,
    block_out: Vec<u8>,
}

impl<W: Write> FrameEncoder<W> {
    /// Creates a new streaming encoder at the given level (-7..=4).
    pub fn new(writer: W, level: i32) -> Result<Self, CompressError> {
        let params = strategy::level_params(level).ok_or(CompressError::InvalidLevel(level))?;
        let (hash_table, hash_long) = alloc_hash_tables(&params);
        Ok(Self {
            inner: writer,
            params,
            buffer: Vec::new(),
            rep_offsets: [1, 4, 8],
            hasher: Xxh64State::new(0),
            header_written: false,
            finished: false,
            workspace: BlockEncodeWorkspace::new(),
            dict: None,
            first_block: false,
            hash_table,
            hash_long,
            sequences: Vec::new(),
            combined: Vec::new(),
            block_out: Vec::new(),
        })
    }

    /// Creates a new streaming encoder with a dictionary at the given level (-7..=4).
    pub fn with_dict(writer: W, level: i32, dict: Dictionary) -> Result<Self, CompressError> {
        let params = strategy::level_params(level).ok_or(CompressError::InvalidLevel(level))?;
        let (hash_table, hash_long) = alloc_hash_tables(&params);
        let rep_offsets = *dict.rep_offsets();
        Ok(Self {
            inner: writer,
            params,
            buffer: Vec::new(),
            rep_offsets,
            hasher: Xxh64State::new(0),
            header_written: false,
            finished: false,
            workspace: BlockEncodeWorkspace::new(),
            dict: Some(dict),
            first_block: true,
            hash_table,
            hash_long,
            sequences: Vec::new(),
            combined: Vec::new(),
            block_out: Vec::new(),
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
        let checksum = (hash & 0xFFFFFFFF) as u32;
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

        self.block_out.clear();
        self.block_out.reserve(chunk.len() + 32);
        if crate::block_looks_incompressible(&chunk) {
            block_encoder::encode_raw_block(&chunk, last, &mut self.block_out);
        } else {
            let use_prefix = self.first_block && self.dict.is_some();
            if use_prefix {
                let prefix = self.dict.as_ref().unwrap().content();
                match self.params.strategy {
                    Strategy::Fast => {
                        fast::compress_fast_with_prefix_reuse(
                            &chunk,
                            &self.params,
                            &self.rep_offsets,
                            prefix,
                            &mut self.hash_table,
                            &mut self.sequences,
                            &mut self.combined,
                        );
                    }
                    Strategy::DFast => {
                        dfast::compress_dfast_with_prefix_reuse(
                            &chunk,
                            &self.params,
                            &self.rep_offsets,
                            prefix,
                            &mut self.hash_table,
                            &mut self.hash_long,
                            &mut self.sequences,
                            &mut self.combined,
                        );
                    }
                }
            } else {
                self.hash_table.fill(0);
                if !self.hash_long.is_empty() {
                    self.hash_long.fill(0);
                }
                match self.params.strategy {
                    Strategy::Fast => {
                        fast::compress_fast_block(
                            &chunk,
                            0,
                            chunk.len(),
                            &self.params,
                            &self.rep_offsets,
                            &mut self.hash_table,
                            &mut self.sequences,
                        );
                    }
                    Strategy::DFast => {
                        dfast::compress_dfast_block(
                            &chunk,
                            0,
                            chunk.len(),
                            &self.params,
                            &self.rep_offsets,
                            &mut self.hash_table,
                            &mut self.hash_long,
                            &mut self.sequences,
                        );
                    }
                }
            };
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
