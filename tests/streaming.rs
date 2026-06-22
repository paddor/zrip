#![cfg(feature = "std")]
// Streaming encoder/decoder tests. Pure zrip (no C zstd).

#[test]
fn streaming_encoder_basic() {
    let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(10000).copied().collect();
    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    std::io::Write::write_all(&mut encoder, &original).unwrap();
    let compressed = encoder.finish().unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, original);
}

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
