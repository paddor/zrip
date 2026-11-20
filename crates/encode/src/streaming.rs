//! [`FrameEncoder`], a streaming encoder over [`std::io::Write`].
#![forbid(unsafe_code)]

use std::io::{self, Write};

use crate::frame_state::EncodeState;
use crate::strategy::{self, LevelParams};
use zrip_core::dict::Dictionary;
use zrip_core::error::CompressError;

#[derive(Clone, Copy)]
enum EncoderState {
    Ready,
    Finished,
    Failed,
}

/// Streaming zstd compressor implementing [`Write`].
///
/// Buffers input until a full block (128 KiB, or the window if smaller) is
/// ready, then compresses and writes it to the underlying writer. Call [`finish`](Self::finish)
/// to flush the final block, write the content checksum, and recover the
/// writer.
///
/// After a write, flush, or finalization error, the encoder cannot be reused.
/// Later writes, flushes, [`finish`](Self::finish), and [`reset`](Self::reset)
/// return errors. Discard the encoder and its incomplete output.
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
    frame: EncodeState,
    header_written: bool,
    state: EncoderState,
    block_out: Vec<u8>,
}

impl<W: Write> FrameEncoder<W> {
    /// Creates a new streaming encoder at the given level (-7..=4; -8 runs as -7).
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

    /// Creates a new streaming encoder with a dictionary at the given level (-7..=4; -8 runs as -7).
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
        Ok(Self {
            inner: writer,
            frame: EncodeState::new(params, dict),
            header_written: false,
            state: EncoderState::Ready,
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
        self.state = EncoderState::Ready;
        self.frame.reset();
        Ok(old)
    }

    fn finish_frame(&mut self) -> io::Result<()> {
        if matches!(self.state, EncoderState::Failed) {
            return Err(io::Error::other("encoder cannot be reused after an error"));
        }
        if matches!(self.state, EncoderState::Finished) {
            return Ok(());
        }
        self.state = EncoderState::Failed;

        if !self.header_written {
            self.write_header()?;
        }

        self.block_out.clear();
        self.frame
            .encode_block(true, &mut self.block_out)
            .map_err(io::Error::other)?;
        self.frame.write_checksum(&mut self.block_out);
        self.inner.write_all(&self.block_out)?;
        self.state = EncoderState::Finished;
        Ok(())
    }

    fn write_header(&mut self) -> io::Result<()> {
        self.block_out.clear();
        self.frame
            .write_header(&mut self.block_out)
            .map_err(io::Error::other)?;
        self.inner.write_all(&self.block_out)?;
        self.header_written = true;
        Ok(())
    }

    fn flush_block(&mut self) -> io::Result<()> {
        self.block_out.clear();
        self.frame
            .encode_block(false, &mut self.block_out)
            .map_err(io::Error::other)?;
        self.inner.write_all(&self.block_out)
    }
}

impl<W: Write> Write for FrameEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if matches!(self.state, EncoderState::Failed) {
            return Err(io::Error::other("encoder cannot be reused after an error"));
        }
        if matches!(self.state, EncoderState::Finished) {
            return Err(io::Error::other("encoder already finished"));
        }
        // Any early return leaves the encoder failed, including partial writes.
        self.state = EncoderState::Failed;

        if !self.header_written {
            self.write_header()?;
        }

        let block_size = self.frame.block_size();
        let mut consumed = 0;
        while consumed < buf.len() {
            consumed += self.frame.take_input(&buf[consumed..]);
            if self.frame.buffered() >= block_size {
                self.flush_block()?;
            }
        }

        self.state = EncoderState::Ready;
        Ok(consumed)
    }

    fn flush(&mut self) -> io::Result<()> {
        if matches!(self.state, EncoderState::Failed) {
            return Err(io::Error::other("encoder cannot be reused after an error"));
        }
        let state = self.state;
        self.state = EncoderState::Failed;
        if self.frame.buffered() > 0 {
            self.flush_block()?;
        }
        self.inner.flush()?;
        self.state = state;
        Ok(())
    }
}
