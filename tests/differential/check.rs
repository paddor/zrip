//! Differential decoding against C zstd, shared by the `zrip_differential`
//! test and the `decode_differential` fuzz target.
//!
//! Every zrip decoder must accept exactly the inputs C zstd accepts, with the
//! same output: zrip's one-shot calls against `ZSTD_decompressDCtx`, and
//! `FrameDecoder` and `PushDecoder` against `ZSTD_decompressStream`. C zstd
//! runs without legacy format support, which zrip does not have.

use std::io::Read;
use zstd::zstd_safe;

/// Output cap for every decoder. Inputs that decode to more fail everywhere.
pub const LIMIT: usize = 1 << 20;

/// Output chunk size of the streaming decoders.
const CHUNK: usize = 4096;

/// Decodes `input` with every decoder and returns a description of the first
/// disagreement with C zstd.
pub fn check(input: &[u8], dict: Option<&[u8]>) -> Result<(), String> {
    let zdict = dict.map(|d| zrip::Dictionary::from_bytes(d).expect("test dictionary"));

    let lenient = c_lenient_huffman(input);
    let c = c_one_shot(input, dict);
    let z = match &zdict {
        Some(d) => {
            zrip::decompress_with_dict_and_limit(input, d, LIMIT).map_err(|e| format!("{e:?}"))
        }
        None => zrip::decompress_with_limit(input, LIMIT).map_err(|e| format!("{e:?}")),
    };
    let mut errors = Vec::new();
    errors.extend(compare("one-shot", &c, &z, lenient).err());

    let c = c_stream(input, dict);
    let z = frame_decoder(input, zdict.clone());
    errors.extend(compare("FrameDecoder", &c, &z, lenient).err());
    let z = push_decoder(input, zdict);
    errors.extend(compare("PushDecoder", &c, &z, lenient).err());
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn compare(
    what: &str,
    c: &Result<Vec<u8>, String>,
    z: &Result<Vec<u8>, String>,
    lenient_huffman: bool,
) -> Result<(), String> {
    match (c, z) {
        (Ok(c), Ok(z)) if c == z => Ok(()),
        (Ok(_), Err(e)) if lenient_huffman && is_huffman_stream_error(e) => Ok(()),
        (Ok(c), Ok(z)) => Err(format!(
            "{what}: output differs: C {} bytes, zrip {} bytes",
            c.len(),
            z.len()
        )),
        (Err(_), Err(_)) => Ok(()),
        (Ok(c), Err(e)) => Err(format!(
            "{what}: C accepts ({} bytes), zrip rejects: {e}",
            c.len()
        )),
        (Err(e), Ok(z)) => Err(format!(
            "{what}: zrip accepts ({} bytes), C rejects: {e}",
            z.len()
        )),
    }
}

/// Whether C zstd's one-shot decoder accepts `input`. Hand-built tests use
/// it to make sure their valid cases are valid.
#[allow(dead_code)]
pub fn c_accepts(input: &[u8]) -> bool {
    c_one_shot(input, None).is_ok()
}

/// zrip's error for a Huffman stream that does not end at its end mark.
fn is_huffman_stream_error(e: &str) -> bool {
    e == "BadHuffmanStream" || e.contains("corrupt Huffman stream")
}

/// Whether `input` has a 4-stream Huffman literals section that C zstd's
/// fast decoder takes: on 64-bit little-endian targets, a table of at most
/// 11 bits, every stream at least 8 bytes, and a nonempty fourth segment.
/// That decoder checks only how much each stream decodes, not where it
/// ends, and where it stops depends on the CPU. zrip rejects such streams.
fn c_lenient_huffman(input: &[u8]) -> bool {
    use zrip_core::block::{BlockType, parse_block_header};
    use zrip_core::frame::header::parse_frame_header;
    if !cfg!(all(target_pointer_width = "64", target_endian = "little")) {
        return false;
    }
    let mut pos = 0;
    while pos + 8 <= input.len() {
        let magic = u32::from_le_bytes(input[pos..pos + 4].try_into().unwrap());
        if magic & 0xFFFF_FFF0 == 0x184D_2A50 {
            let size = u32::from_le_bytes(input[pos + 4..pos + 8].try_into().unwrap());
            pos += 8 + size as usize;
            continue;
        }
        let Ok(header) = parse_frame_header(&input[pos..]) else {
            return false;
        };
        pos += header.header_size;
        while pos + 3 <= input.len() {
            let Ok(block) = parse_block_header(&input[pos..]) else {
                return false;
            };
            let size = block.block_size as usize;
            let body = &input[pos + 3..(pos + 3 + size).min(input.len())];
            if block.block_type == BlockType::Compressed && fast_huffman_section(body) {
                return true;
            }
            pos += 3 + if block.block_type == BlockType::Rle {
                1
            } else {
                size
            };
            if block.last_block {
                break;
            }
        }
        pos += if header.content_checksum { 4 } else { 0 };
    }
    false
}

/// Whether a compressed block body starts with literals that C zstd's fast
/// 4-stream Huffman decoder takes.
fn fast_huffman_section(body: &[u8]) -> bool {
    if body.len() < 5 {
        return false;
    }
    let kind = body[0] & 3;
    let format = (body[0] >> 2) & 3;
    if kind < 2 || format == 0 {
        return false;
    }
    let h = u64::from_le_bytes([body[0], body[1], body[2], body[3], body[4], 0, 0, 0]);
    let (header, regen, comp) = match format {
        1 => (3, (h >> 4) & 0x3FF, (h >> 14) & 0x3FF),
        2 => (4, (h >> 4) & 0x3FFF, (h >> 18) & 0x3FFF),
        _ => (5, (h >> 4) & 0x3FFFF, (h >> 22) & 0x3FFFF),
    };
    let (regen, comp) = (regen as usize, comp as usize);
    let Some(mut data) = body.get(header..header + comp) else {
        return false;
    };
    if kind == 2 {
        let Ok((weights, used)) = zrip_core::huffman::weights::parse_huffman_weights(data) else {
            return false;
        };
        let total: u32 = weights
            .iter()
            .filter(|&&w| w > 0)
            .map(|&w| 1 << (w - 1))
            .sum();
        if total == 0 || 32 - total.leading_zeros() > 11 {
            return false;
        }
        data = &data[used..];
    }
    if data.len() < 10 || 3 * regen.div_ceil(4) >= regen {
        return false;
    }
    let len = |i: usize| usize::from(u16::from_le_bytes([data[i], data[i + 1]]));
    let (l1, l2, l3) = (len(0), len(2), len(4));
    let Some(l4) = data.len().checked_sub(6 + l1 + l2 + l3) else {
        return false;
    };
    l1 >= 8 && l2 >= 8 && l3 >= 8 && l4 >= 8
}

fn c_one_shot(input: &[u8], dict: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let mut dctx = zstd_safe::DCtx::create();
    let mut out = Vec::with_capacity(LIMIT);
    let result = match dict {
        Some(d) => dctx.decompress_using_dict(&mut out, input, d),
        None => dctx.decompress(&mut out, input),
    };
    result
        .map(|_| out)
        .map_err(|code| zstd_safe::get_error_name(code).to_string())
}

/// `ZSTD_decompressStream` with `CHUNK`-byte output buffers, in its buffered
/// mode. Input that ends inside a frame fails.
///
/// Each frame's first byte comes alone. Given a whole frame and room for
/// its content, C zstd decodes it in one pass and skips its window limit
/// (`ZSTD_d_windowLogMax`, 128 MiB), so acceptance would depend on buffer
/// sizes.
fn c_stream(input: &[u8], dict: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let err = |code| zstd_safe::get_error_name(code).to_string();
    let mut dctx = zstd_safe::DCtx::create();
    if let Some(d) = dict {
        dctx.load_dictionary(d).map_err(err)?;
    }
    let mut out = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    let mut pos = 0;
    // 0 between frames: no frame is partly read.
    let mut hint = 0;
    loop {
        if pos == input.len() && hint == 0 {
            return Ok(out);
        }
        let end = if hint == 0 { pos + 1 } else { input.len() };
        let mut src = zstd_safe::InBuffer::around(&input[pos..end]);
        let mut dst = zstd_safe::OutBuffer::around(&mut buf[..]);
        hint = dctx.decompress_stream(&mut dst, &mut src).map_err(err)?;
        let consumed = src.pos();
        pos += consumed;
        let produced = dst.pos();
        out.extend_from_slice(&buf[..produced]);
        if out.len() > LIMIT {
            return Err("output over the limit".into());
        }
        if produced == 0 && consumed == 0 {
            return Err("truncated input".into());
        }
    }
}

fn frame_decoder(input: &[u8], dict: Option<zrip::Dictionary>) -> Result<Vec<u8>, String> {
    let mut decoder = match dict {
        Some(d) => zrip::FrameDecoder::with_dict_and_limit(input, d, LIMIT),
        None => zrip::FrameDecoder::with_limit(input, LIMIT),
    };
    let mut out = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        match decoder.read(&mut buf) {
            Ok(0) => return Ok(out),
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn push_decoder(input: &[u8], dict: Option<zrip::Dictionary>) -> Result<Vec<u8>, String> {
    use zrip::push::{PushDecoder, Status};
    let mut decoder = match dict {
        Some(d) => PushDecoder::with_dict_and_limit(d, LIMIT),
        None => PushDecoder::with_limit(LIMIT),
    };
    let mut out = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    let mut pos = 0;
    loop {
        let p = decoder
            .decode(&input[pos..], &mut buf)
            .map_err(|e| format!("{e:?}"))?;
        pos += p.consumed;
        out.extend_from_slice(&buf[..p.produced]);
        let done = pos == input.len() && p.status != Status::NeedsOutput;
        if done {
            decoder.finish().map_err(|e| format!("{e:?}"))?;
            return Ok(out);
        }
        if p.consumed == 0 && p.produced == 0 && p.status != Status::Finished {
            return Err("no progress".into());
        }
    }
}
