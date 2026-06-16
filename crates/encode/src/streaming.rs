#![forbid(unsafe_code)]

use std::io::{self, Write};

use crate::block_encoder::{self, BlockEncodeWorkspace};
use crate::dfast;
use crate::fast;
use crate::strategy::{self, LevelParams, Strategy};
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
/// ```
/// use std::io::Write;
///
/// let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
/// encoder.write_all(b"streaming data").unwrap();
/// let compressed = encoder.finish().unwrap();
/// assert!(!compressed.is_empty());
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
}

impl<W: Write> FrameEncoder<W> {
    /// Creates a new streaming encoder at the given level (-7..=4).
    pub fn new(writer: W, level: i32) -> Result<Self, CompressError> {
        let params = strategy::level_params(level).ok_or(CompressError::InvalidLevel(level))?;
        Ok(Self {
            inner: writer,
            params,
            buffer: Vec::new(),
            rep_offsets: [1, 4, 8],
            hasher: Xxh64State::new(0),
            header_written: false,
            finished: false,
            workspace: BlockEncodeWorkspace::new(),
        })
    }

    /// Flushes remaining data, writes the content checksum, and returns the inner writer.
    pub fn finish(mut self) -> Result<W, io::Error> {
        if self.finished {
            return Ok(self.inner);
        }
        self.finished = true;

        if !self.header_written {
            self.write_header()?;
        }

        self.flush_block(true)?;

        let hash = self.hasher.finish();
        let checksum = (hash & 0xFFFFFFFF) as u32;
        self.inner.write_all(&checksum.to_le_bytes())?;

        Ok(self.inner)
    }

    fn write_header(&mut self) -> io::Result<()> {
        self.header_written = true;

        self.inner.write_all(&ZSTD_MAGIC.to_le_bytes())?;

        let window_log = self.params.window_log;
        let descriptor = 0x04u8;
        self.inner.write_all(&[descriptor])?;

        let mantissa = 0u8;
        let exponent = (window_log - 10) as u8;
        let window_descriptor = (exponent << 3) | mantissa;
        self.inner.write_all(&[window_descriptor])?;

        Ok(())
    }

    fn flush_block(&mut self, last: bool) -> io::Result<()> {
        if self.buffer.is_empty() && last {
            let mut block_out = Vec::new();
            block_encoder::encode_raw_block(&[], true, &mut block_out);
            self.inner.write_all(&block_out)?;
            return Ok(());
        }

        if self.buffer.is_empty() {
            return Ok(());
        }

        let chunk = core::mem::take(&mut self.buffer);

        let mut block_out = Vec::with_capacity(chunk.len() + 32);
        if crate::block_looks_incompressible(&chunk) {
            block_encoder::encode_raw_block(&chunk, last, &mut block_out);
        } else {
            let sequences = match self.params.strategy {
                Strategy::Fast => fast::compress_fast(&chunk, &self.params, &self.rep_offsets),
                Strategy::DFast => dfast::compress_dfast(&chunk, &self.params, &self.rep_offsets),
            };
            if self.params.force_raw_literals {
                block_encoder::encode_compressed_block_raw(
                    &chunk,
                    &sequences,
                    &mut self.rep_offsets,
                    last,
                    &mut block_out,
                    &mut self.workspace,
                );
            } else {
                block_encoder::encode_compressed_block(
                    &chunk,
                    &sequences,
                    &mut self.rep_offsets,
                    last,
                    &mut block_out,
                    &mut self.workspace,
                );
            }
        }

        self.inner.write_all(&block_out)?;
        Ok(())
    }
}

impl<W: Write> Write for FrameEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.finished {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "encoder already finished",
            ));
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
