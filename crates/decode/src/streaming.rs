#![forbid(unsafe_code)]

use std::io::{self, Read};

use crate::BlockDecodeWorkspace;
use crate::literals::decode_literals_ws;
use crate::sequences::{SequenceDecodeTables, parse_sequence_count, parse_sequence_tables_ws};

use crate::exec::decode_execute_sequences;
use zrip_core::block::{BlockType, parse_block_header};
use zrip_core::dict::Dictionary;
use zrip_core::error::DecompressError;
use zrip_core::frame::header::parse_frame_header;
use zrip_core::frame::{MAX_BLOCK_SIZE, MAX_WINDOW_SIZE};
use zrip_core::fse::{promote_ll_table, promote_ml_table, promote_of_table};
use zrip_core::xxhash::Xxh64State;

#[cfg(all(
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "wasm32"
    ),
    not(feature = "paranoid")
))]
use zrip_core::simd::CpuTier;

enum State {
    FrameHeader,
    BlockHeader,
    BlockData {
        block_type: BlockType,
        block_size: usize,
        last: bool,
    },
    Checksum,
    Done,
}

/// Streaming zstd decompressor implementing [`Read`].
///
/// Wraps a reader of compressed data and yields decompressed bytes.
/// Supports multi-frame streams and skippable frames.
///
/// ```
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
    read_buf: Vec<u8>,
    output_buf: Vec<u8>,
    output_pos: usize,
    ws: Box<BlockDecodeWorkspace>,
    seq_tables: SequenceDecodeTables,
    rep_offsets: [u32; 3],
    hasher: Option<Xxh64State>,
    content_checksum: bool,
    max_output: usize,
    bytes_output: usize,
    frame_content_size: Option<u64>,
    frame_bytes: usize,
    dict: Option<Dictionary>,
    decode_history: Vec<u8>,
    window_size: usize,
}

impl<R: Read> FrameDecoder<R> {
    /// Creates a decoder with [`DEFAULT_DECOMPRESS_LIMIT`](zrip_core::DEFAULT_DECOMPRESS_LIMIT).
    pub fn new(reader: R) -> Self {
        Self::with_limit(reader, zrip_core::DEFAULT_DECOMPRESS_LIMIT)
    }

    /// Creates a decoder with an explicit output size limit.
    pub fn with_limit(reader: R, max_output: usize) -> Self {
        Self {
            inner: reader,
            state: State::FrameHeader,
            read_buf: Vec::new(),
            output_buf: Vec::new(),
            output_pos: 0,
            ws: Box::new(BlockDecodeWorkspace::new()),
            seq_tables: SequenceDecodeTables::new_default(),
            rep_offsets: [1, 4, 8],
            hasher: None,
            content_checksum: false,
            max_output,
            bytes_output: 0,
            frame_content_size: None,
            frame_bytes: 0,
            dict: None,
            decode_history: Vec::new(),
            window_size: 0,
        }
    }

    /// Creates a decoder with a dictionary and default output limit.
    pub fn with_dict(reader: R, dict: Dictionary) -> Self {
        Self::with_dict_and_limit(reader, dict, zrip_core::DEFAULT_DECOMPRESS_LIMIT)
    }

    /// Creates a decoder with a dictionary and explicit output limit.
    pub fn with_dict_and_limit(reader: R, dict: Dictionary, max_output: usize) -> Self {
        Self {
            inner: reader,
            state: State::FrameHeader,
            read_buf: Vec::new(),
            output_buf: Vec::new(),
            output_pos: 0,
            ws: Box::new(BlockDecodeWorkspace::new()),
            seq_tables: SequenceDecodeTables::new_default(),
            rep_offsets: [1, 4, 8],
            hasher: None,
            content_checksum: false,
            max_output,
            bytes_output: 0,
            frame_content_size: None,
            frame_bytes: 0,
            dict: Some(dict),
            decode_history: Vec::new(),
            window_size: 0,
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
        self.output_buf.clear();
        self.output_pos = 0;
        self.rep_offsets = [1, 4, 8];
        self.seq_tables = SequenceDecodeTables::new_default();
        self.ws.huf_valid = false;
        self.hasher = None;
        self.content_checksum = false;
        self.bytes_output = 0;
        self.frame_content_size = None;
        self.frame_bytes = 0;
        self.decode_history.clear();
        self.window_size = 0;
        old
    }

    fn fill_output(&mut self) -> io::Result<()> {
        loop {
            match self.state {
                State::Done => return Ok(()),
                State::FrameHeader => self.read_frame_header()?,
                State::BlockHeader => self.read_block_header()?,
                State::BlockData {
                    block_type,
                    block_size,
                    last,
                } => {
                    self.read_block_data(block_type, block_size, last)?;
                    if self.output_pos < self.output_buf.len() {
                        return Ok(());
                    }
                }
                State::Checksum => self.read_checksum()?,
            }
        }
    }

    fn read_frame_header(&mut self) -> io::Result<()> {
        self.read_buf.resize(18, 0);
        self.inner.read_exact(&mut self.read_buf[..5])?;

        let magic = u32::from_le_bytes([
            self.read_buf[0],
            self.read_buf[1],
            self.read_buf[2],
            self.read_buf[3],
        ]);

        if (magic & 0xFFFF_FFF0) == 0x184D_2A50 {
            self.inner.read_exact(&mut self.read_buf[5..9])?;
            let skip_size = u32::from_le_bytes([
                self.read_buf[5],
                self.read_buf[6],
                self.read_buf[7],
                self.read_buf[8],
            ]) as usize;
            io::copy(
                &mut self.inner.by_ref().take(skip_size as u64),
                &mut io::sink(),
            )?;
            return Ok(());
        }

        let descriptor = self.read_buf[4];
        let single_segment = (descriptor & 0x20) != 0;
        let dict_id_flag = descriptor & 0x03;
        let fcs_flag = (descriptor >> 6) & 0x03;

        let mut hdr_len = 5usize;
        if !single_segment {
            hdr_len += 1;
        }
        hdr_len += match dict_id_flag {
            0 => 0,
            1 => 1,
            2 => 2,
            3 => 4,
            _ => unreachable!(),
        };
        hdr_len += match fcs_flag {
            0 if single_segment => 1,
            0 => 0,
            1 => 2,
            2 => 4,
            3 => 8,
            _ => unreachable!(),
        };

        if hdr_len > 5 {
            self.inner.read_exact(&mut self.read_buf[5..hdr_len])?;
        }

        let header = parse_frame_header(&self.read_buf[..hdr_len])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        if let Some(frame_dict_id) = header.dict_id {
            match &self.dict {
                Some(d) if d.id() == frame_dict_id => {}
                Some(d) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        DecompressError::DictMismatch {
                            expected: frame_dict_id,
                            got: d.id(),
                        },
                    ));
                }
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        DecompressError::DictRequired,
                    ));
                }
            }
        }

        let window_size = if header.window_size > MAX_WINDOW_SIZE {
            if header.single_segment {
                MAX_WINDOW_SIZE as usize
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    DecompressError::WindowTooLarge {
                        requested: header.window_size,
                        max: MAX_WINDOW_SIZE,
                    },
                ));
            }
        } else {
            header.window_size as usize
        };

        if let Some(fcs) = header.frame_content_size {
            if fcs as usize > self.max_output {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    DecompressError::OutputTooSmall,
                ));
            }
        }

        self.window_size = window_size;
        self.decode_history.clear();
        self.frame_content_size = header.frame_content_size;
        self.frame_bytes = 0;
        self.content_checksum = header.content_checksum;
        self.hasher = if header.content_checksum {
            Some(Xxh64State::new(0))
        } else {
            None
        };

        if let Some(ref d) = self.dict {
            self.rep_offsets = *d.rep_offsets();
            self.decode_history.extend_from_slice(d.content());
            let mut st = SequenceDecodeTables::new_default();
            if let Some((t, l)) = d.of_table() {
                st.of_table = promote_of_table(t);
                st.of_accuracy = l;
                st.of_set = true;
            }
            if let Some((t, l)) = d.ml_table() {
                st.ml_table = promote_ml_table(t);
                st.ml_accuracy = l;
                st.ml_set = true;
            }
            if let Some((t, l)) = d.ll_table() {
                st.ll_table = promote_ll_table(t);
                st.ll_accuracy = l;
                st.ll_set = true;
            }
            self.seq_tables = st;
            self.ws.huf_valid = false;
            if let Some((t, l)) = d.huf_table() {
                self.ws.huf_table.clear();
                self.ws.huf_table.extend_from_slice(t);
                self.ws.huf_table_log = l;
                self.ws.huf_valid = true;
            }
        } else {
            self.rep_offsets = [1, 4, 8];
            self.seq_tables = SequenceDecodeTables::new_default();
            self.ws.huf_valid = false;
        }

        self.state = State::BlockHeader;
        Ok(())
    }

    fn read_block_header(&mut self) -> io::Result<()> {
        let mut hdr = [0u8; 3];
        self.inner.read_exact(&mut hdr)?;
        let block_header =
            parse_block_header(&hdr).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let block_size = block_header.block_size as usize;

        match block_header.block_type {
            BlockType::Raw | BlockType::Rle if block_size > MAX_BLOCK_SIZE => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    DecompressError::BlockTooLarge,
                ));
            }
            _ => {}
        }

        self.state = State::BlockData {
            block_type: block_header.block_type,
            block_size,
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
        self.output_buf.clear();
        self.output_pos = 0;

        match block_type {
            BlockType::Raw => {
                self.output_buf.resize(block_size, 0);
                self.inner.read_exact(&mut self.output_buf)?;
            }
            BlockType::Rle => {
                let mut byte = [0u8; 1];
                self.inner.read_exact(&mut byte)?;
                self.output_buf.resize(block_size, byte[0]);
            }
            BlockType::Compressed => {
                self.read_buf.resize(block_size, 0);
                self.inner.read_exact(&mut self.read_buf[..block_size])?;
                self.decode_compressed_block(block_size)?;
            }
        }

        if let Some(ref mut hasher) = self.hasher {
            hasher.update(&self.output_buf);
        }
        self.bytes_output += self.output_buf.len();
        self.frame_bytes += self.output_buf.len();
        if self.bytes_output > self.max_output {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                DecompressError::OutputTooSmall,
            ));
        }

        if self.window_size > 0 {
            self.decode_history.extend_from_slice(&self.output_buf);
            if self.decode_history.len() > self.window_size {
                let start = self.decode_history.len() - self.window_size;
                self.decode_history.copy_within(start.., 0);
                self.decode_history.truncate(self.window_size);
            }
        }

        self.state = if last {
            if let Some(fcs) = self.frame_content_size {
                if self.frame_bytes as u64 != fcs {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        DecompressError::FrameSizeMismatch,
                    ));
                }
            }
            if self.content_checksum {
                State::Checksum
            } else {
                State::FrameHeader
            }
        } else {
            State::BlockHeader
        };

        Ok(())
    }

    fn decode_compressed_block(&mut self, block_size: usize) -> io::Result<()> {
        let history: &[u8] = &self.decode_history;
        let block_data = &self.read_buf[..block_size];

        let lit_consumed = decode_literals_ws(block_data, &mut self.ws)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let remaining = &block_data[lit_consumed..];

        if remaining.is_empty() {
            self.output_buf.extend_from_slice(&self.ws.literal_buf);
            return Ok(());
        }

        let (num_sequences, seq_count_size) = parse_sequence_count(remaining)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        if num_sequences == 0 {
            self.output_buf.extend_from_slice(&self.ws.literal_buf);
            return Ok(());
        }

        let table_data = &remaining[seq_count_size..];
        let tables_consumed =
            parse_sequence_tables_ws(table_data, &mut self.seq_tables, &mut self.ws)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let seq_data = &table_data[tables_consumed..];

        #[cfg(all(target_arch = "x86_64", not(feature = "paranoid")))]
        {
            if zrip_core::simd::cpu_tier() >= CpuTier::Avx2 {
                let before = self.output_buf.len();
                crate::simd_decode::x86_64::decode::decode_execute_avx2_safe(
                    seq_data,
                    num_sequences,
                    &self.seq_tables,
                    &mut self.rep_offsets,
                    &self.ws.literal_buf,
                    &mut self.output_buf,
                    history,
                )
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                if self.output_buf.len() - before > MAX_BLOCK_SIZE {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        DecompressError::BlockTooLarge,
                    ));
                }
                return Ok(());
            }
        }

        #[cfg(all(target_arch = "aarch64", not(feature = "paranoid")))]
        {
            if zrip_core::simd::cpu_tier() >= CpuTier::Neon {
                let before = self.output_buf.len();
                crate::simd_decode::aarch64::decode::decode_execute_neon_safe(
                    seq_data,
                    num_sequences,
                    &self.seq_tables,
                    &mut self.rep_offsets,
                    &self.ws.literal_buf,
                    &mut self.output_buf,
                    history,
                )
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                if self.output_buf.len() - before > MAX_BLOCK_SIZE {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        DecompressError::BlockTooLarge,
                    ));
                }
                return Ok(());
            }
        }

        #[cfg(all(target_arch = "wasm32", not(feature = "paranoid")))]
        {
            if zrip_core::simd::cpu_tier() >= CpuTier::Wasm32Simd128 {
                let before = self.output_buf.len();
                crate::simd_decode::wasm32::decode::decode_execute_wasm32_safe(
                    seq_data,
                    num_sequences,
                    &self.seq_tables,
                    &mut self.rep_offsets,
                    &self.ws.literal_buf,
                    &mut self.output_buf,
                    history,
                )
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                if self.output_buf.len() - before > MAX_BLOCK_SIZE {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        DecompressError::BlockTooLarge,
                    ));
                }
                return Ok(());
            }
        }

        #[cfg(all(target_arch = "wasm32", not(feature = "paranoid")))]
        {
            if zrip_core::simd::cpu_tier() >= CpuTier::Wasm32Simd128 {
                let before = self.output_buf.len();
                crate::simd_decode::wasm32::decode::decode_execute_wasm32_safe(
                    seq_data,
                    num_sequences,
                    &self.seq_tables,
                    &mut self.rep_offsets,
                    &self.ws.literal_buf,
                    &mut self.output_buf,
                    dict_history,
                )
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                if self.output_buf.len() - before > MAX_BLOCK_SIZE {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        DecompressError::BlockTooLarge,
                    ));
                }
                return Ok(());
            }
        }

        let before = self.output_buf.len();
        decode_execute_sequences(
            seq_data,
            num_sequences,
            &self.seq_tables,
            &mut self.rep_offsets,
            &self.ws.literal_buf,
            &mut self.output_buf,
            history,
        )
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if self.output_buf.len() - before > MAX_BLOCK_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                DecompressError::BlockTooLarge,
            ));
        }
        Ok(())
    }

    fn read_checksum(&mut self) -> io::Result<()> {
        let mut buf = [0u8; 4];
        self.inner.read_exact(&mut buf)?;
        let stored = u32::from_le_bytes(buf);

        if let Some(ref hasher) = self.hasher {
            let hash = hasher.finish();
            let expected = (hash & 0xFFFF_FFFF) as u32;
            if expected != stored {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    DecompressError::ChecksumMismatch {
                        expected: stored,
                        got: expected,
                    },
                ));
            }
        }

        self.state = State::FrameHeader;
        Ok(())
    }
}

impl<R: Read> Read for FrameDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.output_pos >= self.output_buf.len() {
            if let State::Done = &self.state {
                return Ok(0);
            }

            self.output_buf.clear();
            self.output_pos = 0;

            match self.fill_output() {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => match &self.state {
                    State::FrameHeader => {
                        self.state = State::Done;
                        return Ok(0);
                    }
                    _ => return Err(e),
                },
                Err(e) => return Err(e),
            }
        }

        let available = &self.output_buf[self.output_pos..];
        let n = buf.len().min(available.len());
        buf[..n].copy_from_slice(&available[..n]);
        self.output_pos += n;
        Ok(n)
    }
}
