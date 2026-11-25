//! Streaming decoding through `std::io::Read`: [`FrameDecoder`].
#![forbid(unsafe_code)]

use std::io::{self, Read};

use crate::frame_state::{
    AfterBlock, BLOCK_HEADER_SIZE, CHECKSUM_SIZE, FRAME_PREFIX_SIZE, FrameState,
    MAX_FRAME_HEADER_SIZE, frame_header_size, is_skippable_magic,
};
use zrip_core::block::{BlockType, parse_block_header};
use zrip_core::dict::Dictionary;
use zrip_core::error::DecompressError;
use zrip_core::frame::ZSTD_MAGIC;

/// What [`FrameDecoder`] reads next.
enum State {
    /// A frame or skippable frame, or the end of input.
    FrameHeader,
    BlockHeader,
    BlockData {
        block_type: BlockType,
        block_size: usize,
        last: bool,
    },
    Checksum,
    /// The input ended on a frame boundary.
    Done,
    /// A read failed; only `reset` recovers.
    Failed,
}

/// Wraps a decode error as an `InvalidData` I/O error.
fn invalid(e: DecompressError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

/// Streaming zstd decompressor implementing [`Read`].
///
/// Wraps a reader of compressed data and yields decompressed bytes.
/// Supports multi-frame streams and skippable frames.
///
/// After a decoding or reader error, further nonempty reads return errors.
/// Call [`reset`](Self::reset) with a new reader to reuse the decoder.
///
/// ```no_run
/// use std::io::Read;
///
/// let data = b"hello, streaming world!".repeat(100);
/// let compressed = zrip::compress(&data, 1).unwrap();
///
/// let mut decoder = zrip::FrameDecoder::new(&compressed[..]);
/// let mut output = Vec::new();
/// decoder.read_to_end(&mut output).unwrap();
/// assert_eq!(output, data);
/// ```
pub struct FrameDecoder<R: Read> {
    inner: R,
    state: State,
    /// Frame header or compressed block bytes.
    read_buf: Vec<u8>,
    /// The current block's output in `frame.block()[output_pos..output_end]`
    /// is not read yet.
    output_pos: usize,
    output_end: usize,
    frame: FrameState,
}

impl<R: Read> FrameDecoder<R> {
    /// Creates a decoder with [`DEFAULT_DECOMPRESS_LIMIT`](zrip_core::DEFAULT_DECOMPRESS_LIMIT).
    pub fn new(reader: R) -> Self {
        Self::with_limit(reader, zrip_core::DEFAULT_DECOMPRESS_LIMIT)
    }

    /// Creates a decoder with an explicit output size limit.
    pub fn with_limit(reader: R, max_output: usize) -> Self {
        Self::from_state(reader, FrameState::new(None, max_output))
    }

    /// Creates a decoder with a dictionary and default output limit.
    pub fn with_dict(reader: R, dict: Dictionary) -> Self {
        Self::with_dict_and_limit(reader, dict, zrip_core::DEFAULT_DECOMPRESS_LIMIT)
    }

    /// Creates a decoder with a dictionary and explicit output limit.
    pub fn with_dict_and_limit(reader: R, dict: Dictionary, max_output: usize) -> Self {
        Self::from_state(reader, FrameState::new(Some(dict), max_output))
    }

    fn from_state(reader: R, frame: FrameState) -> Self {
        Self {
            inner: reader,
            state: State::FrameHeader,
            read_buf: Vec::new(),
            output_pos: 0,
            output_end: 0,
            frame,
        }
    }

    /// Consumes the decoder and returns the underlying reader.
    pub fn into_inner(self) -> R {
        self.inner
    }

    /// Installs a new reader for the next frame, keeping all internal
    /// buffers allocated. Returns the previous reader.
    pub fn reset(&mut self, new_reader: R) -> R {
        let old = core::mem::replace(&mut self.inner, new_reader);
        self.state = State::FrameHeader;
        self.output_pos = 0;
        self.output_end = 0;
        self.frame.reset();
        old
    }

    fn fill_output(&mut self) -> io::Result<()> {
        loop {
            match self.state {
                State::Done => return Ok(()),
                State::Failed => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "decoder must be reset after an error",
                    ));
                }
                State::FrameHeader => self.read_frame_header()?,
                State::BlockHeader => self.read_block_header()?,
                State::BlockData {
                    block_type,
                    block_size,
                    last,
                } => {
                    self.read_block_data(block_type, block_size, last)?;
                    if self.output_pos < self.output_end {
                        return Ok(());
                    }
                }
                State::Checksum => self.read_checksum()?,
            }
        }
    }

    fn read_frame_header(&mut self) -> io::Result<()> {
        self.read_buf.resize(MAX_FRAME_HEADER_SIZE, 0);
        // End of input is clean only before a frame's first byte; later
        // ones fail in `read_exact`.
        loop {
            match self.inner.read(&mut self.read_buf[..1]) {
                Ok(0) => {
                    self.state = State::Done;
                    return Ok(());
                }
                Ok(_) => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        self.inner.read_exact(&mut self.read_buf[1..4])?;

        let magic = u32::from_le_bytes([
            self.read_buf[0],
            self.read_buf[1],
            self.read_buf[2],
            self.read_buf[3],
        ]);

        if is_skippable_magic(magic) {
            self.inner.read_exact(&mut self.read_buf[4..8])?;
            let skip_size = u32::from_le_bytes([
                self.read_buf[4],
                self.read_buf[5],
                self.read_buf[6],
                self.read_buf[7],
            ]);
            let skipped = io::copy(
                &mut self.inner.by_ref().take(u64::from(skip_size)),
                &mut io::sink(),
            )?;
            if skipped != u64::from(skip_size) {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            return Ok(());
        }
        if magic != ZSTD_MAGIC {
            return Err(invalid(DecompressError::BadMagic));
        }

        self.inner
            .read_exact(&mut self.read_buf[4..FRAME_PREFIX_SIZE])?;
        let hdr_len = frame_header_size(&self.read_buf);
        if hdr_len > FRAME_PREFIX_SIZE {
            self.inner
                .read_exact(&mut self.read_buf[FRAME_PREFIX_SIZE..hdr_len])?;
        }

        self.frame
            .begin_frame(&self.read_buf[..hdr_len])
            .map_err(invalid)?;
        self.state = State::BlockHeader;
        Ok(())
    }

    fn read_block_header(&mut self) -> io::Result<()> {
        let mut hdr = [0u8; BLOCK_HEADER_SIZE];
        self.inner.read_exact(&mut hdr)?;
        let block_header = parse_block_header(&hdr).map_err(invalid)?;
        self.frame
            .check_block_header(block_header)
            .map_err(invalid)?;

        self.state = State::BlockData {
            block_type: block_header.block_type,
            block_size: block_header.block_size as usize,
            last: block_header.last_block,
        };
        Ok(())
    }

    fn read_block_data(
        &mut self,
        block_type: BlockType,
        block_size: usize,
        last: bool,
    ) -> io::Result<()> {
        self.output_pos = 0;
        self.output_end = 0;
        self.frame.start_block(match block_type {
            BlockType::Compressed => 0,
            _ => block_size,
        });

        match block_type {
            BlockType::Raw => {
                self.inner.read_exact(self.frame.raw_block(block_size))?;
            }
            BlockType::Rle => {
                let mut byte = [0u8; 1];
                self.inner.read_exact(&mut byte)?;
                self.frame.rle_block(byte[0], block_size);
            }
            BlockType::Compressed => {
                self.read_buf.resize(block_size, 0);
                self.inner.read_exact(&mut self.read_buf[..block_size])?;
                self.frame
                    .decode_compressed(&self.read_buf[..block_size])
                    .map_err(invalid)?;
            }
        }

        self.state = match self.frame.end_block(last).map_err(invalid)? {
            AfterBlock::Block => State::BlockHeader,
            AfterBlock::Checksum => State::Checksum,
            AfterBlock::FrameEnd => State::FrameHeader,
        };
        self.output_end = self.frame.block().len();
        Ok(())
    }

    fn read_checksum(&mut self) -> io::Result<()> {
        let mut buf = [0u8; CHECKSUM_SIZE];
        self.inner.read_exact(&mut buf)?;
        self.frame.verify_checksum(&buf).map_err(invalid)?;
        self.state = State::FrameHeader;
        Ok(())
    }
}

impl<R: Read> Read for FrameDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.output_pos >= self.output_end {
            if let State::Done = &self.state {
                return Ok(0);
            }

            self.output_pos = 0;
            self.output_end = 0;

            if let Err(e) = self.fill_output() {
                self.frame.discard_block();
                self.output_pos = 0;
                self.output_end = 0;
                self.state = State::Failed;
                return Err(e);
            }
        }

        let available = &self.frame.block()[self.output_pos..self.output_end];
        let n = buf.len().min(available.len());
        buf[..n].copy_from_slice(&available[..n]);
        self.output_pos += n;
        Ok(n)
    }
}
