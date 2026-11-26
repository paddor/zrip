// Dictionary tests. Pure zrip (no C zstd).

#[cfg(feature = "dict_builder")]
#[test]
fn zrip_trained_dict_roundtrip() {
    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(r#"{{"id":{i},"name":"user_{i}","email":"user{i}@example.com","active":true}}"#)
                .into_bytes()
        })
        .collect();

    let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &sample_refs,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );

    assert_ne!(dict.id(), 0);
    assert_ne!(dict.content(), []);

    for sample in &samples[..20] {
        let compressed = zrip::compress_with_dict(sample, 1, &dict).unwrap();
        let decompressed = zrip::decompress_with_dict(&compressed, &dict).unwrap();
        assert_eq!(&decompressed, sample);
    }
}

#[cfg(feature = "dict_builder")]
#[test]
fn fastcover_improves_compression() {
    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(r#"{{"id":{i},"name":"user_{i}","email":"user{i}@example.com","active":true}}"#)
                .into_bytes()
        })
        .collect();

    let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &sample_refs,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );

    let test_sample = &samples[50];
    let without_dict = zrip::compress(test_sample, 1).unwrap();
    let with_dict = zrip::compress_with_dict(test_sample, 1, &dict).unwrap();
    assert!(
        with_dict.len() < without_dict.len(),
        "dict should improve compression: {} vs {} bytes",
        with_dict.len(),
        without_dict.len(),
    );
}

#[cfg(feature = "dict_builder")]
#[test]
fn fastcover_various_params() {
    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(r#"{{"id":{i},"name":"user_{i}","email":"user{i}@example.com"}}"#).into_bytes()
        })
        .collect();
    let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();

    let params_list = [
        zrip::dict::fastcover::FastCoverParams {
            k: 1024,
            d: 6,
            accel: 1,
        },
        zrip::dict::fastcover::FastCoverParams {
            k: 2048,
            d: 8,
            accel: 1,
        },
        zrip::dict::fastcover::FastCoverParams {
            k: 4096,
            d: 8,
            accel: 2,
        },
        zrip::dict::fastcover::FastCoverParams {
            k: 2048,
            d: 16,
            accel: 1,
        },
    ];

    for (i, params) in params_list.into_iter().enumerate() {
        let dict = zrip::dict::train_dict_fastcover(&sample_refs, 4096, params);
        assert_ne!(dict.id(), 0, "params {i}");
        for sample in &samples[..10] {
            let compressed = zrip::compress_with_dict(sample, 1, &dict).unwrap();
            let decompressed = zrip::decompress_with_dict(&compressed, &dict).unwrap();
            assert_eq!(&decompressed, sample, "params {i}");
        }
    }
}

#[cfg(feature = "dict_builder")]
#[test]
fn fastcover_diverse_data_types() {
    // JSON-like
    let json_samples: Vec<Vec<u8>> = (0..100)
        .map(|i| format!(r#"{{"key{i}":"val{i}"}}"#).into_bytes())
        .collect();
    let json_refs: Vec<&[u8]> = json_samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &json_refs,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );
    for s in &json_samples[..10] {
        let c = zrip::compress_with_dict(s, 1, &dict).unwrap();
        let d = zrip::decompress_with_dict(&c, &dict).unwrap();
        assert_eq!(&d, s);
    }

    // CSV-like
    let csv_samples: Vec<Vec<u8>> = (0..100)
        .map(|i| format!("{},{},{},{}\n", i, i * 2, i * 3, i % 7).into_bytes())
        .collect();
    let csv_refs: Vec<&[u8]> = csv_samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &csv_refs,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );
    for s in &csv_samples[..10] {
        let c = zrip::compress_with_dict(s, 1, &dict).unwrap();
        let d = zrip::decompress_with_dict(&c, &dict).unwrap();
        assert_eq!(&d, s);
    }

    // Binary-like (protobuf-ish patterns)
    let bin_samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            let mut v = vec![0x08]; // field 1, varint
            v.push((i & 0x7F) as u8);
            v.push(0x12); // field 2, length-delimited
            let s = format!("user_{i}");
            v.push(s.len() as u8);
            v.extend_from_slice(s.as_bytes());
            v
        })
        .collect();
    let bin_refs: Vec<&[u8]> = bin_samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &bin_refs,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );
    for s in &bin_samples[..10] {
        let c = zrip::compress_with_dict(s, 1, &dict).unwrap();
        let d = zrip::decompress_with_dict(&c, &dict).unwrap();
        assert_eq!(&d, s);
    }
}

#[cfg(feature = "dict_builder")]
#[test]
fn fastcover_tiny_samples() {
    let samples: Vec<Vec<u8>> = (0..50).map(|i| vec![i as u8; 10]).collect();
    let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &sample_refs,
        1024,
        zrip::dict::fastcover::FastCoverParams {
            k: 64,
            d: 4,
            accel: 1,
        },
    );
    assert_ne!(dict.id(), 0);
    for s in &samples[..5] {
        let c = zrip::compress_with_dict(s, 1, &dict).unwrap();
        let d = zrip::decompress_with_dict(&c, &dict).unwrap();
        assert_eq!(&d, s);
    }
}

#[cfg(feature = "dict_builder")]
#[test]
fn fastcover_large_dict() {
    let samples: Vec<Vec<u8>> = (0..200)
        .map(|i| {
            format!(
                r#"{{"id":{},"name":"user_{}","email":"user{}@example.com","bio":"{}"}}"#,
                i,
                i,
                i,
                "x".repeat(100 + (i % 50)),
            )
            .into_bytes()
        })
        .collect();
    let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &sample_refs,
        32768,
        zrip::dict::fastcover::FastCoverParams::default(),
    );
    assert_ne!(dict.id(), 0);
    for s in &samples[..20] {
        let c = zrip::compress_with_dict(s, 1, &dict).unwrap();
        let d = zrip::decompress_with_dict(&c, &dict).unwrap();
        assert_eq!(&d, s);
    }
}

#[cfg(feature = "dict_builder")]
#[test]
fn fastcover_all_levels() {
    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(r#"{{"id":{i},"name":"user_{i}","email":"user{i}@example.com","active":true}}"#)
                .into_bytes()
        })
        .collect();
    let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &sample_refs,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );
    for level in [-7, -5, -3, -1, 1, 2, 3, 4] {
        for sample in &samples[..5] {
            let compressed = zrip::compress_with_dict(sample, level, &dict).unwrap();
            let decompressed = zrip::decompress_with_dict(&compressed, &dict).unwrap();
            assert_eq!(&decompressed, sample, "level {level}");
        }
    }
}

// ===== Streaming dict roundtrips =====

#[cfg(feature = "dict_builder")]
#[test]
fn streaming_dict_zrip_trained_roundtrip() {
    use std::io::{Read, Write};

    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(r#"{{"id":{i},"name":"user_{i}","email":"user{i}@example.com","active":true}}"#)
                .into_bytes()
        })
        .collect();
    let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &sample_refs,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );

    for level in [-1, 1, 3, 4] {
        for sample in &samples[..10] {
            let mut encoder =
                zrip::FrameEncoder::with_dict(Vec::new(), level, dict.clone()).unwrap();
            encoder.write_all(sample).unwrap();
            let compressed = encoder.finish().unwrap();

            let mut decoder = zrip::FrameDecoder::with_dict(compressed.as_slice(), dict.clone());
            let mut decompressed = Vec::new();
            decoder.read_to_end(&mut decompressed).unwrap();
            assert_eq!(
                &decompressed, sample,
                "L{level} zrip-trained streaming dict roundtrip mismatch"
            );

            let oneshot = zrip::decompress_with_dict(&compressed, &dict).unwrap();
            assert_eq!(&oneshot, sample);
        }
    }
}

#[cfg(feature = "dict_builder")]
#[test]
fn streaming_dict_empty_input() {
    use std::io::{Read, Write};

    let samples: Vec<Vec<u8>> = (0..50)
        .map(|i| format!("sample data item {i} with some repeated content").into_bytes())
        .collect();
    let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &sample_refs,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );

    let mut encoder = zrip::FrameEncoder::with_dict(Vec::new(), 1, dict.clone()).unwrap();
    encoder.write_all(b"").unwrap();
    let compressed = encoder.finish().unwrap();

    let mut decoder = zrip::FrameDecoder::with_dict(compressed.as_slice(), dict.clone());
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed).unwrap();
    assert_eq!(decompressed, Vec::<u8>::new());
}

#[cfg(feature = "dict_builder")]
#[test]
fn streaming_dict_single_byte_writes() {
    use std::io::{Read, Write};

    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(r#"{{"id":{i},"name":"user_{i}","email":"user{i}@example.com","active":true}}"#)
                .into_bytes()
        })
        .collect();
    let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &sample_refs,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );

    let data = &samples[50];
    let mut encoder = zrip::FrameEncoder::with_dict(Vec::new(), 1, dict.clone()).unwrap();
    for &b in data.iter() {
        encoder.write_all(&[b]).unwrap();
    }
    let compressed = encoder.finish().unwrap();

    let mut decoder = zrip::FrameDecoder::with_dict(compressed.as_slice(), dict.clone());
    let mut decompressed = Vec::new();
    let mut buf = [0u8; 1];
    loop {
        match decoder.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => decompressed.extend_from_slice(&buf[..n]),
            Err(e) => panic!("decode failed: {e}"),
        }
    }
    assert_eq!(&decompressed, data);
}

#[cfg(feature = "dict_builder")]
#[test]
fn streaming_dict_roundtrip() {
    use std::io::{Read, Write};

    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(r#"{{"id":{i},"name":"user_{i}","email":"user{i}@example.com","active":true}}"#)
                .into_bytes()
        })
        .collect();
    let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &sample_refs,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );

    for level in [-1, 1, 3, 4] {
        for sample in &samples[..10] {
            let mut encoder =
                zrip::FrameEncoder::with_dict(Vec::new(), level, dict.clone()).unwrap();
            encoder.write_all(sample).unwrap();
            let compressed = encoder.finish().unwrap();

            let mut decoder = zrip::FrameDecoder::with_dict(compressed.as_slice(), dict.clone());
            let mut decompressed = Vec::new();
            decoder.read_to_end(&mut decompressed).unwrap();
            assert_eq!(
                &decompressed, sample,
                "L{level} streaming dict roundtrip mismatch"
            );
        }
    }
}

#[cfg(feature = "dict_builder")]
#[test]
fn streaming_dict_multiblock() {
    use std::io::Write;

    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(r#"{{"id":{i},"name":"user_{i}","email":"user{i}@example.com","active":true}}"#)
                .into_bytes()
        })
        .collect();
    let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &sample_refs,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );

    let data: Vec<u8> = (0..200_000)
        .map(|i| {
            let pattern = b"{\"id\":1234,\"name\":\"user_test\",\"email\":\"test@example.com\"}";
            pattern[i % pattern.len()]
        })
        .collect();

    for level in [1, 3] {
        let mut encoder = zrip::FrameEncoder::with_dict(Vec::new(), level, dict.clone()).unwrap();
        encoder.write_all(&data).unwrap();
        let compressed = encoder.finish().unwrap();

        let decompressed = zrip::decompress_with_dict(&compressed, &dict).unwrap();
        assert_eq!(
            decompressed.len(),
            data.len(),
            "L{level} multiblock size mismatch"
        );
        assert_eq!(decompressed, data, "L{level} multiblock content mismatch");
    }
}

#[cfg(feature = "dict_builder")]
#[test]
fn streaming_encoder_reset_with_dict() {
    use std::io::Write;

    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(r#"{{"id":{i},"name":"user_{i}","email":"user{i}@example.com","active":true}}"#)
                .into_bytes()
        })
        .collect();
    let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &sample_refs,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );

    let mut encoder = zrip::FrameEncoder::with_dict(Vec::new(), 1, dict.clone()).unwrap();

    for sample in &samples[..5] {
        encoder.write_all(sample).unwrap();
        let compressed = encoder.reset(Vec::new()).unwrap();
        let decompressed = zrip::decompress_with_dict(&compressed, &dict).unwrap();
        assert_eq!(&decompressed, sample);
    }

    encoder.write_all(&samples[5]).unwrap();
    let compressed = encoder.finish().unwrap();
    assert_eq!(
        zrip::decompress_with_dict(&compressed, &dict).unwrap(),
        samples[5]
    );
}

#[cfg(feature = "dict_builder")]
#[test]
fn streaming_decoder_reset_with_dict() {
    use std::io::Read;

    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(r#"{{"id":{i},"name":"user_{i}","email":"user{i}@example.com","active":true}}"#)
                .into_bytes()
        })
        .collect();
    let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &sample_refs,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );

    let compressed: Vec<Vec<u8>> = samples[..5]
        .iter()
        .map(|s| zrip::compress_with_dict(s, 1, &dict).unwrap())
        .collect();

    let mut decoder = zrip::FrameDecoder::with_dict(compressed[0].as_slice(), dict.clone());

    for (i, sample) in samples[..5].iter().enumerate() {
        if i > 0 {
            decoder.reset(compressed[i].as_slice());
        }
        let mut out = Vec::new();
        decoder.read_to_end(&mut out).unwrap();
        assert_eq!(&out, sample);
    }
}

#[cfg(feature = "dict_builder")]
#[test]
fn streaming_dict_multiblock_reset() {
    use std::io::Write;

    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(r#"{{"id":{i},"name":"user_{i}","email":"user{i}@example.com","active":true}}"#)
                .into_bytes()
        })
        .collect();
    let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &sample_refs,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );

    let data: Vec<u8> = (0..200_000)
        .map(|i| {
            let pattern = b"{\"id\":1234,\"name\":\"user_test\",\"email\":\"test@example.com\"}";
            pattern[i % pattern.len()]
        })
        .collect();

    let mut encoder = zrip::FrameEncoder::with_dict(Vec::new(), 1, dict.clone()).unwrap();
    for _ in 0..3 {
        encoder.write_all(&data).unwrap();
        let compressed = encoder.reset(Vec::new()).unwrap();
        let out = zrip::decompress_with_dict(&compressed, &dict).unwrap();
        assert_eq!(out, data);
    }
}

// ===== Dict mismatch =====

#[cfg(feature = "dict_builder")]
#[test]
fn streaming_decoder_dict_mismatch() {
    use std::io::Read;

    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(r#"{{"id":{i},"name":"user_{i}","email":"user{i}@example.com","active":true}}"#)
                .into_bytes()
        })
        .collect();
    let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &sample_refs,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );

    let compressed = zrip::compress_with_dict(&samples[0], 1, &dict).unwrap();

    // Try to decode with no dict
    let mut decoder = zrip::FrameDecoder::new(compressed.as_slice());
    let mut out = Vec::new();
    let err = decoder.read_to_end(&mut out).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

    // Train a different dict to get a different dict_id
    let other_samples: Vec<Vec<u8>> = (0..100)
        .map(|i| format!("completely different data pattern {i} abcdefghijk").into_bytes())
        .collect();
    let other_refs: Vec<&[u8]> = other_samples.iter().map(|s| s.as_slice()).collect();
    let other_dict = zrip::dict::train_dict_fastcover(
        &other_refs,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );

    if other_dict.id() != dict.id() {
        let mut decoder2 = zrip::FrameDecoder::with_dict(compressed.as_slice(), other_dict);
        let mut out2 = Vec::new();
        let err2 = decoder2.read_to_end(&mut out2).unwrap_err();
        assert_eq!(err2.kind(), std::io::ErrorKind::InvalidData);
    }
}

/// Consecutive dictionary frames whose first compressed blocks share a
/// sequence table header must each decode with their own tables.
#[cfg(feature = "dict_builder")]
#[test]
fn frame_decoder_dict_repeated_identical_frames() {
    use std::io::Read;
    let record = |i: u32| {
        format!(
            r#"{{"id":{i},"name":"user_{i}","score":{},"active":{}}}"#,
            i.wrapping_mul(2_654_435_761) % 100_000,
            i.is_multiple_of(3)
        )
        .into_bytes()
    };
    let samples: Vec<Vec<u8>> = (0..200).map(record).collect();
    let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &sample_refs,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );

    let data: Vec<u8> = (1000..2500).flat_map(record).collect();
    let frame = zrip::compress_with_dict(&data, 1, &dict).unwrap();
    let stream = [frame.as_slice(), frame.as_slice(), frame.as_slice()].concat();
    let mut decoder = zrip::FrameDecoder::with_dict(stream.as_slice(), dict);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output).unwrap();
    assert_eq!(output, data.repeat(3));
}

/// JSON events with recurring keys and varying values, `len` bytes long.
#[cfg(feature = "dict_builder")]
fn events(seed: u32, len: usize) -> Vec<u8> {
    use std::fmt::Write as _;

    let mut state = seed.max(1);
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let mut out = String::from("[");
    while out.len() < len {
        let (id, px, qty, ts) = (next(), next() % 100_000, next() % 1000, next());
        let sym = ["AAPL", "MSFT", "NVDA", "AMZN"][(id % 4) as usize];
        write!(
            out,
            r#"{{"id":{id},"sym":"{sym}","side":"buy","px":{}.{:02},"qty":{qty},"ts":{ts}}},"#,
            px / 100,
            px % 100
        )
        .unwrap();
    }
    out.truncate(len);
    out.into_bytes()
}

/// Each input size takes another dictionary path: attached below 8 KiB, the
/// prefilled snapshot or attached by parameter tier above, and multi-block
/// prefix compression past 128 KiB. The sizes around 256 KiB cross the limit of
/// the denser search at L3 and L4.
///
/// Past one block the dictionary only helps early on, and search noise in later
/// blocks can outweigh it: at most 0.07% across 200 seeds.
#[cfg(feature = "dict_builder")]
#[test]
fn dict_frames_never_lose_to_plain_frames_on_similar_records() {
    let samples: Vec<Vec<u8>> = (0..64).map(|i| events(7 + i, 512)).collect();
    let sample_refs: Vec<&[u8]> = samples.iter().map(Vec::as_slice).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &sample_refs,
        2048,
        zrip::dict::fastcover::FastCoverParams::default(),
    );

    for level in [-7, -5, -3, -1, 1, 2, 3, 4] {
        let mut plain = zrip::CompressContext::new(level).unwrap();
        let mut with = zrip::CompressContext::with_dict(level, dict.clone()).unwrap();
        for len in [
            300, 1024, 4096, 8192, 8193, 16384, 16385, 40_000, 200_000, 261_000, 262_144, 262_145,
        ] {
            let input = events(1000 + len as u32, len);
            let without = plain.compress(&input).unwrap().len();
            let frame = with.compress(&input).unwrap().into_owned();
            assert_eq!(zrip::decompress_with_dict(&frame, &dict).unwrap(), input);
            let slack = if len > 128 * 1024 { without / 1000 } else { 0 };
            assert!(
                frame.len() <= without + slack,
                "L{level} {len} B: dict frame {} B, plain frame {without} B",
                frame.len()
            );
        }
    }
}

#[cfg(feature = "dict_builder")]
#[test]
fn set_level_prepares_the_dictionary_again() {
    let samples: Vec<Vec<u8>> = (0..64).map(|i| events(7 + i, 512)).collect();
    let sample_refs: Vec<&[u8]> = samples.iter().map(Vec::as_slice).collect();
    let dict = zrip::dict::train_dict_fastcover(
        &sample_refs,
        2048,
        zrip::dict::fastcover::FastCoverParams::default(),
    );

    let mut ctx = zrip::CompressContext::with_dict(-1, dict.clone()).unwrap();
    for level in [4, -7, 1, 3, -1] {
        ctx.set_level(level).unwrap();
        for len in [300, 8192, 20_000] {
            let input = events(len as u32, len);
            let mut fresh = zrip::CompressContext::with_dict(level, dict.clone()).unwrap();
            let frame = ctx.compress(&input).unwrap().into_owned();
            assert_eq!(
                frame,
                fresh.compress(&input).unwrap().into_owned(),
                "L{level} {len} B"
            );
            assert_eq!(zrip::decompress_with_dict(&frame, &dict).unwrap(), input);
        }
    }
}

#[cfg(feature = "dict_builder")]
#[test]
fn small_dict_sizes_train_the_least_dictionary() {
    // As bindings train: select the content, then finalize it.
    use zrip::dict::fastcover::{FastCoverParams, select_segments};
    use zrip::dict::{MIN_DICT_SIZE, finalize::finalize_dictionary, train_dict_fastcover};
    let samples: Vec<Vec<u8>> = (0..64).map(|i| events(7 + i, 512)).collect();
    let refs: Vec<&[u8]> = samples.iter().map(Vec::as_slice).collect();
    let least = train_dict_fastcover(&refs, MIN_DICT_SIZE, FastCoverParams::default());
    for size in [0, 16, 64, MIN_DICT_SIZE - 1, MIN_DICT_SIZE] {
        let content = select_segments(&refs, size, &FastCoverParams::default());
        let dict = finalize_dictionary(&content, &refs, size);
        assert_eq!(dict.len(), MIN_DICT_SIZE, "size {size}");
        let trained = train_dict_fastcover(&refs, size, FastCoverParams::default());
        assert_eq!(trained.content(), least.content(), "size {size}");
        let frame = zrip::compress_with_dict(&samples[3], 3, &trained).unwrap();
        assert_eq!(
            zrip::decompress_with_dict(&frame, &trained).unwrap(),
            samples[3]
        );
    }
}
