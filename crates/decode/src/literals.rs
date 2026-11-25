//! Literals section decoding: raw, RLE, and Huffman-coded literals, with
//! caches that skip rebuilding an unchanged Huffman table.
#![forbid(unsafe_code)]

use crate::BlockDecodeWorkspace;
use crate::fast_vec::{LITERAL_OVERREAD, Literals};
use zrip_core::error::DecompressError;
use zrip_core::huffman::decode::{
    build_pair_table, build_row_table, decode_4_streams_into, decode_4_streams_pairs_rows_into,
    decode_4_streams_rows_into, decode_single_stream_vec, prefers_pairs,
};
use zrip_core::huffman::weights::{build_huffman_decode_tables_into, parse_huffman_weights_into};
use zrip_core::huffman::{FAST_LOOP_ROWS, FAST_TABLE_LOG};

/// `Literals_Block_Type`, the low 2 bits of the section header.
#[derive(Debug, Clone, Copy)]
pub enum LiteralsBlockType {
    /// Stored as is.
    Raw,
    /// One byte, repeated.
    Rle,
    /// Huffman-coded, after a new tree description.
    Compressed,
    /// Huffman-coded with the previous tree.
    Treeless,
}

/// Parses a raw or RLE section header. Returns the literal count and the
/// header size.
fn parse_raw_rle_header(data: &[u8]) -> Result<(usize, usize), DecompressError> {
    let header_byte = data[0];
    // Size format in bits 3-2: x0 is a 5-bit count in 1 byte, 01 a 12-bit
    // count in 2 bytes, 11 a 20-bit count in 3 bytes.
    let size_format = (header_byte >> 2) & 0x03;
    match size_format {
        0 | 2 => Ok(((header_byte >> 3) as usize, 1)),
        1 => {
            if data.len() < 2 {
                return Err(DecompressError::CorruptLiterals);
            }
            Ok((((header_byte >> 4) as usize) | ((data[1] as usize) << 4), 2))
        }
        3 => {
            if data.len() < 3 {
                return Err(DecompressError::CorruptLiterals);
            }
            Ok((
                ((header_byte >> 4) as usize)
                    | ((data[1] as usize) << 4)
                    | ((data[2] as usize) << 12),
                3,
            ))
        }
        _ => unreachable!(),
    }
}

/// Parses a compressed or treeless section header. Returns the literal
/// count, the compressed size, the stream count and the header size.
fn parse_compressed_header(data: &[u8]) -> Result<(usize, usize, usize, usize), DecompressError> {
    let header_byte = data[0];
    // Size format in bits 3-2: 0 is one stream, others four. Both sizes are
    // 10 bits for formats 0 and 1, 14 bits for 2, and 18 bits for 3.
    let size_format = (header_byte >> 2) & 0x03;
    match size_format {
        0 => {
            if data.len() < 3 {
                return Err(DecompressError::CorruptLiterals);
            }
            let both = (header_byte as usize >> 4)
                | ((data[1] as usize) << 4)
                | ((data[2] as usize) << 12);
            Ok((both & 0x3FF, both >> 10, 1, 3))
        }
        1 => {
            if data.len() < 3 {
                return Err(DecompressError::CorruptLiterals);
            }
            let both = (header_byte as usize >> 4)
                | ((data[1] as usize) << 4)
                | ((data[2] as usize) << 12);
            Ok((both & 0x3FF, both >> 10, 4, 3))
        }
        2 => {
            if data.len() < 4 {
                return Err(DecompressError::CorruptLiterals);
            }
            let both = (header_byte as usize >> 4)
                | ((data[1] as usize) << 4)
                | ((data[2] as usize) << 12)
                | ((data[3] as usize) << 20);
            Ok((both & 0x3FFF, both >> 14, 4, 4))
        }
        3 => {
            if data.len() < 5 {
                return Err(DecompressError::CorruptLiterals);
            }
            let both = (header_byte as u64 >> 4)
                | ((data[1] as u64) << 4)
                | ((data[2] as u64) << 12)
                | ((data[3] as u64) << 20)
                | ((data[4] as u64) << 28);
            Ok(((both & 0x3FFFF) as usize, (both >> 18) as usize, 4, 5))
        }
        _ => unreachable!(),
    }
}

/// Where [`decode_literals_ws`] left a block's literals.
#[derive(Clone, Copy, Debug)]
pub(crate) enum LiteralsAt {
    /// The first `len` bytes of `ws.literal_buf`, followed by
    /// [`LITERAL_OVERREAD`] zero bytes.
    Buffer(usize),
    /// Raw literals in place: `len` bytes from `start` of the block, which
    /// holds at least [`LITERAL_OVERREAD`] more bytes after them.
    Block { start: usize, len: usize },
}

impl LiteralsAt {
    /// The literals, given the block they were decoded from and the
    /// workspace's literal buffer.
    pub(crate) fn resolve<'a>(self, data: &'a [u8], buf: &'a [u8]) -> Literals<'a> {
        match self {
            Self::Buffer(len) => Literals::new(buf, len),
            Self::Block { start, len } => Literals::new(&data[start..], len),
        }
    }

    /// How many literals the block has.
    pub(crate) fn len(self) -> usize {
        match self {
            Self::Buffer(len) | Self::Block { len, .. } => len,
        }
    }
}

/// Decodes the literals section at the start of `data`, returning its size
/// and where the literals are. Raw literals stay in the block when it has
/// enough bytes after them for the executor's chunked copies; all others go
/// to `ws.literal_buf` with that much zeroed slack.
///
/// More than `block_size_max` literals, the frame's `Block_Maximum_Size`,
/// fail, as in C zstd.
///
/// Compiled for x86-64-v3 when the CPU has it, and then kept out of line, so
/// the
/// table builds and decode loops use BMI2 and LZCNT in builds for the
/// baseline target, as C zstd's do.
#[cfg_attr(
    all(
        feature = "std",
        feature = "simd",
        target_arch = "x86_64",
        not(target_feature = "avx2")
    ),
    inline(never)
)]
pub(crate) fn decode_literals_ws(
    data: &[u8],
    ws: &mut BlockDecodeWorkspace,
    block_size_max: usize,
) -> Result<(usize, LiteralsAt), DecompressError> {
    simd_body!(decode_literals_ws_impl(data, ws, block_size_max))
}

#[inline(always)]
fn decode_literals_ws_impl(
    data: &[u8],
    ws: &mut BlockDecodeWorkspace,
    block_size_max: usize,
) -> Result<(usize, LiteralsAt), DecompressError> {
    if data.is_empty() {
        return Err(DecompressError::CorruptLiterals);
    }

    let block_type = match data[0] & 0x03 {
        0 => LiteralsBlockType::Raw,
        1 => LiteralsBlockType::Rle,
        2 => LiteralsBlockType::Compressed,
        3 => LiteralsBlockType::Treeless,
        _ => unreachable!(),
    };

    let (consumed, at) = match block_type {
        LiteralsBlockType::Raw => {
            let (regen_size, header_size) = parse_raw_rle_header(data)?;
            if regen_size > block_size_max {
                return Err(DecompressError::CorruptLiterals);
            }
            if data.len() < header_size + regen_size {
                return Err(DecompressError::CorruptLiterals);
            }
            let end = header_size + regen_size;
            if data.len() - end >= LITERAL_OVERREAD {
                (
                    end,
                    LiteralsAt::Block {
                        start: header_size,
                        len: regen_size,
                    },
                )
            } else {
                ws.literal_buf.clear();
                ws.literal_buf.extend_from_slice(&data[header_size..end]);
                ws.literal_buf.resize(regen_size + LITERAL_OVERREAD, 0);
                (end, LiteralsAt::Buffer(regen_size))
            }
        }
        LiteralsBlockType::Rle => {
            let (regen_size, header_size) = parse_raw_rle_header(data)?;
            if regen_size > block_size_max {
                return Err(DecompressError::CorruptLiterals);
            }
            if data.len() < header_size + 1 {
                return Err(DecompressError::CorruptLiterals);
            }
            let byte = data[header_size];
            ws.literal_buf.clear();
            ws.literal_buf.resize(regen_size, byte);
            ws.literal_buf.resize(regen_size + LITERAL_OVERREAD, 0);
            (header_size + 1, LiteralsAt::Buffer(regen_size))
        }
        LiteralsBlockType::Compressed | LiteralsBlockType::Treeless => {
            let treeless = matches!(block_type, LiteralsBlockType::Treeless);
            let (regen_size, compressed_size, num_streams, header_size) =
                parse_compressed_header(data)?;

            if regen_size > block_size_max {
                return Err(DecompressError::CorruptLiterals);
            }
            // As C zstd (`MIN_LITERALS_FOR_4_STREAMS`): four streams need six
            // literals. With six or nine, the fourth stream is empty.
            if num_streams == 4 && regen_size < 6 {
                return Err(DecompressError::CorruptLiterals);
            }
            if data.len() < header_size + compressed_size {
                return Err(DecompressError::CorruptLiterals);
            }

            let stream_data = &data[header_size..header_size + compressed_size];

            // Treeless reuses the last table of the frame or its dictionary.
            // A tree description equal to the last one reuses its table
            // without parsing, and equal weights reuse it without a rebuild.
            let huf_consumed = if treeless {
                if !ws.huf_valid {
                    return Err(DecompressError::CorruptLiterals);
                }
                0
            } else if ws.huf_last_header_valid
                && ws.huf_last_weights_valid
                && stream_data.len() >= ws.huf_last_header.len()
                && stream_data[..ws.huf_last_header.len()] == ws.huf_last_header
            {
                ws.huf_valid = true;
                ws.huf_last_header.len()
            } else {
                let consumed = parse_huffman_weights_into(
                    stream_data,
                    &mut ws.huf_weights,
                    &mut ws.fse_build_buf,
                    &mut ws.fse_dist,
                )?;
                let can_reuse = ws.huf_last_weights_valid && ws.huf_weights == ws.huf_last_weights;
                if !can_reuse {
                    ws.huf_is_dict = false;
                    // Four streams read the fast loop's rows, where it reads
                    // rows, and the build stores them with the table.
                    let rows = FAST_LOOP_ROWS && num_streams == 4;
                    ws.huf_table_log = build_huffman_decode_tables_into(
                        &ws.huf_weights,
                        &mut ws.huf_table,
                        rows.then_some(&mut ws.huf_rows),
                        &mut ws.huf_all_weights,
                    )?;
                    ws.huf_pairs_valid = false;
                    ws.huf_rows_valid = rows;
                    ws.huf_last_weights.clear();
                    ws.huf_last_weights.extend_from_slice(&ws.huf_weights);
                    ws.huf_last_weights_valid = true;
                }
                ws.huf_last_header.clear();
                ws.huf_last_header
                    .extend_from_slice(&stream_data[..consumed]);
                ws.huf_last_header_valid = true;
                ws.huf_valid = true;
                consumed
            };

            let compressed_stream = &stream_data[huf_consumed..];

            decode_huffman_streams(ws, num_streams, compressed_stream, regen_size)?;

            ws.literal_buf.resize(regen_size + LITERAL_OVERREAD, 0);
            (
                header_size + compressed_size,
                LiteralsAt::Buffer(regen_size),
            )
        }
    };

    Ok((consumed, at))
}

/// Decodes a section's Huffman streams into `ws.literal_buf` with the table
/// in `ws`.
#[inline(always)]
fn decode_huffman_streams(
    ws: &mut BlockDecodeWorkspace,
    num_streams: usize,
    compressed_stream: &[u8],
    regen_size: usize,
) -> Result<(), DecompressError> {
    if num_streams == 1 {
        decode_single_stream_vec(
            &ws.huf_table,
            ws.huf_table_log,
            compressed_stream,
            regen_size,
            &mut ws.literal_buf,
        )?;
    } else if ws.huf_table_log <= FAST_TABLE_LOG {
        // Tables from a dictionary, or built for one stream, get their rows
        // here, once.
        if FAST_LOOP_ROWS && !ws.huf_rows_valid {
            build_row_table(&ws.huf_table, ws.huf_table_log, &mut ws.huf_rows)?;
            ws.huf_rows_valid = true;
        }
        if prefers_pairs(regen_size, compressed_stream.len()) {
            // Two symbols per lookup; the pair table is built lazily.
            if !ws.huf_pairs_valid {
                build_pair_table(&ws.huf_table, ws.huf_table_log, &mut ws.huf_pairs)?;
                ws.huf_pairs_valid = true;
            }
            decode_4_streams_pairs_rows_into(
                &ws.huf_table,
                ws.huf_table_log,
                &ws.huf_pairs,
                &ws.huf_rows,
                compressed_stream,
                regen_size,
                &mut ws.literal_buf,
            )?;
        } else {
            decode_4_streams_rows_into(
                &ws.huf_table,
                ws.huf_table_log,
                &ws.huf_rows,
                compressed_stream,
                regen_size,
                &mut ws.literal_buf,
            )?;
        }
    } else {
        decode_4_streams_into(
            &ws.huf_table,
            ws.huf_table_log,
            compressed_stream,
            regen_size,
            &mut ws.literal_buf,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A raw literals section of `len` bytes (two-byte header), followed by
    /// `tail` more block bytes.
    fn raw_section(len: usize, tail: usize) -> Vec<u8> {
        let mut data = vec![0b0100 | ((len as u8 & 0x0F) << 4), (len >> 4) as u8];
        data.extend((0..len).map(|i| i as u8 ^ 0x5A));
        data.extend(core::iter::repeat_n(0xEE, tail));
        data
    }

    #[test]
    fn raw_literals_stay_in_the_block_when_slack_follows() {
        let mut ws = BlockDecodeWorkspace::new();
        let data = raw_section(40, LITERAL_OVERREAD);
        let (consumed, at) =
            decode_literals_ws(&data, &mut ws, zrip_core::frame::MAX_BLOCK_SIZE).unwrap();
        assert_eq!(consumed, 42);
        assert!(matches!(at, LiteralsAt::Block { start: 2, len: 40 }));
        let lits = at.resolve(&data, &ws.literal_buf);
        assert_eq!(lits.as_slice(), &data[2..42]);
    }

    #[test]
    fn literals_without_block_slack_are_copied_with_zeroed_slack() {
        let mut ws = BlockDecodeWorkspace::new();
        let data = raw_section(40, LITERAL_OVERREAD - 1);
        let (_, at) = decode_literals_ws(&data, &mut ws, zrip_core::frame::MAX_BLOCK_SIZE).unwrap();
        assert!(matches!(at, LiteralsAt::Buffer(40)));
        assert_eq!(ws.literal_buf.len(), 40 + LITERAL_OVERREAD);
        assert!(ws.literal_buf[40..].iter().all(|&b| b == 0));
        let lits = at.resolve(&data, &ws.literal_buf);
        assert_eq!(lits.as_slice(), &data[2..42]);

        // RLE literals get the same slack.
        let rle = [0b0001 | (5 << 3), b'q'];
        let (consumed, at) =
            decode_literals_ws(&rle, &mut ws, zrip_core::frame::MAX_BLOCK_SIZE).unwrap();
        assert_eq!(consumed, 2);
        assert_eq!(at.resolve(&rle, &ws.literal_buf).as_slice(), b"qqqqq");
        assert_eq!(ws.literal_buf.len(), 5 + LITERAL_OVERREAD);
    }
}
