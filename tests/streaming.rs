#![cfg(feature = "std")]
// Streaming encoder/decoder tests. Pure zrip (no C zstd).

#[cfg(not(miri))]
const BLOCK: usize = zrip::frame::MAX_BLOCK_SIZE; // 128 KiB
#[cfg(not(miri))]
const KNUTH: u32 = 0x9E37_79B1; // xxHash PRIME32_1, used as cheap pseudo-random scatter

#[cfg(not(miri))]
#[test]
fn streaming_encoder_basic() {
    let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(10000).copied().collect();
    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    std::io::Write::write_all(&mut encoder, &original).unwrap();
    let compressed = encoder.finish().unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, original);
}

#[cfg(not(miri))]
#[test]
fn streaming_encoder_chunked_writes() {
    let original: Vec<u8> = b"the quick brown fox jumps over the lazy dog. "
        .iter()
        .cycle()
        .take(50_000)
        .copied()
        .collect();
    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 3).unwrap();
    for chunk in original.chunks(1337) {
        std::io::Write::write_all(&mut encoder, chunk).unwrap();
    }
    let compressed = encoder.finish().unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn streaming_encoder_empty() {
    let encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    let compressed = encoder.finish().unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, b"");
}

#[cfg(not(miri))]
#[test]
fn streaming_encoder_all_levels() {
    let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(5000).copied().collect();
    for level in [-7, -5, -3, -1, 1, 2, 3, 4] {
        let mut encoder = zrip::FrameEncoder::new(Vec::new(), level).unwrap();
        std::io::Write::write_all(&mut encoder, &original).unwrap();
        let compressed = encoder.finish().unwrap();
        let decompressed = zrip::decompress(&compressed)
            .unwrap_or_else(|e| panic!("level {level}: decompress: {e}"));
        assert_eq!(decompressed, original, "level {level}");
    }
}

// ===== Streaming decoder (FrameDecoder) =====

#[test]
fn frame_decoder_basic() {
    use std::io::Read;
    let data = b"Hello, streaming decoder!".repeat(1000);
    let compressed = zrip::compress(&data, 1).unwrap();
    let mut decoder = zrip::FrameDecoder::new(&compressed[..]);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output).unwrap();
    assert_eq!(output, data);
}

#[cfg(not(miri))]
#[test]
fn frame_decoder_small_reads() {
    use std::io::Read;
    let data: Vec<u8> = (0..100_000).map(|i| (i % 251) as u8).collect();
    let compressed = zrip::compress(&data, 1).unwrap();
    let mut decoder = zrip::FrameDecoder::new(&compressed[..]);
    let mut output = Vec::new();
    let mut buf = [0u8; 37];
    loop {
        let n = decoder.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        output.extend_from_slice(&buf[..n]);
    }
    assert_eq!(output, data);
}

#[cfg(not(miri))]
#[test]
fn frame_decoder_multiframe() {
    use std::io::Read;
    let d1 = b"first frame data".repeat(500);
    let d2 = b"second frame".repeat(500);
    let c1 = zrip::compress(&d1, 1).unwrap();
    let c2 = zrip::compress(&d2, 3).unwrap();
    let mut stream = c1.clone();
    stream.extend_from_slice(&c2);
    let mut decoder = zrip::FrameDecoder::new(&stream[..]);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output).unwrap();
    let mut expected = d1.clone();
    expected.extend_from_slice(&d2);
    assert_eq!(output, expected);
}

#[cfg(not(miri))]
#[test]
fn frame_decoder_with_checksum() {
    use std::io::Read;
    let data = b"checksum test".repeat(2000);
    let compressed = zrip::compress(&data, 1).unwrap();
    let mut decoder = zrip::FrameDecoder::new(&compressed[..]);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output).unwrap();
    assert_eq!(output, data);
}

#[cfg(not(miri))]
#[test]
fn frame_decoder_with_limit_rejects_oversized() {
    use std::io::Read;
    let data = b"limit test data".repeat(1000);
    let compressed = zrip::compress(&data, 1).unwrap();
    let mut decoder = zrip::FrameDecoder::with_limit(&compressed[..], 100);
    let mut output = Vec::new();
    let err = decoder.read_to_end(&mut output).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[cfg(not(miri))]
#[test]
fn frame_decoder_with_limit_allows_within_limit() {
    use std::io::Read;
    let data = b"limit test data".repeat(1000);
    let compressed = zrip::compress(&data, 1).unwrap();
    let mut decoder = zrip::FrameDecoder::with_limit(&compressed[..], data.len() + 1024);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output).unwrap();
    assert_eq!(output, data);
}

// ===== Encoder reset =====

#[test]
fn streaming_encoder_reset_reuses_buffers() {
    use std::io::Write;
    let data1 = b"first frame data repeated enough to compress well ".repeat(50);
    let data2 = b"second frame different content also repeated lots ".repeat(50);

    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    encoder.write_all(&data1).unwrap();
    let out1 = encoder.reset(Vec::new()).unwrap();

    encoder.write_all(&data2).unwrap();
    let out2 = encoder.finish().unwrap();

    assert_eq!(zrip::decompress(&out1).unwrap(), data1);
    assert_eq!(zrip::decompress(&out2).unwrap(), data2);
}

#[test]
fn streaming_encoder_reset_empty_frame() {
    use std::io::Write;
    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    let out1 = encoder.reset(Vec::new()).unwrap();
    assert_eq!(zrip::decompress(&out1).unwrap(), b"");

    encoder.write_all(b"after empty").unwrap();
    let out2 = encoder.finish().unwrap();
    assert_eq!(zrip::decompress(&out2).unwrap(), b"after empty");
}

#[cfg(not(miri))]
#[test]
fn streaming_encoder_reset_multiple_nodict() {
    use std::io::Write;
    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 3).unwrap();
    for i in 0..5 {
        let data = format!("frame {i} with some repeated content to compress ").repeat(20);
        encoder.write_all(data.as_bytes()).unwrap();
        let out = encoder.reset(Vec::new()).unwrap();
        assert_eq!(zrip::decompress(&out).unwrap(), data.as_bytes());
    }
    encoder.write_all(b"final").unwrap();
    let out = encoder.finish().unwrap();
    assert_eq!(zrip::decompress(&out).unwrap(), b"final");
}

// ===== Decoder reset =====

#[test]
fn streaming_decoder_reset() {
    use std::io::Read;
    let data1 = b"decoder reset test frame one ".repeat(100);
    let data2 = b"decoder reset test frame two ".repeat(100);
    let c1 = zrip::compress(&data1, 1).unwrap();
    let c2 = zrip::compress(&data2, 1).unwrap();

    let mut decoder = zrip::FrameDecoder::new(c1.as_slice());
    let mut out1 = Vec::new();
    decoder.read_to_end(&mut out1).unwrap();
    assert_eq!(out1, data1);

    decoder.reset(c2.as_slice());
    let mut out2 = Vec::new();
    decoder.read_to_end(&mut out2).unwrap();
    assert_eq!(out2, data2);
}

#[test]
fn streaming_decoder_with_dict_on_nodict_frame() {
    #[cfg(feature = "dict_builder")]
    {
        use std::io::Read;
        let samples: Vec<Vec<u8>> = (0..50)
            .map(|i| format!("sample data item {i} with some repeated content").into_bytes())
            .collect();
        let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
        let dict = zrip::dict::train_dict_fastcover(
            &sample_refs,
            4096,
            zrip::dict::fastcover::FastCoverParams::default(),
        );

        let data = b"plain frame with no dictionary".repeat(10);
        let compressed = zrip::compress(&data, 1).unwrap();

        let mut decoder = zrip::FrameDecoder::with_dict(compressed.as_slice(), dict);
        let mut out = Vec::new();
        decoder.read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
    }
}

// ===== Streaming encoder edge cases =====

#[test]
fn streaming_encoder_single_byte() {
    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    std::io::Write::write_all(&mut encoder, b"x").unwrap();
    let compressed = encoder.finish().unwrap();
    assert_eq!(zrip::decompress(&compressed).unwrap(), b"x");
}

#[cfg(not(miri))]
#[test]
fn streaming_encoder_exact_one_block() {
    use std::io::Write;
    let data: Vec<u8> = b"exact block "
        .iter()
        .cycle()
        .take(BLOCK)
        .copied()
        .collect();
    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    encoder.write_all(&data).unwrap();
    let compressed = encoder.finish().unwrap();
    assert_eq!(zrip::decompress(&compressed).unwrap(), data);
}

#[cfg(not(miri))]
#[test]
fn streaming_encoder_exact_two_blocks() {
    use std::io::Write;
    let data: Vec<u8> = b"two blocks! "
        .iter()
        .cycle()
        .take(2 * BLOCK)
        .copied()
        .collect();
    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 3).unwrap();
    encoder.write_all(&data).unwrap();
    let compressed = encoder.finish().unwrap();
    assert_eq!(zrip::decompress(&compressed).unwrap(), data);
}

#[cfg(not(miri))]
#[test]
fn streaming_encoder_byte_at_a_time() {
    use std::io::Write;
    let data: Vec<u8> = b"byte at a time "
        .iter()
        .cycle()
        .take(5000)
        .copied()
        .collect();
    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    for &b in &data {
        encoder.write_all(&[b]).unwrap();
    }
    let compressed = encoder.finish().unwrap();
    assert_eq!(zrip::decompress(&compressed).unwrap(), data);
}

#[cfg(not(miri))]
#[test]
fn streaming_encoder_flush_mid_stream() {
    use std::io::Write;
    let part1 = b"first part of data ".repeat(200);
    let part2 = b"second part after flush ".repeat(200);
    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    encoder.write_all(&part1).unwrap();
    encoder.flush().unwrap();
    encoder.write_all(&part2).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut expected = part1.clone();
    expected.extend_from_slice(&part2);
    assert_eq!(zrip::decompress(&compressed).unwrap(), expected);
}

#[cfg(not(miri))]
#[test]
fn streaming_encoder_triggers_compaction() {
    use std::io::Write;
    let window_size = 1usize << 19;
    let total = window_size * 3;
    let data: Vec<u8> = b"compaction trigger test! "
        .iter()
        .cycle()
        .take(total)
        .copied()
        .collect();
    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    encoder.write_all(&data).unwrap();
    let compressed = encoder.finish().unwrap();
    assert_eq!(zrip::decompress(&compressed).unwrap(), data);
}

#[cfg(not(miri))]
#[test]
fn streaming_encoder_compaction_all_levels() {
    use std::io::Write;
    let data: Vec<u8> = b"compaction all levels "
        .iter()
        .cycle()
        .take(6 * BLOCK)
        .copied()
        .collect();
    for level in [-7, -1, 1, 3, 4] {
        let mut encoder = zrip::FrameEncoder::new(Vec::new(), level).unwrap();
        encoder.write_all(&data).unwrap();
        let compressed = encoder.finish().unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("L{level}: {e}"));
        assert_eq!(decompressed, data, "L{level}");
    }
}

#[cfg(not(miri))]
#[test]
fn streaming_roundtrip_parity_with_singleshot() {
    use std::io::Write;
    let data: Vec<u8> = b"parity check streaming vs singleshot "
        .iter()
        .cycle()
        .take(400_000)
        .copied()
        .collect();
    for level in [1, 3, 4] {
        let singleshot = zrip::compress(&data, level).unwrap();
        let mut encoder = zrip::FrameEncoder::new(Vec::new(), level).unwrap();
        encoder.write_all(&data).unwrap();
        let streaming = encoder.finish().unwrap();
        let ss_ratio = data.len() as f64 / singleshot.len() as f64;
        let st_ratio = data.len() as f64 / streaming.len() as f64;
        let diff = (ss_ratio - st_ratio).abs() / ss_ratio;
        assert!(
            diff < 0.05,
            "L{level}: ratio parity: singleshot={ss_ratio:.3}x streaming={st_ratio:.3}x diff={:.1}%",
            diff * 100.0
        );
    }
}

// ===== Streaming decoder edge cases =====

#[test]
fn frame_decoder_single_byte() {
    use std::io::Read;
    let compressed = zrip::compress(b"y", 1).unwrap();
    let mut decoder = zrip::FrameDecoder::new(&compressed[..]);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output).unwrap();
    assert_eq!(output, b"y");
}

#[test]
fn frame_decoder_empty() {
    use std::io::Read;
    let compressed = zrip::compress(b"", 1).unwrap();
    let mut decoder = zrip::FrameDecoder::new(&compressed[..]);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output).unwrap();
    assert_eq!(output, b"");
}

#[cfg(not(miri))]
#[test]
fn frame_decoder_multiblock_all_levels() {
    use std::io::Read;
    let data: Vec<u8> = b"multiblock decode all levels "
        .iter()
        .cycle()
        .take(300_000)
        .copied()
        .collect();
    for level in [-7, -1, 1, 2, 3, 4] {
        let compressed = zrip::compress(&data, level).unwrap();
        let mut decoder = zrip::FrameDecoder::new(&compressed[..]);
        let mut output = Vec::new();
        decoder.read_to_end(&mut output).unwrap();
        assert_eq!(output, data, "L{level}");
    }
}

#[cfg(not(miri))]
#[test]
fn streaming_encoder_decoder_all_levels() {
    use std::io::{Read, Write};
    let data: Vec<u8> = b"full encode decode pipeline "
        .iter()
        .cycle()
        .take(300_000)
        .copied()
        .collect();
    for level in [-7, -1, 1, 2, 3, 4] {
        let mut encoder = zrip::FrameEncoder::new(Vec::new(), level).unwrap();
        encoder.write_all(&data).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut decoder = zrip::FrameDecoder::new(&compressed[..]);
        let mut output = Vec::new();
        decoder.read_to_end(&mut output).unwrap();
        assert_eq!(output, data, "L{level}");
    }
}

// ===== Options in streaming mode =====

#[cfg(not(miri))]
#[test]
fn streaming_encoder_with_options_window_log() {
    use std::io::{Read, Write};
    let data: Vec<u8> = b"streaming window_log option "
        .iter()
        .cycle()
        .take(2 * BLOCK)
        .copied()
        .collect();
    let opts = zrip::Options::default().window_log(21);
    let mut encoder = zrip::FrameEncoder::with_options(Vec::new(), 3, &opts).unwrap();
    encoder.write_all(&data).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut decoder = zrip::FrameDecoder::new(&compressed[..]);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output).unwrap();
    assert_eq!(output, data);
}

#[cfg(not(miri))]
#[test]
fn streaming_encoder_with_options_reset() {
    use std::io::Write;
    let opts = zrip::Options::default().window_log(21);
    let mut encoder = zrip::FrameEncoder::with_options(Vec::new(), 1, &opts).unwrap();
    let d1: Vec<u8> = b"frame one opts "
        .iter()
        .cycle()
        .take(50_000)
        .copied()
        .collect();
    let d2: Vec<u8> = b"frame two opts "
        .iter()
        .cycle()
        .take(50_000)
        .copied()
        .collect();
    encoder.write_all(&d1).unwrap();
    let out1 = encoder.reset(Vec::new()).unwrap();
    encoder.write_all(&d2).unwrap();
    let out2 = encoder.finish().unwrap();
    assert_eq!(zrip::decompress(&out1).unwrap(), d1);
    assert_eq!(zrip::decompress(&out2).unwrap(), d2);
}

// ===== Cross-block matching =====

#[cfg(not(miri))]
fn make_multiblock_data(total: usize, pattern_len: usize, distance: usize) -> Vec<u8> {
    let mut data = vec![0u8; total];
    for (i, b) in data.iter_mut().enumerate() {
        *b = (i.wrapping_mul(KNUTH as usize) >> 24) as u8;
    }
    let pattern: Vec<u8> = (0..pattern_len)
        .map(|i| ((i * 7 + 13) & 0xFF) as u8)
        .collect();
    data[..pattern_len].copy_from_slice(&pattern);
    if distance + pattern_len <= total {
        data[distance..distance + pattern_len].copy_from_slice(&pattern);
    }
    data
}

#[cfg(not(miri))]
#[test]
fn frame_decoder_multiblock_crossblock_refs() {
    use std::io::Read;
    let data: Vec<u8> = b"ABCDEFGH"
        .iter()
        .cycle()
        .take(2 * BLOCK)
        .copied()
        .collect();
    let compressed = zrip::compress(&data, 1).unwrap();
    let mut decoder = zrip::FrameDecoder::new(&compressed[..]);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output).unwrap();
    assert_eq!(output, data);
}

#[cfg(not(miri))]
#[test]
fn frame_decoder_multiblock_c_zstd_data() {
    use std::io::Read;
    let data: Vec<u8> = b"The quick brown fox jumps over the lazy dog. "
        .iter()
        .cycle()
        .take(300_000)
        .copied()
        .collect();
    let compressed = zstd::encode_all(&data[..], 3).unwrap();
    let mut decoder = zrip::FrameDecoder::new(&compressed[..]);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output).unwrap();
    assert_eq!(output, data);
}

#[cfg(not(miri))]
#[test]
fn streaming_encoder_multiblock_roundtrip() {
    use std::io::Write;
    let data = make_multiblock_data(4 * BLOCK, 4096, 200_000);
    for level in [-7, -3, -1, 1, 2, 3, 4] {
        let mut encoder = zrip::FrameEncoder::new(Vec::new(), level).unwrap();
        encoder.write_all(&data).unwrap();
        let compressed = encoder.finish().unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("L{level}: decompress: {e}"));
        assert_eq!(decompressed, data, "L{level}");
    }
}

#[cfg(not(miri))]
#[test]
fn streaming_encoder_multiblock_c_zstd_decompress() {
    use std::io::Write;
    let data: Vec<u8> = b"streaming multiblock test data "
        .iter()
        .cycle()
        .take(300_000)
        .copied()
        .collect();
    for level in [1, 3, 4] {
        let mut encoder = zrip::FrameEncoder::new(Vec::new(), level).unwrap();
        encoder.write_all(&data).unwrap();
        let compressed = encoder.finish().unwrap();
        let decompressed = zstd::decode_all(&compressed[..]).unwrap();
        assert_eq!(decompressed, data, "L{level}: C zstd decompress mismatch");
    }
}

#[cfg(not(miri))]
#[test]
fn streaming_encoder_crossblock_improves_ratio() {
    use std::io::Write;
    let block_size = BLOCK;
    let block: Vec<u8> = b"ABCDEFGHIJKLMNOP"
        .iter()
        .cycle()
        .take(block_size)
        .copied()
        .collect();
    let mut data = Vec::with_capacity(block_size * 3);
    data.extend_from_slice(&block);
    data.extend_from_slice(&block);
    data.extend_from_slice(&block);

    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    encoder.write_all(&data).unwrap();
    let streaming = encoder.finish().unwrap();
    let oneshot = zrip::compress(&data, 1).unwrap();
    assert!(
        streaming.len() < data.len(),
        "streaming should compress: {} >= {}",
        streaming.len(),
        data.len()
    );
    let ratio_diff = (streaming.len() as f64 / oneshot.len() as f64) - 1.0;
    assert!(
        ratio_diff < 0.15,
        "streaming should be within 15% of oneshot: streaming={} oneshot={} diff={:.1}%",
        streaming.len(),
        oneshot.len(),
        ratio_diff * 100.0
    );
}

#[cfg(not(miri))]
#[test]
fn streaming_encoder_decoder_multiblock_roundtrip() {
    use std::io::{Read, Write};
    let data = make_multiblock_data(3 * BLOCK, 4096, 200_000);
    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    encoder.write_all(&data).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut decoder = zrip::FrameDecoder::new(&compressed[..]);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output).unwrap();
    assert_eq!(output, data);
}

#[cfg(not(miri))]
#[test]
fn streaming_encoder_reset_clears_history() {
    use std::io::Write;
    let data1 = make_multiblock_data(2 * BLOCK, 4096, 150_000);
    let data2 = b"completely different second frame content".repeat(100);
    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    encoder.write_all(&data1).unwrap();
    let out1 = encoder.reset(Vec::new()).unwrap();
    encoder.write_all(&data2).unwrap();
    let out2 = encoder.finish().unwrap();
    assert_eq!(zrip::decompress(&out1).unwrap(), data1);
    assert_eq!(zrip::decompress(&out2).unwrap(), data2);
}

// ===== Large multiblock streaming =====

#[cfg(not(miri))]
#[test]
fn streaming_encoder_decoder_multiblock_large() {
    use std::io::{Read, Write};

    let mut data = Vec::with_capacity(4 * BLOCK + 7000);
    let pattern = b"cross-block match reference test data with enough repetition ";
    for _ in 0..=(4 * BLOCK / pattern.len()) {
        data.extend_from_slice(pattern);
    }
    data.truncate(4 * BLOCK + 7000);

    for level in [-7, -1, 1, 3, 4] {
        let mut encoder = zrip::FrameEncoder::new(Vec::new(), level).unwrap();
        encoder.write_all(&data).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut decoder = zrip::FrameDecoder::new(&compressed[..]);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();
        assert_eq!(decompressed, data, "level {level}");
    }
}

// ===== Options API (large window / LDM) =====

#[test]
fn options_window_log_only() {
    assert!(zrip::compress(b"hello", 5).is_err());
}

#[cfg(not(miri))]
#[test]
fn options_window_log_roundtrip() {
    let data: Vec<u8> = b"options window log test "
        .iter()
        .cycle()
        .take(2 * BLOCK)
        .copied()
        .collect();
    let opts = zrip::Options::default().window_log(24);
    let compressed = zrip::compress_opts(&data, 4, &opts).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, data);
}

#[cfg(feature = "ldm")]
mod ldm {
    #[cfg(not(miri))]
    use super::{BLOCK, KNUTH};

    #[cfg(not(miri))]
    #[test]
    fn roundtrip_with_options_ldm() {
        let data: Vec<u8> = b"LDM roundtrip test data "
            .iter()
            .cycle()
            .take(2 * BLOCK)
            .copied()
            .collect();
        let opts = zrip::Options::default().window_log(26).ldm(true);
        let compressed = zrip::compress_opts(&data, 4, &opts).unwrap();
        let decompressed = zrip::decompress(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[cfg(not(miri))]
    #[test]
    fn c_zstd_decompresses_options_ldm() {
        let data: Vec<u8> = b"cross validate options LDM "
            .iter()
            .cycle()
            .take(2 * BLOCK)
            .copied()
            .collect();
        let opts = zrip::Options::default().window_log(26).ldm(true);
        let compressed = zrip::compress_opts(&data, 4, &opts).unwrap();
        let decompressed = zstd::decode_all(&compressed[..]).unwrap();
        assert_eq!(decompressed, data);
    }

    #[cfg(not(miri))]
    #[test]
    fn streaming_with_options_ldm() {
        use std::io::{Read, Write};
        let data: Vec<u8> = b"streaming LDM options test "
            .iter()
            .cycle()
            .take(4 * BLOCK)
            .copied()
            .collect();
        let opts = zrip::Options::default().window_log(26).ldm(true);
        let mut encoder = zrip::FrameEncoder::with_options(Vec::new(), 4, &opts).unwrap();
        encoder.write_all(&data).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut decoder = zrip::FrameDecoder::new(&compressed[..]);
        let mut output = Vec::new();
        decoder.read_to_end(&mut output).unwrap();
        assert_eq!(output, data);
    }

    #[cfg(not(miri))]
    #[test]
    fn long_range_match_improves_ratio() {
        let dup_block: Vec<u8> = b"DUPLICATED_BLOCK_CONTENT_"
            .iter()
            .cycle()
            .take(BLOCK)
            .copied()
            .collect();
        let mut data = Vec::new();
        data.extend_from_slice(&dup_block);
        for i in 0..72 {
            let filler: Vec<u8> = format!("filler block {i:04} content ")
                .bytes()
                .cycle()
                .take(BLOCK)
                .collect();
            data.extend_from_slice(&filler);
        }
        data.extend_from_slice(&dup_block);
        let l4 = zrip::compress(&data, 4).unwrap();
        let opts = zrip::Options::default().window_log(24);
        let large_win = zrip::compress_opts(&data, 4, &opts).unwrap();
        assert!(
            large_win.len() < l4.len(),
            "window_log=24 should beat L4 (window_log=23) on ~9M-distance match: large_win={} l4={}",
            large_win.len(),
            l4.len(),
        );
    }

    #[cfg(not(miri))]
    #[test]
    fn with_window_log_roundtrip() {
        let data: Vec<u8> = b"custom window log test "
            .iter()
            .cycle()
            .take(BLOCK)
            .copied()
            .collect();
        let params = zrip::encode::strategy::level_params(4)
            .unwrap()
            .with_window_log(20);
        let compressed = zrip::compress_with_params(&data, &params).unwrap();
        let decompressed = zrip::decompress(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[cfg(not(miri))]
    #[test]
    fn streaming_ldm_reset() {
        use std::io::Write;
        let opts = zrip::Options::default().window_log(24).ldm(true);
        let mut encoder = zrip::FrameEncoder::with_options(Vec::new(), 4, &opts).unwrap();
        let d1: Vec<u8> = b"ldm frame one "
            .iter()
            .cycle()
            .take(2 * BLOCK)
            .copied()
            .collect();
        let d2: Vec<u8> = b"ldm frame two "
            .iter()
            .cycle()
            .take(2 * BLOCK)
            .copied()
            .collect();
        encoder.write_all(&d1).unwrap();
        let out1 = encoder.reset(Vec::new()).unwrap();
        encoder.write_all(&d2).unwrap();
        let out2 = encoder.finish().unwrap();
        assert_eq!(zrip::decompress(&out1).unwrap(), d1);
        assert_eq!(zrip::decompress(&out2).unwrap(), d2);
    }

    #[cfg(not(miri))]
    #[test]
    fn streaming_ldm_c_zstd_decompresses() {
        use std::io::Write;
        let data: Vec<u8> = b"ldm streaming c zstd check "
            .iter()
            .cycle()
            .take(4 * BLOCK)
            .copied()
            .collect();
        let opts = zrip::Options::default().window_log(24).ldm(true);
        let mut encoder = zrip::FrameEncoder::with_options(Vec::new(), 4, &opts).unwrap();
        encoder.write_all(&data).unwrap();
        let compressed = encoder.finish().unwrap();
        let decompressed = zstd::decode_all(&compressed[..]).unwrap();
        assert_eq!(decompressed, data);
    }

    #[cfg(not(miri))]
    #[test]
    fn roundtrip_ldm_all_levels() {
        let data: Vec<u8> = b"LDM all levels roundtrip "
            .iter()
            .cycle()
            .take(2 * BLOCK)
            .copied()
            .collect();
        let opts = zrip::Options::default().window_log(24).ldm(true);
        for level in [-7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4] {
            let compressed = zrip::compress_opts(&data, level, &opts).unwrap();
            let decompressed =
                zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
            assert_eq!(decompressed, data, "level {level}");
        }
    }

    #[cfg(not(miri))]
    #[test]
    fn streaming_ldm_cross_block_improves_ratio() {
        use std::io::Write;

        let mut data = Vec::with_capacity(4 * BLOCK);
        let block = b"long distance match target block with distinctive content. ";
        for _ in 0..1024 {
            data.extend_from_slice(block);
        }
        let filler: Vec<u8> = (0..2 * BLOCK as u32)
            .map(|i| ((i.wrapping_mul(KNUTH)) >> 24) as u8)
            .collect();
        data.extend_from_slice(&filler);
        for _ in 0..1024 {
            data.extend_from_slice(block);
        }

        let opts_no_ldm = zrip::Options::default().window_log(24);
        let compressed_no_ldm = zrip::compress_opts(&data, 1, &opts_no_ldm).unwrap();
        let decompressed = zrip::decompress(&compressed_no_ldm).unwrap();
        assert_eq!(decompressed, data);

        let opts_ldm = zrip::Options::default().window_log(24).ldm(true);
        let mut encoder = zrip::FrameEncoder::with_options(Vec::new(), 1, &opts_ldm).unwrap();
        encoder.write_all(&data).unwrap();
        let compressed_ldm = encoder.finish().unwrap();
        let decompressed = zrip::decompress(&compressed_ldm).unwrap();
        assert_eq!(decompressed, data);

        assert!(
            compressed_ldm.len() < compressed_no_ldm.len(),
            "LDM should improve ratio: ldm={} vs no_ldm={}",
            compressed_ldm.len(),
            compressed_no_ldm.len()
        );
    }

    #[cfg(not(miri))]
    #[test]
    fn with_ldm_custom_roundtrip() {
        let data: Vec<u8> = b"custom LDM params test "
            .iter()
            .cycle()
            .take(2 * BLOCK)
            .copied()
            .collect();
        let params = zrip::encode::strategy::level_params(4)
            .unwrap()
            .with_window_log(24)
            .with_ldm(zrip::LdmParams {
                hash_log: 18,
                bucket_size_log: 3,
                min_match_length: 64,
                hash_rate_log: 6,
            });
        let compressed = zrip::compress_with_params(&data, &params).unwrap();
        let decompressed = zrip::decompress(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }
}
