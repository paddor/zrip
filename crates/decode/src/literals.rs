#![forbid(unsafe_code)]

use crate::BlockDecodeWorkspace;
use zrip_core::error::DecompressError;
use zrip_core::huffman::decode::{decode_4_streams_into, decode_single_stream_vec};
use zrip_core::huffman::weights::{build_huffman_decode_table_into, parse_huffman_weights};

#[derive(Debug, Clone, Copy)]
pub enum LiteralsBlockType {
    Raw,
    Rle,
    Compressed,
    Treeless,
}

fn parse_raw_rle_header(data: &[u8]) -> Result<(usize, usize), DecompressError> {
    let header_byte = data[0];
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

fn parse_compressed_header(data: &[u8]) -> Result<(usize, usize, usize, usize), DecompressError> {
    let header_byte = data[0];
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

pub(crate) fn decode_literals_ws(
    data: &[u8],
    ws: &mut BlockDecodeWorkspace,
) -> Result<usize, DecompressError> {
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

    let consumed = match block_type {
        LiteralsBlockType::Raw => {
            let (regen_size, header_size) = parse_raw_rle_header(data)?;
            if regen_size > zrip_core::frame::MAX_BLOCK_SIZE {
                return Err(DecompressError::CorruptLiterals);
            }
            if data.len() < header_size + regen_size {
                return Err(DecompressError::CorruptLiterals);
            }
            ws.literal_buf.clear();
            ws.literal_buf
                .extend_from_slice(&data[header_size..header_size + regen_size]);
            header_size + regen_size
        }
        LiteralsBlockType::Rle => {
            let (regen_size, header_size) = parse_raw_rle_header(data)?;
            if regen_size > zrip_core::frame::MAX_BLOCK_SIZE {
                return Err(DecompressError::CorruptLiterals);
            }
            if data.len() < header_size + 1 {
                return Err(DecompressError::CorruptLiterals);
            }
            let byte = data[header_size];
            ws.literal_buf.clear();
            ws.literal_buf.resize(regen_size, byte);
            header_size + 1
        }
        LiteralsBlockType::Compressed | LiteralsBlockType::Treeless => {
            let treeless = matches!(block_type, LiteralsBlockType::Treeless);
            let (regen_size, compressed_size, num_streams, header_size) =
                parse_compressed_header(data)?;

            if regen_size > zrip_core::frame::MAX_BLOCK_SIZE {
                return Err(DecompressError::CorruptLiterals);
            }
            if data.len() < header_size + compressed_size {
                return Err(DecompressError::CorruptLiterals);
            }

            let stream_data = &data[header_size..header_size + compressed_size];

            let huf_consumed = if treeless {
                if !ws.huf_valid {
                    return Err(DecompressError::CorruptLiterals);
                }
                0
            } else {
                let (weights, consumed) = parse_huffman_weights(stream_data)?;
                ws.huf_table_log = build_huffman_decode_table_into(
                    &weights,
                    &mut ws.huf_table,
                    &mut ws.huf_all_weights,
                    &mut ws.huf_rank_count,
                    &mut ws.huf_rank_start,
                )?;
                ws.huf_valid = true;
                consumed
            };

            let compressed_stream = &stream_data[huf_consumed..];

            if num_streams == 1 {
                decode_single_stream_vec(
                    &ws.huf_table,
                    ws.huf_table_log,
                    compressed_stream,
                    regen_size,
                    &mut ws.literal_buf,
                )?;
            } else {
                decode_4_streams_into(
                    &ws.huf_table,
                    ws.huf_table_log,
                    compressed_stream,
                    regen_size,
                    &mut ws.literal_buf,
                )?;
            }

            header_size + compressed_size
        }
    };

    // Ensure 32 bytes of initialized padding past literal data for
    // unconditional SIMD loads in the sequence decode loop.
    let real_len = ws.literal_buf.len();
    ws.literal_buf.resize(real_len + 32, 0);
    ws.literal_buf.truncate(real_len);

    Ok(consumed)
}
