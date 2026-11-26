//! Push-style (sans-io) decoding: the caller feeds input chunks and drains
//! output into its own buffers.
#![forbid(unsafe_code)]

use alloc::vec::Vec;
use core::mem::MaybeUninit;

use crate::frame_state::{
    AfterBlock, BLOCK_HEADER_SIZE, CHECKSUM_SIZE, FRAME_PREFIX_SIZE, FrameState,
    SKIPPABLE_HEADER_SIZE, frame_header_size, is_skippable_magic,
};
use zrip_core::block::{BlockType, parse_block_header};
use zrip_core::dict::Dictionary;
use zrip_core::error::DecompressError;
use zrip_core::frame::ZSTD_MAGIC;
pub use zrip_core::push::{Progress, Status};

/// The next piece of input the decoder waits for.
#[derive(Clone, Copy)]
enum Unit {
    /// A frame's or skippable frame's 4-byte magic number.
    Magic,
    /// Frame header descriptor, which sizes the rest of the header.
    Descriptor,
    /// The rest of the frame header; the stage holds `size` bytes in total.
    Header {
        size: usize,
    },
    /// Length of a skippable frame.
    SkipSize,
    /// The rest of a skippable frame's payload, discarded as it arrives.
    Skip {
        remaining: u32,
    },
    BlockHeader,
    Raw {
        size: usize,
        last: bool,
    },
    Rle {
        size: usize,
        last: bool,
    },
    /// A compressed block, decoded once all of it arrived. A body split
    /// across calls is gathered in `stage`.
    Compressed {
        size: usize,
        last: bool,
    },
    Checksum,
    /// A call failed; `PushDecoder::error` holds the error.
    Failed,
}

/// Push-style zstd decompressor.
///
/// Feed compressed input in chunks of any size with [`decode`](Self::decode)
/// and drain output into buffers of any size. No I/O, no `std`; it needs only
/// the `alloc` feature. `FrameDecoder` is the `std::io::Read` counterpart.
///
/// Reads multi-frame streams and skips skippable frames. Each frame ends with
/// [`Status::Finished`]; calling again continues with the next frame. At end
/// of input, call [`finish`](Self::finish) to reject a truncated frame.
///
/// After an error, every call returns that error until
/// [`reset`](Self::reset).
///
/// ```
/// use zrip::push::{PushDecoder, Status};
///
/// let data = b"hello, push world! ".repeat(1000);
/// let compressed = zrip::compress(&data, 1).unwrap();
///
/// let mut dec = PushDecoder::new();
/// let mut out = Vec::new();
/// let mut buf = [0u8; 512];
/// for chunk in compressed.chunks(100) {
///     let mut input = chunk;
///     loop {
///         let p = dec.decode(input, &mut buf).unwrap();
///         out.extend_from_slice(&buf[..p.produced]);
///         input = &input[p.consumed..];
///         if p.status == Status::NeedsInput {
///             break;
///         }
///     }
/// }
/// // Drain anything still pending, then check the frame ended.
/// while dec.decode(&[], &mut buf).unwrap().status == Status::NeedsOutput {}
/// dec.finish().unwrap();
/// assert_eq!(out, data);
/// ```
pub struct PushDecoder {
    frame: FrameState,
    unit: Unit,
    /// Bytes of a unit that arrived split across calls.
    stage: Vec<u8>,
    /// Decoded bytes in `frame.block()[output_pos..output_end]` are not
    /// delivered yet.
    output_pos: usize,
    output_end: usize,
    /// A frame ended; report it once its output is delivered.
    frame_done: bool,
    /// A frame ended, its output was delivered, and no later input was read.
    at_frame_end: bool,
    /// The error every call returns until `reset`.
    error: Option<DecompressError>,
}

impl core::fmt::Debug for PushDecoder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PushDecoder")
            .field("pending_output", &(self.output_end - self.output_pos))
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl Default for PushDecoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Where decoded bytes go.
trait Sink {
    /// Writes up to `data.len()` bytes and returns how many fit.
    fn put(&mut self, data: &[u8]) -> usize;
    /// No byte more fits.
    fn full(&self) -> bool;
}

struct SliceSink<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl Sink for SliceSink<'_> {
    fn put(&mut self, data: &[u8]) -> usize {
        let n = data.len().min(self.buf.len() - self.pos);
        self.buf[self.pos..self.pos + n].copy_from_slice(&data[..n]);
        self.pos += n;
        n
    }

    fn full(&self) -> bool {
        self.pos == self.buf.len()
    }
}

struct UninitSink<'a> {
    buf: &'a mut [MaybeUninit<u8>],
    pos: usize,
}

impl Sink for UninitSink<'_> {
    fn put(&mut self, data: &[u8]) -> usize {
        let n = data.len().min(self.buf.len() - self.pos);
        // Compiles to a memcpy. `write_copy_of_slice` needs Rust 1.93.
        for (dst, &src) in self.buf[self.pos..self.pos + n].iter_mut().zip(data) {
            dst.write(src);
        }
        self.pos += n;
        n
    }

    fn full(&self) -> bool {
        self.pos == self.buf.len()
    }
}

impl Sink for Vec<u8> {
    fn put(&mut self, data: &[u8]) -> usize {
        self.extend_from_slice(data);
        data.len()
    }

    fn full(&self) -> bool {
        false
    }
}

/// Appends input to `stage` until it holds `size` bytes. Returns true once
/// it does.
fn fill_stage(stage: &mut Vec<u8>, input: &[u8], consumed: &mut usize, size: usize) -> bool {
    let take = (size - stage.len()).min(input.len() - *consumed);
    stage.extend_from_slice(&input[*consumed..*consumed + take]);
    *consumed += take;
    stage.len() == size
}

impl PushDecoder {
    /// Creates a decoder with [`DEFAULT_DECOMPRESS_LIMIT`](zrip_core::DEFAULT_DECOMPRESS_LIMIT).
    pub fn new() -> Self {
        Self::with_limit(zrip_core::DEFAULT_DECOMPRESS_LIMIT)
    }

    /// Creates a decoder that fails with [`DecompressError::OutputTooSmall`]
    /// once the stream decodes to more than `max_output` bytes.
    pub fn with_limit(max_output: usize) -> Self {
        Self::from_state(FrameState::new(None, max_output))
    }

    /// Creates a decoder with a dictionary.
    pub fn with_dict(dict: Dictionary) -> Self {
        Self::with_dict_and_limit(dict, zrip_core::DEFAULT_DECOMPRESS_LIMIT)
    }

    /// Creates a decoder with a dictionary and an output limit.
    pub fn with_dict_and_limit(dict: Dictionary, max_output: usize) -> Self {
        Self::from_state(FrameState::new(Some(dict), max_output))
    }

    fn from_state(frame: FrameState) -> Self {
        Self {
            frame,
            unit: Unit::Magic,
            stage: Vec::new(),
            output_pos: 0,
            output_end: 0,
            frame_done: false,
            at_frame_end: false,
            error: None,
        }
    }

    /// Sets the largest frame window to accept, in place of the default
    /// 128 MiB. Larger windows fail with [`DecompressError::WindowTooLarge`].
    /// The decoder keeps up to two windows of history, so this caps its
    /// memory. A frame header with a content size reserves that much
    /// history up front, up to 8 MiB and within the cap and the output
    /// limit; the rest grows with output.
    #[must_use]
    pub fn max_window(mut self, bytes: u64) -> Self {
        self.frame.set_max_window(bytes);
        self
    }

    /// Decodes from `input` into `output`.
    ///
    /// Returns how much input was consumed, how much output was written, and
    /// what to do next. Stops after each frame with [`Status::Finished`], so
    /// `consumed` can be less than `input.len()` then too.
    pub fn decode(&mut self, input: &[u8], output: &mut [u8]) -> Result<Progress, DecompressError> {
        let mut sink = SliceSink {
            buf: output,
            pos: 0,
        };
        let (consumed, status) = self.run(input, &mut sink, true)?;
        Ok(Progress {
            consumed,
            produced: sink.pos,
            status,
        })
    }

    /// Like [`decode`](Self::decode), but `output` may be uninitialized,
    /// so callers need not zero their buffers. On return, the first
    /// `produced` bytes of `output` are initialized. Bytes past them are left
    /// as they were, and no byte is ever made uninitialized, so callers may
    /// rely on both in unsafe code, as when they track initialized capacity.
    pub fn decode_uninit(
        &mut self,
        input: &[u8],
        output: &mut [MaybeUninit<u8>],
    ) -> Result<Progress, DecompressError> {
        let mut sink = UninitSink {
            buf: output,
            pos: 0,
        };
        let (consumed, status) = self.run(input, &mut sink, true)?;
        Ok(Progress {
            consumed,
            produced: sink.pos,
            status,
        })
    }

    /// Decodes all of `input`, across frame boundaries, and appends the
    /// output to `output`. Returns the number of bytes appended.
    pub fn decode_to_vec(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<usize, DecompressError> {
        let before = output.len();
        self.run(input, output, false)?;
        Ok(output.len() - before)
    }

    /// Checks that the input ended on a frame boundary.
    ///
    /// Returns [`DecompressError::InputExhausted`] if a frame is incomplete,
    /// and the stored error if decoding failed.
    pub fn finish(&self) -> Result<(), DecompressError> {
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        if matches!(self.unit, Unit::Magic) && self.stage.is_empty() {
            Ok(())
        } else {
            Err(DecompressError::InputExhausted)
        }
    }

    /// Whether the decoder stands at the end of a frame: one ended, all of
    /// its output was delivered, and no input after it was read yet.
    ///
    /// [`Status::Finished`] is reported once, so that a loop calling until
    /// [`Status::NeedsInput`] ends. This stays true until the next frame's
    /// input arrives or [`reset`](Self::reset), for callers that learn of
    /// the frame end later, such as after draining output in a flush.
    pub fn at_frame_end(&self) -> bool {
        self.at_frame_end
    }

    /// Whether decoded output is still waiting to be delivered.
    pub fn has_pending_output(&self) -> bool {
        self.output_pos < self.output_end
    }

    /// Discards the current stream and any error, keeping allocations.
    pub fn reset(&mut self) {
        self.frame.reset();
        self.unit = Unit::Magic;
        self.stage.clear();
        self.output_pos = 0;
        self.output_end = 0;
        self.frame_done = false;
        self.at_frame_end = false;
        self.error = None;
    }

    /// Drains pending output into `sink`, then steps through `input` until
    /// it runs out or `sink` is full. With `stop_at_frame_end`, returns
    /// [`Status::Finished`] after each frame.
    fn run<S: Sink>(
        &mut self,
        input: &[u8],
        sink: &mut S,
        stop_at_frame_end: bool,
    ) -> Result<(usize, Status), DecompressError> {
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        let mut consumed = 0;
        loop {
            if self.has_pending_output() {
                self.output_pos += sink.put(&self.frame.block()[self.output_pos..self.output_end]);
                if self.has_pending_output() {
                    return Ok((consumed, Status::NeedsOutput));
                }
            }
            if self.frame_done {
                self.frame_done = false;
                self.at_frame_end = true;
                if stop_at_frame_end {
                    return Ok((consumed, Status::Finished));
                }
            }
            // A full buffer stops before the next block header: a corrupt one
            // fails only a call that has room for the block.
            if matches!(self.unit, Unit::BlockHeader) && sink.full() {
                return Ok((consumed, Status::NeedsOutput));
            }
            let before = consumed;
            let stepped = self.step(input, &mut consumed);
            if consumed > before {
                self.at_frame_end = false;
            }
            match stepped {
                Ok(true) => {}
                Ok(false) => return Ok((consumed, Status::NeedsInput)),
                Err(e) => {
                    self.at_frame_end = false;
                    self.unit = Unit::Failed;
                    self.frame.discard_block();
                    self.output_pos = 0;
                    self.output_end = 0;
                    self.error = Some(e.clone());
                    return Err(e);
                }
            }
        }
    }

    /// Advances by one unit. Returns false when it needs more input.
    fn step(&mut self, input: &[u8], consumed: &mut usize) -> Result<bool, DecompressError> {
        let avail = input.len() - *consumed;
        match self.unit {
            Unit::Magic => {
                if !fill_stage(&mut self.stage, input, consumed, 4) {
                    return Ok(false);
                }
                let magic = u32::from_le_bytes([
                    self.stage[0],
                    self.stage[1],
                    self.stage[2],
                    self.stage[3],
                ]);
                self.unit = if is_skippable_magic(magic) {
                    Unit::SkipSize
                } else if magic == ZSTD_MAGIC {
                    Unit::Descriptor
                } else {
                    return Err(DecompressError::BadMagic);
                };
            }
            Unit::Descriptor => {
                if !fill_stage(&mut self.stage, input, consumed, FRAME_PREFIX_SIZE) {
                    return Ok(false);
                }
                self.unit = Unit::Header {
                    size: frame_header_size(&self.stage),
                };
            }
            Unit::Header { size } => {
                if !fill_stage(&mut self.stage, input, consumed, size) {
                    return Ok(false);
                }
                self.frame.begin_frame(&self.stage)?;
                self.stage.clear();
                self.unit = Unit::BlockHeader;
            }
            Unit::SkipSize => {
                if !fill_stage(&mut self.stage, input, consumed, SKIPPABLE_HEADER_SIZE) {
                    return Ok(false);
                }
                let remaining = u32::from_le_bytes([
                    self.stage[4],
                    self.stage[5],
                    self.stage[6],
                    self.stage[7],
                ]);
                self.stage.clear();
                self.unit = Unit::Skip { remaining };
            }
            Unit::Skip { remaining } => {
                // `n` fits in u32 because `remaining` does.
                let n = (remaining as usize).min(avail);
                *consumed += n;
                let remaining = remaining - n as u32;
                if remaining > 0 {
                    self.unit = Unit::Skip { remaining };
                    return Ok(false);
                }
                self.unit = Unit::Magic;
                self.frame_done = true;
            }
            Unit::BlockHeader => {
                let header = if self.stage.is_empty() && avail >= BLOCK_HEADER_SIZE {
                    let h = parse_block_header(&input[*consumed..*consumed + BLOCK_HEADER_SIZE])?;
                    *consumed += BLOCK_HEADER_SIZE;
                    h
                } else {
                    if !fill_stage(&mut self.stage, input, consumed, BLOCK_HEADER_SIZE) {
                        return Ok(false);
                    }
                    let h = parse_block_header(&self.stage)?;
                    self.stage.clear();
                    h
                };
                self.frame.check_block_header(header)?;
                let size = header.block_size as usize;
                let last = header.last_block;
                self.frame.start_block(match header.block_type {
                    BlockType::Compressed => 0,
                    _ => size,
                });
                self.output_pos = 0;
                self.output_end = 0;
                self.unit = match header.block_type {
                    BlockType::Raw => Unit::Raw { size, last },
                    BlockType::Rle => Unit::Rle { size, last },
                    BlockType::Compressed => Unit::Compressed { size, last },
                };
            }
            Unit::Raw { size, last } => {
                let have = self.frame.block().len();
                let n = (size - have).min(avail);
                self.frame.extend_block(&input[*consumed..*consumed + n]);
                *consumed += n;
                if have + n < size {
                    return Ok(false);
                }
                self.end_block(last)?;
            }
            Unit::Rle { size, last } => {
                if avail == 0 {
                    return Ok(false);
                }
                let byte = input[*consumed];
                *consumed += 1;
                self.frame.rle_block(byte, size);
                self.end_block(last)?;
            }
            Unit::Compressed { size, last } => {
                if self.stage.is_empty() && avail >= size {
                    self.frame
                        .decode_compressed(&input[*consumed..*consumed + size])?;
                    *consumed += size;
                } else {
                    if !fill_stage(&mut self.stage, input, consumed, size) {
                        return Ok(false);
                    }
                    self.frame.decode_compressed(&self.stage)?;
                    self.stage.clear();
                }
                self.end_block(last)?;
            }
            Unit::Checksum => {
                if self.stage.is_empty() && avail >= CHECKSUM_SIZE {
                    self.frame
                        .verify_checksum(&input[*consumed..*consumed + CHECKSUM_SIZE])?;
                    *consumed += CHECKSUM_SIZE;
                } else {
                    if !fill_stage(&mut self.stage, input, consumed, CHECKSUM_SIZE) {
                        return Ok(false);
                    }
                    self.frame.verify_checksum(&self.stage)?;
                    self.stage.clear();
                }
                self.unit = Unit::Magic;
                self.frame_done = true;
            }
            Unit::Failed => unreachable!("errors return before stepping"),
        }
        Ok(true)
    }

    fn end_block(&mut self, last: bool) -> Result<(), DecompressError> {
        self.unit = match self.frame.end_block(last)? {
            AfterBlock::Block => Unit::BlockHeader,
            AfterBlock::Checksum => Unit::Checksum,
            AfterBlock::FrameEnd => {
                self.frame_done = true;
                Unit::Magic
            }
        };
        self.output_pos = 0;
        self.output_end = self.frame.block().len();
        Ok(())
    }
}
