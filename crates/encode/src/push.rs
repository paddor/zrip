//! Push-style (sans-io) encoding: the caller feeds input chunks and drains
//! frame bytes into its own buffers.
#![forbid(unsafe_code)]

use alloc::vec::Vec;

use crate::frame_state::EncodeState;
use crate::strategy::{self, LevelParams};
use zrip_core::dict::Dictionary;
use zrip_core::error::CompressError;
pub use zrip_core::push::{Progress, Status};

/// What [`PushEncoder::encode`] should do once it has consumed the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndOp {
    /// Keep buffering; emit blocks only as they fill.
    Continue,
    /// End the current block so everything consumed so far can be decoded.
    Flush,
    /// End the frame: last block and content checksum.
    End,
}

/// Push-style zstd compressor.
///
/// Feed input in chunks of any size with [`encode`](Self::encode) and drain
/// frame bytes into buffers of any size. No I/O, no `std`; it needs only the
/// `alloc` feature. `FrameEncoder` is the `std::io::Write` counterpart.
///
/// Frames carry a content checksum. A frame that ends before any of it was
/// written, such as all input passed with [`EndOp::End`], is encoded in one
/// pass like `zrip::compress`: same bytes, content size in the header, and
/// tables sized to the input. Longer streams have no content size. Hash
/// tables, window and block buffers are allocated on first use and reused
/// for every block and every frame.
///
/// After an error, every call returns that error until
/// [`reset`](Self::reset).
///
/// ```
/// use zrip::push::{EndOp, PushEncoder, Status};
///
/// let data = b"hello, push world! ".repeat(1000);
/// let mut enc = PushEncoder::new(1).unwrap();
/// let mut frame = Vec::new();
/// let mut buf = [0u8; 256];
/// for chunk in data.chunks(100) {
///     let mut input = chunk;
///     while !input.is_empty() {
///         let p = enc.encode(input, &mut buf, EndOp::Continue).unwrap();
///         frame.extend_from_slice(&buf[..p.produced]);
///         input = &input[p.consumed..];
///     }
/// }
/// loop {
///     let p = enc.encode(&[], &mut buf, EndOp::End).unwrap();
///     frame.extend_from_slice(&buf[..p.produced]);
///     if p.status == Status::Finished {
///         break;
///     }
/// }
/// assert_eq!(zrip::decompress(&frame).unwrap(), data);
/// ```
pub struct PushEncoder {
    frame: EncodeState,
    /// Level and options, for frames encoded in one pass.
    level: i32,
    opts: Option<strategy::Options>,
    /// Frame bytes in `pending[pending_pos..]` are not delivered yet.
    pending: Vec<u8>,
    pending_pos: usize,
    header_written: bool,
    /// The frame's last bytes are in `pending`; report
    /// [`Status::Finished`] once they are delivered.
    frame_done: bool,
    error: Option<CompressError>,
}

impl core::fmt::Debug for PushEncoder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PushEncoder")
            .field("pending_output", &(self.pending.len() - self.pending_pos))
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl PushEncoder {
    /// Creates an encoder at the given level (-7..=4; -8 runs as -7).
    pub fn new(level: i32) -> Result<Self, CompressError> {
        let params = strategy::level_params(level).ok_or(CompressError::InvalidLevel(level))?;
        Ok(Self::from_params(level, None, params, None))
    }

    /// Creates an encoder with [`Options`](strategy::Options) for large
    /// windows and/or LDM.
    pub fn with_options(level: i32, opts: &strategy::Options) -> Result<Self, CompressError> {
        let mut params = strategy::level_params(level).ok_or(CompressError::InvalidLevel(level))?;
        strategy::apply_options(&mut params, opts);
        Ok(Self::from_params(level, Some(opts.clone()), params, None))
    }

    /// Creates an encoder with a dictionary.
    pub fn with_dict(level: i32, dict: Dictionary) -> Result<Self, CompressError> {
        let params = strategy::level_params(level).ok_or(CompressError::InvalidLevel(level))?;
        Ok(Self::from_params(level, None, params, Some(dict)))
    }

    fn from_params(
        level: i32,
        opts: Option<strategy::Options>,
        params: LevelParams,
        dict: Option<Dictionary>,
    ) -> Self {
        Self {
            frame: EncodeState::new(params, dict),
            level,
            opts,
            pending: Vec::new(),
            pending_pos: 0,
            header_written: false,
            frame_done: false,
            error: None,
        }
    }

    /// Consumes input and writes frame bytes into `output`.
    ///
    /// - [`EndOp::Continue`]: returns [`Status::NeedsInput`] once all input
    ///   is consumed and every full block is delivered.
    /// - [`EndOp::Flush`]: also ends the current block; returns
    ///   [`Status::NeedsInput`] once it is delivered.
    /// - [`EndOp::End`]: also ends the frame; returns [`Status::Finished`]
    ///   once its last byte is delivered. The next call starts a new frame.
    ///
    /// [`Status::NeedsOutput`] means `output` filled up first: call again
    /// with the rest of the input (`consumed` tells how much was taken) and
    /// the same `op`.
    pub fn encode(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        op: EndOp,
    ) -> Result<Progress, CompressError> {
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        let mut consumed = 0;
        let mut produced = 0;
        let result = self.run(input, output, op, &mut consumed, &mut produced);
        match result {
            Ok(status) => Ok(Progress {
                consumed,
                produced,
                status,
            }),
            Err(e) => {
                self.error = Some(e.clone());
                Err(e)
            }
        }
    }

    /// Encodes all of `input` and appends the frame bytes to `output`.
    /// With [`EndOp::End`], `output` then ends with a complete frame.
    ///
    /// Bytes still pending from [`encode`](Self::encode) go out first. If
    /// they end a frame, `input` starts the next one.
    pub fn encode_to_vec(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
        op: EndOp,
    ) -> Result<(), CompressError> {
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        output.extend_from_slice(&self.pending[self.pending_pos..]);
        self.pending.clear();
        self.pending_pos = 0;
        if self.frame_done {
            self.end_frame();
            // The call only finished the pending frame.
            if input.is_empty() {
                return Ok(());
            }
        }
        let mut consumed = 0;
        loop {
            if self.frame_done {
                self.end_frame();
                return Ok(());
            }
            match self.advance(input, &mut consumed, op, output) {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(e) => {
                    self.error = Some(e.clone());
                    return Err(e);
                }
            }
        }
    }

    /// Whether frame bytes are still waiting to be delivered.
    pub fn has_pending_output(&self) -> bool {
        self.pending_pos < self.pending.len()
    }

    /// Discards the current frame and any error, keeping allocations. The
    /// next call starts a new frame.
    pub fn reset(&mut self) {
        self.frame.reset();
        self.pending.clear();
        self.pending_pos = 0;
        self.header_written = false;
        self.frame_done = false;
        self.error = None;
    }

    fn run(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        op: EndOp,
        consumed: &mut usize,
        produced: &mut usize,
    ) -> Result<Status, CompressError> {
        loop {
            if self.has_pending_output() {
                let src = &self.pending[self.pending_pos..];
                let n = src.len().min(output.len() - *produced);
                output[*produced..*produced + n].copy_from_slice(&src[..n]);
                *produced += n;
                self.pending_pos += n;
                if self.has_pending_output() {
                    return Ok(Status::NeedsOutput);
                }
            }
            self.pending.clear();
            self.pending_pos = 0;

            if self.frame_done {
                self.end_frame();
                return Ok(Status::Finished);
            }

            let mut pending = core::mem::take(&mut self.pending);
            let advanced = self.advance(input, consumed, op, &mut pending);
            self.pending = pending;
            if !advanced? {
                return Ok(Status::NeedsInput);
            }
        }
    }

    /// Takes input and appends the next block to `out` once one is due,
    /// with the frame header before the first block and the checksum after
    /// the last. Returns false if no block is due yet.
    fn advance(
        &mut self,
        input: &[u8],
        consumed: &mut usize,
        op: EndOp,
        out: &mut Vec<u8>,
    ) -> Result<bool, CompressError> {
        // The frame ends before any of it went out, so its size is known:
        // encode it in one pass, as C zstd does. Tables are then sized to
        // the input, and the header records the content size.
        if op == EndOp::End && !self.header_written {
            let rest = &input[*consumed..];
            if self.frame.buffered() == 0 {
                self.one_pass(Some(rest), out)?;
                *consumed = input.len();
                self.frame_done = true;
                return Ok(true);
            }
            if self.frame.buffered() + rest.len() <= self.frame.block_size() {
                *consumed += self.frame.take_input(rest);
                self.one_pass(None, out)?;
                self.frame_done = true;
                return Ok(true);
            }
        }

        *consumed += self.frame.take_input(&input[*consumed..]);
        let more_input = *consumed < input.len();
        let full = self.frame.buffered() == self.frame.block_size();

        // A full block waits for the next byte, so `End` can still mark it
        // last instead of writing an empty last block after it.
        let last = !more_input && op == EndOp::End;
        let due = (full && more_input)
            || last
            || (!more_input && op == EndOp::Flush && self.frame.buffered() > 0);
        if !due {
            return Ok(false);
        }

        if !self.header_written {
            self.frame.write_header(out)?;
            self.header_written = true;
        }
        self.frame.encode_block(last, out)?;
        if last {
            self.frame.write_checksum(out);
            self.frame_done = true;
        }
        Ok(true)
    }

    /// Appends `input`, or the buffered input if `None`, as one whole frame,
    /// like the one-shot API but in the encoder's tables and buffers.
    fn one_pass(&mut self, input: Option<&[u8]>, out: &mut Vec<u8>) -> Result<(), CompressError> {
        let len = input.map_or_else(|| self.frame.buffered(), <[u8]>::len);
        let params = match (self.frame.dict(), &self.opts) {
            (Some(dict), _) => {
                strategy::level_params_with_prefix(self.level, dict.content().len(), len)
            }
            (None, Some(opts)) => Some(crate::params_with_options(self.level, len, opts)?),
            (None, None) => strategy::level_params_for_size(self.level, len),
        }
        .ok_or(CompressError::InvalidLevel(self.level))?;
        self.frame.encode_one_pass(input, &params, out)
    }

    fn end_frame(&mut self) {
        self.frame_done = false;
        self.header_written = false;
        self.frame.reset();
    }
}
