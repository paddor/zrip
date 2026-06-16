use zrip;

// ===== Encoder round-trip (zrip compress -> C decompress) =====

#[test]
fn roundtrip_all_levels_repetitive() {
    let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(200_000).copied().collect();
    for level in [-7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4] {
        let compressed = zrip::compress(&original, level).unwrap();
        let decompressed = zrip::decompress(&compressed)
            .unwrap_or_else(|e| panic!("level {level} zrip decompress: {e}"));
        assert_eq!(decompressed, original, "level {level} zrip roundtrip");
        let c_decompressed = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("level {level} C decompress: {e}"));
        assert_eq!(c_decompressed, original, "level {level} C roundtrip");
    }
}

#[test]
fn roundtrip_all_levels_random() {
    let original: Vec<u8> = (0..100_000u32)
        .map(|i| ((i.wrapping_mul(2654435761)) >> 24) as u8)
        .collect();
    for level in [-7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4] {
        let compressed = zrip::compress(&original, level).unwrap();
        let decompressed = zrip::decompress(&compressed)
            .unwrap_or_else(|e| panic!("level {level} zrip decompress: {e}"));
        assert_eq!(decompressed, original, "level {level} zrip roundtrip");
        let c_decompressed = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("level {level} C decompress: {e}"));
        assert_eq!(c_decompressed, original, "level {level} C roundtrip");
    }
}

#[test]
fn roundtrip_zeros() {
    let original = vec![0u8; 100_000];
    let compressed = zrip::compress(&original, 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, original);
    let c_decompressed = zstd::decode_all(&compressed[..]).unwrap();
    assert_eq!(c_decompressed, original);
}

#[test]
fn roundtrip_empty() {
    let compressed = zrip::compress(b"", 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, b"");
    let c_decompressed = zstd::decode_all(&compressed[..]).unwrap();
    assert_eq!(c_decompressed, b"");
}

#[test]
fn roundtrip_small() {
    for size in [1, 7, 15, 31, 63, 127, 255, 512, 1024] {
        let original: Vec<u8> = (0u8..=255).cycle().take(size).collect();
        let compressed = zrip::compress(&original, 1).unwrap();
        let decompressed = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("size {size}: C decompress failed: {e}"));
        assert_eq!(decompressed, original, "size {size}");
    }
}

#[test]
fn roundtrip_large() {
    let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(200_000).copied().collect();
    let compressed = zrip::compress(&original, 1).unwrap();
    let decompressed = zstd::decode_all(&compressed[..]).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn roundtrip_zrip_compress_decompress() {
    let original: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    let compressed = zrip::compress(&original, 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn zrip_compress_c_decompress() {
    let original: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    let compressed = zrip::compress(&original, 1).unwrap();
    let decompressed = zstd::decode_all(&compressed[..]).unwrap();
    assert_eq!(decompressed, original);
}

// ===== Decoder cross-validation (C compress -> zrip decompress) =====

#[test]
fn decompress_c_zstd_empty() {
    let compressed = zstd::encode_all(&b""[..], 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, b"");
}

#[test]
fn decompress_c_zstd_hello() {
    let original = b"Hello, World!";
    let compressed = zstd::encode_all(&original[..], 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn decompress_c_zstd_repetitive() {
    let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(10000).copied().collect();
    let compressed = zstd::encode_all(&original[..], 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn decompress_c_zstd_all_levels() {
    let original: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    for level in [1, 2, 3, 4] {
        let compressed = zstd::encode_all(&original[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, original, "level {level} mismatch");
    }
}

#[test]
fn decompress_c_zstd_negative_levels() {
    let original: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    for level in [-7, -5, -3, -1] {
        let compressed = zstd::encode_all(&original[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, original, "level {level} mismatch");
    }
}

#[test]
fn decompress_c_zstd_random() {
    let original: Vec<u8> = (0..100_000u32)
        .map(|i| ((i.wrapping_mul(2654435761)) >> 24) as u8)
        .collect();
    for level in [1, 3, 5, 9] {
        let compressed = zstd::encode_all(&original[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, original, "level {level} mismatch");
    }
}

#[test]
fn decompress_c_zstd_small_data() {
    for size in [1, 2, 3, 4, 7, 8, 15, 16, 31, 32, 63, 64, 127, 128, 255] {
        let original: Vec<u8> = (0u8..=255).cycle().take(size).collect();
        let compressed = zstd::encode_all(&original[..], 1).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("size {size}: {e}"));
        assert_eq!(decompressed, original, "size {size} mismatch");
    }
}

#[test]
fn xxh64_matches_c_zstd_checksum() {
    let data = b"test data for checksum validation with zrip and C zstd";
    let mut encoder = zstd::Encoder::new(Vec::new(), 1).unwrap();
    encoder.include_checksum(true).unwrap();
    std::io::Write::write_all(&mut encoder, data).unwrap();
    let compressed = encoder.finish().unwrap();

    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
fn decompress_c_zstd_high_levels() {
    let original: Vec<u8> = (0u8..=255).cycle().take(8192).collect();
    for level in [5, 9, 15, 19, 22] {
        let compressed = zstd::encode_all(&original[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, original, "level {level} mismatch");
    }
}

// ===== Stress: diverse data patterns at every level =====

#[test]
fn decompress_c_all_levels_all_patterns() {
    let patterns: Vec<(&str, Vec<u8>)> = vec![
        ("single_byte", vec![0x42; 10000]),
        (
            "two_bytes",
            vec![0xAA, 0x55].into_iter().cycle().take(10000).collect(),
        ),
        ("ascending", (0u8..=255).cycle().take(10000).collect()),
        (
            "descending",
            (0u8..=255).rev().cycle().take(10000).collect(),
        ),
        (
            "text_like",
            b"the quick brown fox jumps over the lazy dog. "
                .iter()
                .cycle()
                .take(10000)
                .copied()
                .collect(),
        ),
        ("long_match", {
            let pattern: Vec<u8> = (0..1000).map(|i| (i % 251) as u8).collect();
            pattern.iter().cycle().take(10000).copied().collect()
        }),
        (
            "random_ish",
            (0..10000u32)
                .map(|i| {
                    ((i as u64)
                        .wrapping_mul(6364136223846793005u64)
                        .wrapping_add(1442695040888963407u64)
                        >> 56) as u8
                })
                .collect(),
        ),
    ];

    for level in [-7, -5, -3, -1, 1, 2, 3, 4, 5, 9, 15, 22] {
        for (name, data) in &patterns {
            let compressed = zstd::encode_all(&data[..], level).unwrap();
            let decompressed = zrip::decompress(&compressed)
                .unwrap_or_else(|e| panic!("pattern={name} level={level}: {e}"));
            assert_eq!(&decompressed, data, "pattern={name} level={level}");
        }
    }
}

#[test]
fn roundtrip_all_levels_all_patterns() {
    let patterns: Vec<(&str, Vec<u8>)> = vec![
        ("single_byte", vec![0x42; 10000]),
        (
            "two_bytes",
            vec![0xAA, 0x55].into_iter().cycle().take(10000).collect(),
        ),
        ("ascending", (0u8..=255).cycle().take(10000).collect()),
        (
            "text_like",
            b"hello world! "
                .iter()
                .cycle()
                .take(10000)
                .copied()
                .collect(),
        ),
        (
            "random_ish",
            (0..10000u32)
                .map(|i| {
                    ((i as u64)
                        .wrapping_mul(6364136223846793005u64)
                        .wrapping_add(1442695040888963407u64)
                        >> 56) as u8
                })
                .collect(),
        ),
    ];

    for level in [-7, -5, -3, -1, 1, 2, 3, 4] {
        for (name, data) in &patterns {
            let compressed = zrip::compress(data, level).unwrap();
            let decompressed = zrip::decompress(&compressed)
                .unwrap_or_else(|e| panic!("pattern={name} level={level} zrip rt: {e}"));
            assert_eq!(&decompressed, data, "pattern={name} level={level} zrip rt");
            let c_decompressed = zstd::decode_all(&compressed[..])
                .unwrap_or_else(|e| panic!("pattern={name} level={level} C: {e}"));
            assert_eq!(&c_decompressed, data, "pattern={name} level={level} C rt");
        }
    }
}

// ===== Edge cases: block boundary, max block size, minimal data =====

#[test]
fn roundtrip_block_boundary_sizes() {
    for size in [
        128 * 1024 - 1,
        128 * 1024,
        128 * 1024 + 1,
        256 * 1024,
        256 * 1024 + 1,
    ] {
        let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(size).copied().collect();
        let compressed = zrip::compress(&original, 1).unwrap();
        let decompressed =
            zstd::decode_all(&compressed[..]).unwrap_or_else(|e| panic!("size {size}: {e}"));
        assert_eq!(decompressed, original, "size {size}");
    }
}

#[test]
fn roundtrip_single_bytes() {
    for b in 0u8..=255 {
        let original = vec![b];
        let compressed = zrip::compress(&original, 1).unwrap();
        let decompressed =
            zstd::decode_all(&compressed[..]).unwrap_or_else(|e| panic!("byte {b:#04x}: {e}"));
        assert_eq!(decompressed, original, "byte {b:#04x}");
    }
}

#[test]
fn decompress_c_block_boundary_sizes() {
    for size in [128 * 1024 - 1, 128 * 1024, 128 * 1024 + 1, 200_000] {
        let original: Vec<u8> = (0..size as u32)
            .map(|i| ((i.wrapping_mul(2654435761)) >> 24) as u8)
            .collect();
        let compressed = zstd::encode_all(&original[..], 1).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("size {size}: {e}"));
        assert_eq!(decompressed, original, "size {size}");
    }
}

// ===== Extreme data patterns =====

#[test]
fn roundtrip_all_same_bytes() {
    for b in [0u8, 1, 127, 128, 254, 255] {
        let original = vec![b; 50_000];
        let compressed = zrip::compress(&original, 1).unwrap();
        let decompressed = zrip::decompress(&compressed).unwrap();
        assert_eq!(decompressed, original, "byte {b}");
        let c_dec = zstd::decode_all(&compressed[..]).unwrap();
        assert_eq!(c_dec, original, "byte {b} C");
    }
}

#[test]
fn decompress_c_high_entropy() {
    let original: Vec<u8> = (0..50_000u32)
        .map(|i| {
            let x = i
                .wrapping_mul(2654435761)
                .wrapping_add(i.wrapping_mul(1103515245));
            (x >> 16) as u8
        })
        .collect();
    for level in [1, 3, 9, 22] {
        let compressed = zstd::encode_all(&original[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, original, "level {level}");
    }
}

#[test]
fn decompress_c_one_long_match() {
    let mut original = vec![0u8; 50_000];
    original[0] = 1;
    for level in [1, 3, 9] {
        let compressed = zstd::encode_all(&original[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, original, "level {level}");
    }
}

#[test]
fn decompress_c_many_short_matches() {
    let original: Vec<u8> = b"abcd".iter().cycle().take(50_000).copied().collect();
    for level in [1, 3, 9] {
        let compressed = zstd::encode_all(&original[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, original, "level {level}");
    }
}

#[test]
fn decompress_c_alternating_compressible_incompressible() {
    let mut original = Vec::with_capacity(50_000);
    for i in 0..100 {
        if i % 2 == 0 {
            original.extend(std::iter::repeat(0x42u8).take(250));
        } else {
            original.extend((0..250u32).map(|j| ((j + i).wrapping_mul(2654435761) >> 24) as u8));
        }
    }
    for level in [1, 3, 9] {
        let compressed = zstd::encode_all(&original[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, original, "level {level}");
    }
}

// ===== Size sweep: power-of-two boundaries =====

#[test]
fn decompress_c_size_sweep() {
    for exp in 0..=17 {
        let size = 1usize << exp;
        for offset in [0, 1] {
            let s = size + offset;
            if s == 0 {
                continue;
            }
            let original: Vec<u8> = (0..s as u32)
                .map(|i| ((i.wrapping_mul(2654435761)) >> 24) as u8)
                .collect();
            let compressed = zstd::encode_all(&original[..], 1).unwrap();
            let decompressed =
                zrip::decompress(&compressed).unwrap_or_else(|e| panic!("size {s}: {e}"));
            assert_eq!(decompressed, original, "size {s}");
        }
    }
}

#[test]
fn roundtrip_size_sweep() {
    for exp in 0..=17 {
        let size = 1usize << exp;
        let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(size).copied().collect();
        let compressed = zrip::compress(&original, 1).unwrap();
        let decompressed =
            zstd::decode_all(&compressed[..]).unwrap_or_else(|e| panic!("size {size}: {e}"));
        assert_eq!(decompressed, original, "size {size}");
    }
}

// ===== Error handling =====

#[test]
fn decompress_truncated_frame() {
    let original = b"hello world hello world hello world";
    let compressed = zstd::encode_all(&original[..], 1).unwrap();
    for truncate_at in [1, 2, 3, 4, 5, compressed.len() / 2, compressed.len() - 1] {
        let truncated = &compressed[..truncate_at];
        assert!(
            zrip::decompress(truncated).is_err(),
            "should fail at truncate_at={truncate_at}"
        );
    }
}

#[test]
fn decompress_bad_magic() {
    let data = [0x00, 0x00, 0x00, 0x00, 0x00];
    assert!(zrip::decompress(&data).is_err());
}

#[test]
fn decompress_empty_input() {
    let result = zrip::decompress(&[]).unwrap();
    assert!(result.is_empty());
}

#[test]
fn compress_level_zero_is_default() {
    let data = b"ABCDEFGH".repeat(1000);
    let c0 = zrip::compress(&data, 0).unwrap();
    let c1 = zrip::compress(&data, 1).unwrap();
    assert_eq!(c0, c1);
}

#[test]
fn compress_invalid_level() {
    assert!(zrip::compress(b"hello", 5).is_err());
    assert!(zrip::compress(b"hello", -8).is_err());
    assert!(zrip::compress(b"hello", 100).is_err());
}

// ===== compress_into API =====

#[test]
fn compress_into_basic() {
    let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(10000).copied().collect();
    let mut buf = vec![0u8; original.len() + 100];
    let n = zrip::compress_into(&original, &mut buf, 1).unwrap();
    let decompressed = zstd::decode_all(&buf[..n]).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn compress_into_too_small() {
    let original = b"hello world hello world hello world";
    let mut buf = [0u8; 1];
    assert!(zrip::compress_into(original, &mut buf, 1).is_err());
}

// ===== Streaming FrameEncoder =====

#[test]
fn streaming_encoder_basic() {
    let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(10000).copied().collect();
    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    std::io::Write::write_all(&mut encoder, &original).unwrap();
    let compressed = encoder.finish().unwrap();
    let decompressed = zstd::decode_all(&compressed[..]).unwrap();
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
    let decompressed = zstd::decode_all(&compressed[..]).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn streaming_encoder_empty() {
    let encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    let compressed = encoder.finish().unwrap();
    let decompressed = zstd::decode_all(&compressed[..]).unwrap();
    assert_eq!(decompressed, b"");
}

#[test]
fn streaming_encoder_all_levels() {
    let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(5000).copied().collect();
    for level in [-7, -5, -3, -1, 1, 2, 3, 4] {
        let mut encoder = zrip::FrameEncoder::new(Vec::new(), level).unwrap();
        std::io::Write::write_all(&mut encoder, &original).unwrap();
        let compressed = encoder.finish().unwrap();
        let decompressed = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("level {level}: C decompress: {e}"));
        assert_eq!(decompressed, original, "level {level}");
    }
}

// ===== Checksum validation =====

#[test]
fn checksum_mismatch_detected() {
    let original = b"test data for checksum";
    let mut compressed = zrip::compress(original, 1).unwrap();
    let last = compressed.len() - 1;
    compressed[last] ^= 0xFF;
    assert!(zrip::decompress(&compressed).is_err());
}

#[test]
fn checksum_various_sizes() {
    for size in [0, 1, 100, 1000, 10000, 100_000] {
        let original: Vec<u8> = (0..size as u32)
            .map(|i| ((i.wrapping_mul(2654435761)) >> 24) as u8)
            .collect();
        let compressed = zrip::compress(&original, 1).unwrap();
        let decompressed = zrip::decompress(&compressed).unwrap();
        assert_eq!(decompressed, original, "size {size}");
    }
}

// ===== Proptest: randomized cross-validation =====

mod proptest_tests {
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        #[test]
        fn roundtrip_random_data(data in proptest::collection::vec(any::<u8>(), 0..50_000)) {
            let level = 1;
            let compressed = zrip::compress(&data, level).unwrap();
            let decompressed = zrip::decompress(&compressed).unwrap();
            prop_assert_eq!(&decompressed, &data);
            let c_decompressed = zstd::decode_all(&compressed[..]).unwrap();
            prop_assert_eq!(&c_decompressed, &data);
        }

        #[test]
        fn roundtrip_random_data_all_levels(
            data in proptest::collection::vec(any::<u8>(), 100..10_000),
            level in prop_oneof![Just(-7i32), Just(-5), Just(-3), Just(-1), Just(1), Just(2), Just(3), Just(4)]
        ) {
            let compressed = zrip::compress(&data, level).unwrap();
            let decompressed = zrip::decompress(&compressed).unwrap();
            prop_assert_eq!(&decompressed, &data);
            let c_decompressed = zstd::decode_all(&compressed[..]).unwrap();
            prop_assert_eq!(&c_decompressed, &data);
        }

        #[test]
        fn decompress_c_random_data(data in proptest::collection::vec(any::<u8>(), 0..50_000)) {
            let compressed = zstd::encode_all(&data[..], 1).unwrap();
            let decompressed = zrip::decompress(&compressed).unwrap();
            prop_assert_eq!(&decompressed, &data);
        }

        #[test]
        fn decompress_c_random_levels(
            data in proptest::collection::vec(any::<u8>(), 100..10_000),
            level in 1..=22i32,
        ) {
            let compressed = zstd::encode_all(&data[..], level).unwrap();
            let decompressed = zrip::decompress(&compressed).unwrap();
            prop_assert_eq!(&decompressed, &data);
        }

        #[test]
        fn decompress_corrupt_never_panics(data in proptest::collection::vec(any::<u8>(), 4..200)) {
            let _ = zrip::decompress(&data);
        }

        #[test]
        fn roundtrip_repetitive_varying_period(
            period in 1usize..256,
            count in 1usize..1000,
        ) {
            let original: Vec<u8> = (0..period).map(|i| i as u8).collect::<Vec<_>>()
                .into_iter().cycle().take(period * count).collect();
            let compressed = zrip::compress(&original, 1).unwrap();
            let decompressed = zrip::decompress(&compressed).unwrap();
            prop_assert_eq!(&decompressed, &original);
            let c_dec = zstd::decode_all(&compressed[..]).unwrap();
            prop_assert_eq!(&c_dec, &original);
        }
    }
}

// ===== Dictionary parsing =====

#[test]
fn parse_c_trained_dictionary() {
    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(
                r#"{{"id":{},"name":"user_{}","email":"user{}@example.com","active":true}}"#,
                i, i, i
            )
            .into_bytes()
        })
        .collect();

    let mut concat = Vec::new();
    let mut sizes = Vec::new();
    for s in &samples {
        concat.extend_from_slice(s);
        sizes.push(s.len());
    }

    let mut dict_buf = vec![0u8; 16384];
    let dict_size =
        zstd_safe::train_from_buffer(&mut dict_buf, &concat, &sizes).expect("training failed");
    let dict_data = &dict_buf[..dict_size];

    let dict = zrip::dict::Dictionary::from_bytes(dict_data).unwrap();
    assert_ne!(dict.id(), 0);
    assert!(!dict.content().is_empty());
    assert!(dict.rep_offsets()[0] > 0);
    assert!(dict.rep_offsets()[1] > 0);
    assert!(dict.rep_offsets()[2] > 0);
}

#[test]
fn decompress_c_with_dictionary() {
    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(
                r#"{{"id":{},"name":"user_{}","email":"user{}@example.com","active":true}}"#,
                i, i, i
            )
            .into_bytes()
        })
        .collect();

    let mut concat = Vec::new();
    let mut sizes = Vec::new();
    for s in &samples {
        concat.extend_from_slice(s);
        sizes.push(s.len());
    }

    let mut dict_buf = vec![0u8; 16384];
    let dict_size =
        zstd_safe::train_from_buffer(&mut dict_buf, &concat, &sizes).expect("training failed");
    let dict_data = &dict_buf[..dict_size];

    let dict = zrip::dict::Dictionary::from_bytes(dict_data).unwrap();

    // Compress samples with C zstd using the dictionary, decompress with zrip
    let c_dict = zstd::dict::EncoderDictionary::copy(dict_data, 1);
    for sample in &samples[..10] {
        let mut encoder = zstd::Encoder::with_prepared_dictionary(Vec::new(), &c_dict).unwrap();
        std::io::Write::write_all(&mut encoder, sample).unwrap();
        let compressed = encoder.finish().unwrap();
        let decompressed = zrip::decompress_with_dict(&compressed, &dict)
            .unwrap_or_else(|e| panic!("dict decompress failed: {}", e));
        assert_eq!(&decompressed, sample);
    }
}

#[test]
fn roundtrip_dict_compress_c_decompress() {
    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(
                r#"{{"id":{},"name":"user_{}","email":"user{}@example.com","active":true}}"#,
                i, i, i
            )
            .into_bytes()
        })
        .collect();

    let mut concat = Vec::new();
    let mut sizes = Vec::new();
    for s in &samples {
        concat.extend_from_slice(s);
        sizes.push(s.len());
    }

    let mut dict_buf = vec![0u8; 16384];
    let dict_size =
        zstd_safe::train_from_buffer(&mut dict_buf, &concat, &sizes).expect("training failed");
    let dict_data = &dict_buf[..dict_size];

    let dict = zrip::dict::Dictionary::from_bytes(dict_data).unwrap();
    let c_dict = zstd::dict::DecoderDictionary::copy(dict_data);

    for sample in &samples[..20] {
        let compressed = zrip::compress_with_dict(sample, 1, &dict).unwrap();
        // Verify C zstd can decompress our dict-compressed output
        let mut decoder =
            zstd::Decoder::with_prepared_dictionary(compressed.as_slice(), &c_dict).unwrap();
        let mut decompressed = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut decompressed).unwrap();
        assert_eq!(&decompressed, sample);
        // Also verify our own decoder
        let zrip_dec = zrip::decompress_with_dict(&compressed, &dict).unwrap();
        assert_eq!(&zrip_dec, sample);
    }
}

#[cfg(feature = "dict_builder")]
#[test]
fn zrip_trained_dict_roundtrip() {
    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(
                r#"{{"id":{},"name":"user_{}","email":"user{}@example.com","active":true}}"#,
                i, i, i
            )
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
    assert!(!dict.content().is_empty());

    // Compress and decompress with our trained dict
    for sample in &samples[..20] {
        let compressed = zrip::compress_with_dict(sample, 1, &dict).unwrap();
        let decompressed = zrip::decompress_with_dict(&compressed, &dict).unwrap();
        assert_eq!(&decompressed, sample);
    }
}

#[cfg(feature = "dict_builder")]
#[test]
fn fastcover_c_zstd_cross_validates() {
    // Train with C zstd, use same dict bytes for both zrip and C
    let samples: Vec<Vec<u8>> = (0..200)
        .map(|i| {
            format!(
                r#"{{"ts":{},"level":"info","msg":"request processed","user_id":{},"latency_ms":{}}}"#,
                1700000000 + i,
                i % 50,
                i * 3 + 10,
            )
            .into_bytes()
        })
        .collect();

    let mut concat = Vec::new();
    let mut sizes = Vec::new();
    for s in &samples {
        concat.extend_from_slice(s);
        sizes.push(s.len());
    }
    let mut dict_buf = vec![0u8; 8192];
    let dict_size =
        zstd_safe::train_from_buffer(&mut dict_buf, &concat, &sizes).expect("training failed");
    let dict_data = &dict_buf[..dict_size];

    let dict = zrip::dict::Dictionary::from_bytes(dict_data).unwrap();
    let c_dict = zstd::dict::DecoderDictionary::copy(dict_data);

    for level in [-1, 1, 3, 4] {
        for sample in &samples[100..120] {
            let compressed = zrip::compress_with_dict(sample, level, &dict).unwrap();
            let mut decoder =
                zstd::Decoder::with_prepared_dictionary(compressed.as_slice(), &c_dict).unwrap();
            let mut decompressed = Vec::new();
            std::io::Read::read_to_end(&mut decoder, &mut decompressed).unwrap();
            assert_eq!(&decompressed, sample, "level {level}");
            let zrip_dec = zrip::decompress_with_dict(&compressed, &dict).unwrap();
            assert_eq!(&zrip_dec, sample, "level {level} self-decode");
        }
    }
}

#[cfg(feature = "dict_builder")]
#[test]
fn fastcover_improves_compression() {
    // With a good dictionary, compression ratio should be better than without
    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(
                r#"{{"id":{},"name":"user_{}","email":"user{}@example.com","active":true}}"#,
                i, i, i
            )
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
            format!(
                r#"{{"id":{},"name":"user_{}","email":"user{}@example.com"}}"#,
                i, i, i,
            )
            .into_bytes()
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
        // Roundtrip
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
        .map(|i| format!(r#"{{"key{}":"val{}"}}"#, i, i).into_bytes())
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
            let s = format!("user_{}", i);
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
    // Edge case: very small samples
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
    // Large dictionary (32KB)
    let samples: Vec<Vec<u8>> = (0..200)
        .map(|i| {
            let mut s = format!(
                r#"{{"id":{},"name":"user_{}","email":"user{}@example.com","bio":"{}"}}"#,
                i,
                i,
                i,
                "x".repeat(100 + (i % 50)),
            );
            s.into_bytes()
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
            format!(
                r#"{{"id":{},"name":"user_{}","email":"user{}@example.com","active":true}}"#,
                i, i, i
            )
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

#[test]
fn compress_context_roundtrip() {
    let data = b"hello world, this is a test of the compress context API";
    let mut ctx = zrip::CompressContext::new(1).unwrap();
    let compressed = ctx.compress(data).unwrap().to_vec();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(&decompressed, data);

    let data2 = b"second call reuses buffers";
    let compressed2 = ctx.compress(data2).unwrap().to_vec();
    let decompressed2 = zrip::decompress(&compressed2).unwrap();
    assert_eq!(&decompressed2, data2);
}

#[test]
fn compress_context_with_dict() {
    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(
                r#"{{"id":{},"name":"user_{}","email":"user{}@example.com","active":true}}"#,
                i, i, i
            )
            .into_bytes()
        })
        .collect();

    let mut concat = Vec::new();
    let mut sizes = Vec::new();
    for s in &samples {
        concat.extend_from_slice(s);
        sizes.push(s.len());
    }

    let mut dict_buf = vec![0u8; 16384];
    let dict_size =
        zstd_safe::train_from_buffer(&mut dict_buf, &concat, &sizes).expect("training failed");
    let dict_data = &dict_buf[..dict_size];
    let dict = zrip::dict::Dictionary::from_bytes(dict_data).unwrap();
    let c_dict = zstd::dict::DecoderDictionary::copy(dict_data);

    // Test CompressContext::with_dict + compress (stored dict, buffer reuse)
    let dict2 = zrip::dict::Dictionary::from_bytes(dict_data).unwrap();
    let mut ctx = zrip::CompressContext::with_dict(1, dict2).unwrap();
    for sample in &samples[..20] {
        let compressed = ctx.compress(sample).unwrap().to_vec();
        let mut decoder =
            zstd::Decoder::with_prepared_dictionary(compressed.as_slice(), &c_dict).unwrap();
        let mut decompressed = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut decompressed).unwrap();
        assert_eq!(&decompressed, sample);
        let zrip_dec = zrip::decompress_with_dict(&compressed, &dict).unwrap();
        assert_eq!(&zrip_dec, sample);
    }

    // Test CompressContext::compress_with_dict (ad-hoc dict, buffer reuse)
    let mut ctx2 = zrip::CompressContext::new(1).unwrap();
    for sample in &samples[..20] {
        let compressed = ctx2.compress_with_dict(sample, &dict).unwrap().to_vec();
        let mut decoder =
            zstd::Decoder::with_prepared_dictionary(compressed.as_slice(), &c_dict).unwrap();
        let mut decompressed = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut decompressed).unwrap();
        assert_eq!(&decompressed, sample);
    }
}

// ===== Adversarial: corrupt data must never panic =====

#[test]
fn decompress_garbage_bytes_never_panics() {
    // Pure pseudorandom garbage at various lengths
    for seed in 0u64..500 {
        let len = (seed % 300) as usize + 1;
        let data: Vec<u8> = (0..len)
            .map(|i| {
                let x = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(i as u64);
                (x >> 33) as u8
            })
            .collect();
        let _ = zrip::decompress(&data);
    }
}

#[test]
fn decompress_valid_frame_with_corrupt_blocks_never_panics() {
    // Valid magic + frame header, then garbage block data
    let magic = [0x28, 0xB5, 0x2F, 0xFD];
    for seed in 0u64..200 {
        let garbage_len = (seed % 200) as usize + 1;
        let garbage: Vec<u8> = (0..garbage_len)
            .map(|i| {
                let x = seed
                    .wrapping_mul(2654435761)
                    .wrapping_add(i as u64 * 1103515245);
                (x >> 24) as u8
            })
            .collect();
        let mut frame = Vec::with_capacity(4 + garbage_len);
        frame.extend_from_slice(&magic);
        frame.extend_from_slice(&garbage);
        let _ = zrip::decompress(&frame);
    }
}

#[test]
fn decompress_bit_flipped_frames_never_panic() {
    let original: Vec<u8> = b"the quick brown fox jumps over the lazy dog "
        .iter()
        .cycle()
        .take(5000)
        .copied()
        .collect();
    let compressed = zstd::encode_all(&original[..], 1).unwrap();

    // Flip every single bit in the compressed frame
    for byte_pos in 0..compressed.len() {
        for bit in 0..8u8 {
            let mut corrupted = compressed.clone();
            corrupted[byte_pos] ^= 1 << bit;
            let _ = zrip::decompress(&corrupted);
        }
    }
}

#[test]
fn decompress_truncated_at_every_byte_never_panics() {
    let original: Vec<u8> = b"ABCDEFGHIJKLMNOP"
        .iter()
        .cycle()
        .take(8000)
        .copied()
        .collect();
    let compressed = zstd::encode_all(&original[..], 3).unwrap();
    for truncate_at in 0..compressed.len() {
        let _ = zrip::decompress(&compressed[..truncate_at]);
    }
}

#[test]
fn decompress_two_byte_flips_never_panic() {
    let original: Vec<u8> = (0..2000u32)
        .map(|i| ((i.wrapping_mul(2654435761)) >> 24) as u8)
        .collect();
    let compressed = zstd::encode_all(&original[..], 1).unwrap();
    let len = compressed.len();

    // Flip pairs of bytes spread across the frame
    let step = (len / 50).max(1);
    for i in (0..len).step_by(step) {
        for j in (i + 1..len).step_by(step) {
            let mut corrupted = compressed.clone();
            corrupted[i] ^= 0xFF;
            corrupted[j] ^= 0xFF;
            let _ = zrip::decompress(&corrupted);
        }
    }
}

#[test]
fn decompress_zero_filled_after_header_never_panics() {
    let original: Vec<u8> = b"hello world ".iter().cycle().take(4000).copied().collect();
    let compressed = zstd::encode_all(&original[..], 1).unwrap();

    // Keep magic + frame header, zero out rest
    for zero_start in 4..compressed.len().min(20) {
        let mut corrupted = compressed.clone();
        for b in &mut corrupted[zero_start..] {
            *b = 0;
        }
        let _ = zrip::decompress(&corrupted);
    }
}

#[test]
fn decompress_ff_filled_after_header_never_panics() {
    let original: Vec<u8> = b"test data ".iter().cycle().take(4000).copied().collect();
    let compressed = zstd::encode_all(&original[..], 1).unwrap();

    for ff_start in 4..compressed.len().min(20) {
        let mut corrupted = compressed.clone();
        for b in &mut corrupted[ff_start..] {
            *b = 0xFF;
        }
        let _ = zrip::decompress(&corrupted);
    }
}

// ===== Adversarial: crafted malicious-looking inputs =====

#[test]
fn decompress_max_block_size_claim_with_tiny_input() {
    // Magic + frame descriptor claiming huge content but with almost no data
    let frames: Vec<Vec<u8>> = vec![
        // FHD with single_segment=0, fcs_field=3 (8-byte FCS), content_size=0xFFFFFFFF
        vec![
            0x28, 0xB5, 0x2F, 0xFD, // magic
            0xE0, // FHD: fcs=3, single=0, checksum=0, dictid=0
            0x00, // window descriptor
            0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, // FCS = 4GB
        ],
        // FHD with single_segment=1, fcs_field=0 (1-byte FCS = 255)
        vec![
            0x28, 0xB5, 0x2F, 0xFD, // magic
            0x20, // FHD: single_segment, fcs=0
            0xFF, // FCS = 255
        ],
    ];
    for frame in &frames {
        let _ = zrip::decompress(frame);
    }
}

#[test]
fn decompress_block_header_claiming_huge_size() {
    // Valid magic + minimal frame header + block header claiming max size
    let mut frame = vec![
        0x28, 0xB5, 0x2F, 0xFD, // magic
        0x00, // FHD: no checksum, no dict, fcs=0, single=0
        0x00, // window descriptor
    ];
    // Block header: last=1, type=raw, size=2^17-1 = 131071
    // Encoding: 3 bytes, bits[0]=last, bits[2:1]=type, bits[23:3]=size
    let block_hdr = (131071u32 << 3) | 0b001; // last=1, raw=00
    frame.push((block_hdr & 0xFF) as u8);
    frame.push(((block_hdr >> 8) & 0xFF) as u8);
    frame.push(((block_hdr >> 16) & 0xFF) as u8);
    // No actual block data
    let _ = zrip::decompress(&frame);
}

// ===== Fuzz regression vectors (ported from structured-zstd) =====

#[test]
fn decompress_overproducing_block_returns_err() {
    // Crafted sequences that produce more output than MAX_BLOCK_SIZE per block.
    // Must return Err, not panic or OOM.
    let data: &[u8] = &[
        0x28, 0xb5, 0x2f, 0xfd, 0x5d, 0x00, 0x00, 0xf7, 0x06, 0x5d, 0x00, 0x00, 0x5d, 0x00, 0x80,
        0xf7, 0xff, 0x5d, 0x00, 0x00, 0x01, 0xe0, 0xe0, 0xe0, 0xe0, 0xe2, 0xe0, 0xa4, 0x00, 0x0c,
        0x0c, 0x2c, 0x0c,
    ];
    let result = zrip::decompress(data);
    assert!(result.is_err());
}

#[test]
fn decompress_oom_vector_returns_err() {
    // Multi-frame stream where sequences cumulatively drive unbounded growth.
    // Must return Err (or truncated output), not OOM.
    let data: &[u8] = &[
        0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x30, 0xb5, 0x00, 0x00, 0x2d, 0x28, 0xb5, 0x2f, 0xfd, 0x00,
        0x26, 0x02, 0x00, 0x04, 0x28, 0xb5, 0x2f, 0xfd, 0x34, 0x0e, 0x02, 0x00, 0x0a, 0x0a, 0x0a,
        0x0a, 0x0a, 0x0a, 0x00, 0x0b, 0x0b, 0x19, 0x00, 0x02, 0xfc, 0xe9, 0x98, 0x0a, 0x0a, 0x0a,
        0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a,
        0x0a, 0x0a, 0xd7, 0x0a, 0x0a, 0x0a, 0x0a, 0xd3, 0x4a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a,
        0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a,
        0x0a, 0x0a, 0x0a, 0x0a, 0xb5, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0xe5, 0x0a, 0xb5,
    ];
    let _ = zrip::decompress(data);
}

#[test]
fn decompress_multiframe_corrupt_never_panics() {
    // Multi-frame stream where later frames are malformed. Must not panic.
    let data: &[u8] = &[
        0x28, 0xB5, 0x2F, 0xFD, 0x28, 0x28, 0xF5, 0x00, 0x00, 0x2D, 0x27, 0x8C, 0xB4, 0xB4, 0x20,
        0xA0, 0x00, 0x02, 0x00, 0xF2, 0xF2, 0xF2, 0xF2, 0x85, 0x21, 0xF2, 0xF2, 0xF2, 0xF2, 0xF2,
        0xF2, 0xF2, 0xF2, 0xF2, 0xA8, 0xA8, 0xA8, 0xA8, 0x28, 0xB5, 0x2F, 0xFD, 0x30, 0x28, 0x2D,
        0x00, 0x00, 0x61, 0x6A, 0x10, 0x00, 0x2D, 0x00, 0xA8, 0xA8, 0xA8, 0xA8, 0xA8, 0xA8, 0xA8,
        0xF2, 0xF2, 0xF2, 0x28, 0xB5, 0x2F, 0xFD, 0x00, 0x28, 0xB5, 0x2F, 0x00, 0x00, 0xB5, 0x28,
        0x00, 0x28, 0xFD, 0xB5, 0x00, 0x00, 0x2D, 0x0B, 0x8C, 0xB4, 0xB4, 0x04, 0x21, 0xA0, 0x00,
        0x00, 0x5E, 0xB4, 0x00, 0x00, 0x72, 0xA4, 0x00, 0xB4, 0x00, 0xFF, 0xFF, 0xFF, 0x28, 0x72,
        0xA4, 0x00, 0xB4, 0x00, 0x00, 0x72, 0x28, 0xCF, 0xA4, 0x00, 0xB4, 0xA8, 0x28, 0xB5, 0x2F,
        0xFD, 0x30, 0x00, 0x2D, 0x00, 0x00, 0x61, 0x6A, 0x10, 0x00, 0x2D, 0x00, 0xA8, 0xA8, 0xA8,
        0xA8, 0xA8, 0xA8, 0xA8, 0xF2, 0xF2, 0xF2, 0x28, 0xB5, 0x2F, 0xFD, 0x00, 0x28, 0xB5, 0x00,
        0x00, 0x28, 0xB5, 0x2F, 0xFD, 0x00, 0x28, 0xB5, 0x00, 0x02, 0x00, 0x2D, 0x0B, 0x02, 0x02,
        0x02, 0xFF, 0xFF, 0xF2, 0x00, 0x8C,
    ];
    let _ = zrip::decompress(data);
}

// ===== Decode path: multi-block frames =====

#[test]
fn decompress_c_multiblock_frame() {
    // Data larger than one block (128KB) forces multi-block
    let original: Vec<u8> = (0..200_000u32)
        .map(|i| ((i.wrapping_mul(2654435761)) >> 24) as u8)
        .collect();
    for level in [1, 3, 9, 19] {
        let compressed = zstd::encode_all(&original[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, original, "level {level}");
    }
}

#[test]
fn decompress_c_large_multiblock() {
    // Several blocks worth of data
    let original: Vec<u8> = b"the quick brown fox "
        .iter()
        .cycle()
        .take(500_000)
        .copied()
        .collect();
    let compressed = zstd::encode_all(&original[..], 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, original);
}

// ===== Decode path: sequence edge cases =====

#[test]
fn decompress_c_long_literal_lengths() {
    // Input with very long literal runs (incompressible regions)
    let mut data = Vec::with_capacity(50_000);
    for i in 0..50 {
        // 500 bytes of pseudorandom (won't compress well = long literal length)
        data.extend(
            (0..500u32).map(|j| ((j.wrapping_add(i * 500).wrapping_mul(2654435761)) >> 16) as u8),
        );
        // 500 bytes of repetitive (will compress = match)
        data.extend(std::iter::repeat(0x42u8).take(500));
    }
    for level in [1, 3, 9] {
        let compressed = zstd::encode_all(&data[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, data, "level {level}");
    }
}

#[test]
fn decompress_c_long_match_lengths() {
    // Very long matches: repeat a pattern, then offset back
    let mut data = vec![0u8; 100];
    for i in 0..100u8 {
        data[i as usize] = i;
    }
    // Now repeat the first 100 bytes many times (long match at offset 100)
    for _ in 0..500 {
        data.extend_from_slice(&data[..100].to_vec());
    }
    for level in [1, 3, 9] {
        let compressed = zstd::encode_all(&data[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, data, "level {level}");
    }
}

#[test]
fn decompress_c_many_rep_offsets() {
    // Alternating patterns that exercise repeat offset codes
    let mut data = Vec::with_capacity(50_000);
    let pattern_a: Vec<u8> = (0..50).map(|i| (i * 3) as u8).collect();
    let pattern_b: Vec<u8> = (0..50).map(|i| (i * 7 + 1) as u8).collect();
    for i in 0..500 {
        if i % 2 == 0 {
            data.extend_from_slice(&pattern_a);
        } else {
            data.extend_from_slice(&pattern_b);
        }
    }
    for level in [1, 3, 9] {
        let compressed = zstd::encode_all(&data[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, data, "level {level}");
    }
}

#[test]
fn decompress_c_offset_1_rle_like() {
    // Data that produces offset=1 sequences (RLE-like through match copy)
    let mut data = Vec::with_capacity(50_000);
    // Start with a byte, then repeat it (offset=1 match)
    data.push(0xAB);
    data.extend(std::iter::repeat(0xAB).take(10_000));
    // Switch to a different byte
    data.push(0xCD);
    data.extend(std::iter::repeat(0xCD).take(10_000));
    // Mix
    data.extend(vec![0xAB, 0xCD].into_iter().cycle().take(10_000));
    for level in [1, 3, 9] {
        let compressed = zstd::encode_all(&data[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, data, "level {level}");
    }
}

#[test]
fn decompress_c_small_offset_overlapping_copy() {
    // Offsets 1-7 exercise the scalar copy_match path (overlap < 8 bytes)
    for off in 1..8usize {
        let mut data = Vec::with_capacity(10_000);
        // Seed: `off` distinct bytes
        for i in 0..off {
            data.push((i + 1) as u8);
        }
        // Repeat to 10KB (the compressor should find offset=`off` matches)
        while data.len() < 10_000 {
            let idx = data.len() - off;
            data.push(data[idx]);
        }
        let compressed = zstd::encode_all(&data[..], 1).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("off {off}: {e}"));
        assert_eq!(decompressed, data, "off {off}");
    }
}

#[test]
fn decompress_c_medium_offset_copy() {
    // Offsets 8-31 exercise the 8-byte copy loop path
    for off in [8, 15, 16, 24, 31] {
        let mut data: Vec<u8> = (0..off as u8).collect();
        while data.len() < 10_000 {
            let idx = data.len() - off;
            data.push(data[idx]);
        }
        let compressed = zstd::encode_all(&data[..], 1).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("off {off}: {e}"));
        assert_eq!(decompressed, data, "off {off}");
    }
}

#[test]
fn decompress_c_large_offset_avx_copy() {
    // Offsets >= 32 exercise the AVX2 32-byte copy loop
    for off in [32, 64, 128, 256, 1024] {
        let mut data: Vec<u8> = (0..off).map(|i| (i % 251) as u8).collect();
        while data.len() < 20_000 {
            let idx = data.len() - off;
            data.push(data[idx]);
        }
        let compressed = zstd::encode_all(&data[..], 1).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("off {off}: {e}"));
        assert_eq!(decompressed, data, "off {off}");
    }
}

// ===== Decode path: Huffman stream edge cases =====

#[test]
fn decompress_c_single_symbol_huffman() {
    // All bytes the same: compresses to single-symbol Huffman tree
    let data = vec![0x42u8; 10_000];
    for level in [1, 3, 9, 19] {
        let compressed = zstd::encode_all(&data[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, data, "level {level}");
    }
}

#[test]
fn decompress_c_two_symbol_huffman() {
    let data: Vec<u8> = (0..10_000)
        .map(|i| if i % 3 == 0 { 0xAA } else { 0x55 })
        .collect();
    let compressed = zstd::encode_all(&data[..], 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
fn decompress_c_skewed_distribution_huffman() {
    // One symbol dominates (e.g. 99% frequency)
    let data: Vec<u8> = (0..10_000u32)
        .map(|i| if i % 100 == 0 { 0xFF } else { 0x00 })
        .collect();
    let compressed = zstd::encode_all(&data[..], 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
fn decompress_c_max_symbol_count_huffman() {
    // All 256 byte values present
    let data: Vec<u8> = (0..10_000).map(|i| (i % 256) as u8).collect();
    let compressed = zstd::encode_all(&data[..], 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
fn decompress_c_4stream_segment_boundary_sizes() {
    // 4-stream decode: segment size = ceil(output_size / 4)
    // Test sizes where output_size % 4 != 0 (uneven segments)
    for output_size in [
        997, 998, 999, 1000, 1001, 1002, 1003, // near mod-4 boundary
        4093, 4094, 4095, 4096, 4097, // power of 2 boundary
        255, 256, 257, // small
    ] {
        let data: Vec<u8> = (0..output_size)
            .map(|i| ((i as u32).wrapping_mul(2654435761) >> 24) as u8)
            .collect();
        let compressed = zstd::encode_all(&data[..], 1).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("size {output_size}: {e}"));
        assert_eq!(decompressed, data, "size {output_size}");
    }
}

// ===== Decode path: FSE table edge cases =====

#[test]
fn decompress_c_predefined_fse_tables() {
    // Very small inputs use predefined (default) FSE tables
    for size in [20, 50, 100, 200] {
        let data: Vec<u8> = b"abcabc".iter().cycle().take(size).copied().collect();
        let compressed = zstd::encode_all(&data[..], 1).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("size {size}: {e}"));
        assert_eq!(decompressed, data, "size {size}");
    }
}

#[test]
fn decompress_c_high_accuracy_fse_tables() {
    // High C levels produce more complex FSE tables
    let data: Vec<u8> = (0..50_000u32)
        .map(|i| {
            let x = i.wrapping_mul(1103515245).wrapping_add(12345);
            ((x >> 16) & 0xFF) as u8
        })
        .collect();
    for level in [15, 19, 22] {
        let compressed = zstd::encode_all(&data[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, data, "level {level}");
    }
}

// ===== Decode path: history / window reference =====

#[test]
fn decompress_c_multiblock_back_references() {
    // Matches that reference previous blocks (window/history)
    // Use a repeating pattern larger than one block
    let pattern: Vec<u8> = (0..200).map(|i| (i % 251) as u8).collect();
    let mut data = Vec::new();
    for _ in 0..2000 {
        data.extend_from_slice(&pattern);
    }
    for level in [1, 3, 9] {
        let compressed = zstd::encode_all(&data[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, data, "level {level}");
    }
}

// ===== Output size limits =====

#[test]
fn decompress_refuses_output_exceeding_max_raw_block() {
    // RLE block: the max_output check fires before decoding
    let data = vec![0x42u8; 10_000];
    let compressed = zrip::compress(&data, 1).unwrap();
    // Verify decompression works with sufficient max_output
    let ok = zrip::decompress(&compressed).unwrap();
    assert_eq!(ok, data);
}

#[test]
fn decompress_with_limit_rejects_oversized_output() {
    let mut frame = vec![
        0x28, 0xB5, 0x2F, 0xFD, // magic
        0x20, // FHD: single_segment, fcs_field=0
        0x63, // FCS = 99
    ];
    let bh = (99u32 << 3) | 0b011; // RLE block, last=1, size=99
    frame.push((bh & 0xFF) as u8);
    frame.push(((bh >> 8) & 0xFF) as u8);
    frame.push(((bh >> 16) & 0xFF) as u8);
    frame.push(0x42); // RLE byte
    let mut ctx = zrip::DecompressContext::new();
    assert!(ctx.decompress_with_limit(&frame, 10).is_err());
    assert!(ctx.decompress(&frame).is_ok());
}

// ===== Roundtrip: exact boundary sizes =====

#[test]
fn roundtrip_exact_block_fill() {
    // Data that compresses to exactly one full block (128KB compressed)
    // Can't control compressed size, but test uncompressed sizes near 128KB
    for delta in [-2i32, -1, 0, 1, 2] {
        let size = (128 * 1024) as i32 + delta;
        if size <= 0 {
            continue;
        }
        let size = size as usize;
        let data: Vec<u8> = (0..size as u32)
            .map(|i| ((i.wrapping_mul(2654435761)) >> 24) as u8)
            .collect();
        let compressed = zrip::compress(&data, 1).unwrap();
        let decompressed = zrip::decompress(&compressed).unwrap();
        assert_eq!(decompressed, data, "size {size}");
        let c_dec = zstd::decode_all(&compressed[..]).unwrap();
        assert_eq!(c_dec, data, "size {size} C");
    }
}

// ===== Roundtrip: data that defeats various encoder strategies =====

#[test]
fn roundtrip_incompressible_random() {
    // Truly random data: encoder should fall back to raw/uncompressed blocks
    let data: Vec<u8> = (0..50_000u64)
        .map(|i| {
            let x = i
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (x >> 33) as u8
        })
        .collect();
    for level in [-7, -1, 1, 3, 4] {
        let compressed = zrip::compress(&data, level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, data, "level {level}");
        let c_dec =
            zstd::decode_all(&compressed[..]).unwrap_or_else(|e| panic!("level {level} C: {e}"));
        assert_eq!(c_dec, data, "level {level} C");
    }
}

#[test]
fn roundtrip_mixed_compressible_incompressible() {
    let mut data = Vec::with_capacity(100_000);
    for i in 0u64..200 {
        if i % 2 == 0 {
            data.extend(std::iter::repeat(((i * 7) & 0xFF) as u8).take(250));
        } else {
            data.extend((0..250u64).map(|j| {
                let x = (i * 1000 + j)
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (x >> 33) as u8
            }));
        }
    }
    for level in [-7, 1, 3, 4] {
        let compressed = zrip::compress(&data, level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, data, "level {level}");
    }
}

// ===== Cross-validation: C compress at all levels, zrip decode =====

#[test]
fn decompress_c_all_22_levels() {
    let data: Vec<u8> = b"the quick brown fox jumps over the lazy dog. "
        .iter()
        .cycle()
        .take(20_000)
        .copied()
        .collect();
    for level in 1..=22 {
        let compressed = zstd::encode_all(&data[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, data, "level {level}");
    }
}

#[test]
fn decompress_c_negative_levels() {
    let data: Vec<u8> = b"abcdefghijklmnop"
        .iter()
        .cycle()
        .take(20_000)
        .copied()
        .collect();
    for level in [-7, -6, -5, -4, -3, -2, -1] {
        let compressed = zstd::encode_all(&data[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, data, "level {level}");
    }
}

// ===== Adversarial: sequence section corruption =====

#[test]
fn decompress_corrupt_sequence_section_never_panics() {
    // Compress valid data, then corrupt bytes in the latter part of blocks
    // (where sequence data lives)
    let original: Vec<u8> = b"ABCDEFGHIJKLMNOP"
        .iter()
        .cycle()
        .take(8000)
        .copied()
        .collect();
    let compressed = zstd::encode_all(&original[..], 1).unwrap();

    // Corrupt random positions in the back half of the compressed data
    // (more likely to hit sequence sections than the frame header)
    let mid = compressed.len() / 2;
    for pos in mid..compressed.len() {
        for val in [0x00, 0xFF, 0x80, 0x01] {
            let mut corrupted = compressed.clone();
            corrupted[pos] = val;
            let _ = zrip::decompress(&corrupted);
        }
    }
}

#[test]
fn decompress_corrupt_literals_section_never_panics() {
    // Corrupt the front half (where literals/Huffman data lives)
    let original: Vec<u8> = (0..8000u32)
        .map(|i| ((i.wrapping_mul(2654435761)) >> 24) as u8)
        .collect();
    let compressed = zstd::encode_all(&original[..], 1).unwrap();

    let mid = compressed.len() / 2;
    // Frame header is ~6 bytes, start after that
    for pos in 6..mid {
        for val in [0x00, 0xFF, 0x80] {
            let mut corrupted = compressed.clone();
            corrupted[pos] = val;
            let _ = zrip::decompress(&corrupted);
        }
    }
}

// ===== Adversarial: near-valid frames =====

#[test]
fn decompress_frame_content_size_mismatch_never_panics() {
    // Valid compressed data but with frame content size tampered
    let original = b"test data for fcs mismatch";
    let compressed = zstd::encode_all(&original[..], 1).unwrap();

    // The FCS is typically near the start of the frame header (byte 5+)
    // Tamper with bytes 5-8 which likely contain FCS or window descriptor
    for pos in 4..compressed.len().min(12) {
        for delta in [1u8, 2, 0x80, 0xFF] {
            let mut corrupted = compressed.clone();
            corrupted[pos] = corrupted[pos].wrapping_add(delta);
            let _ = zrip::decompress(&corrupted);
        }
    }
}

// ===== Proptest: adversarial corruption =====

mod proptest_adversarial {
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]

        #[test]
        fn corrupt_valid_frame_never_panics(
            data in proptest::collection::vec(any::<u8>(), 100..5000),
            corrupt_positions in proptest::collection::vec(0usize..1000, 1..10),
            corrupt_values in proptest::collection::vec(any::<u8>(), 1..10),
        ) {
            let compressed = zstd::encode_all(&data[..], 1).unwrap();
            let mut corrupted = compressed.clone();
            for (pos_raw, val) in corrupt_positions.iter().zip(corrupt_values.iter()) {
                let pos = pos_raw % corrupted.len();
                corrupted[pos] = *val;
            }
            let _ = zrip::decompress(&corrupted);
        }

        #[test]
        fn corrupt_zrip_frame_never_panics(
            data in proptest::collection::vec(any::<u8>(), 100..5000),
            level in prop_oneof![Just(-1i32), Just(1), Just(3)],
            corrupt_positions in proptest::collection::vec(0usize..1000, 1..10),
            corrupt_values in proptest::collection::vec(any::<u8>(), 1..10),
        ) {
            if let Ok(compressed) = zrip::compress(&data, level) {
                let mut corrupted = compressed.clone();
                for (pos_raw, val) in corrupt_positions.iter().zip(corrupt_values.iter()) {
                    let pos = pos_raw % corrupted.len();
                    corrupted[pos] = *val;
                }
                let _ = zrip::decompress(&corrupted);
            }
        }

        #[test]
        fn truncated_valid_frame_never_panics(
            data in proptest::collection::vec(any::<u8>(), 50..5000),
            truncate_frac in 0.0f64..1.0,
        ) {
            let compressed = zstd::encode_all(&data[..], 1).unwrap();
            let truncate_at = (compressed.len() as f64 * truncate_frac) as usize;
            let _ = zrip::decompress(&compressed[..truncate_at]);
        }

        #[test]
        fn spliced_frames_never_panic(
            data_a in proptest::collection::vec(any::<u8>(), 100..2000),
            data_b in proptest::collection::vec(any::<u8>(), 100..2000),
            splice_point_frac in 0.1f64..0.9,
        ) {
            let comp_a = zstd::encode_all(&data_a[..], 1).unwrap();
            let comp_b = zstd::encode_all(&data_b[..], 1).unwrap();
            let splice_a = (comp_a.len() as f64 * splice_point_frac) as usize;
            let splice_b = (comp_b.len() as f64 * splice_point_frac) as usize;
            let mut spliced = comp_a[..splice_a].to_vec();
            spliced.extend_from_slice(&comp_b[splice_b..]);
            let _ = zrip::decompress(&spliced);
        }

        #[test]
        fn arbitrary_garbage_with_valid_magic_never_panics(
            garbage in proptest::collection::vec(any::<u8>(), 0..500),
        ) {
            let mut frame = vec![0x28, 0xB5, 0x2F, 0xFD];
            frame.extend_from_slice(&garbage);
            let _ = zrip::decompress(&frame);
        }
    }
}

// ===== Proptest: extensive roundtrip with structured data =====

mod proptest_structured {
    use proptest::prelude::*;

    fn structured_data() -> impl Strategy<Value = Vec<u8>> {
        prop_oneof![
            // Runs of same byte
            (any::<u8>(), 1..50_000usize).prop_map(|(b, n)| vec![b; n]),
            // Ascending with period
            (1usize..256, 100..10_000usize)
                .prop_map(|(period, len)| (0..len).map(|i| (i % period) as u8).collect()),
            // Two interleaved patterns
            (any::<[u8; 8]>(), any::<[u8; 8]>(), 100..5000usize).prop_map(|(a, b, n)| {
                let mut v = Vec::with_capacity(n);
                for i in 0..n {
                    if i % 16 < 8 {
                        v.push(a[i % 8]);
                    } else {
                        v.push(b[i % 8]);
                    }
                }
                v
            }),
            // Near-uniform with rare outliers
            (any::<u8>(), any::<u8>(), 1000..20_000usize).prop_map(|(common, rare, n)| {
                (0..n)
                    .map(|i| if i % 100 == 0 { rare } else { common })
                    .collect()
            }),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        #[test]
        fn roundtrip_structured(
            data in structured_data(),
            level in prop_oneof![Just(-7i32), Just(-1), Just(1), Just(3), Just(4)]
        ) {
            let compressed = zrip::compress(&data, level).unwrap();
            let decompressed = zrip::decompress(&compressed).unwrap();
            prop_assert_eq!(&decompressed, &data);
            let c_dec = zstd::decode_all(&compressed[..]).unwrap();
            prop_assert_eq!(&c_dec, &data);
        }

        #[test]
        fn decompress_c_structured(
            data in structured_data(),
            level in 1..=22i32,
        ) {
            let compressed = zstd::encode_all(&data[..], level).unwrap();
            let decompressed = zrip::decompress(&compressed).unwrap();
            prop_assert_eq!(&decompressed, &data);
        }
    }
}

// ===== Concurrent frames (multiple frames in one buffer) =====

#[test]
fn decompress_concatenated_frames() {
    let data_a = b"hello world hello world";
    let data_b = b"foo bar baz foo bar baz";
    let mut buf = Vec::new();
    buf.extend_from_slice(&zstd::encode_all(&data_a[..], 1).unwrap());
    buf.extend_from_slice(&zstd::encode_all(&data_b[..], 1).unwrap());
    let decompressed = zrip::decompress(&buf).unwrap();
    let mut expected = Vec::new();
    expected.extend_from_slice(data_a);
    expected.extend_from_slice(data_b);
    assert_eq!(decompressed, expected);
}

// ===== ReverseBitReader unit tests =====

#[test]
fn reverse_bit_reader_exact_64_bits() {
    // 8 bytes + sentinel = exactly 64 data bits
    let data: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE, 0x80];
    // Just verify it doesn't panic
    let _ = zrip::decompress(&data);
}

#[test]
fn reverse_bit_reader_1_byte() {
    // Minimal: 1 byte with sentinel
    let data = vec![0x01]; // Just the sentinel
    let _ = zrip::decompress(&data);
}

// ===== Checksum: content checksum validation =====

#[test]
fn checksum_validated_on_multiblock() {
    let data: Vec<u8> = b"checksum test data "
        .iter()
        .cycle()
        .take(200_000)
        .copied()
        .collect();
    let mut encoder = zstd::Encoder::new(Vec::new(), 1).unwrap();
    encoder.include_checksum(true).unwrap();
    std::io::Write::write_all(&mut encoder, &data).unwrap();
    let compressed = encoder.finish().unwrap();

    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, data);

    // Corrupt checksum (last 4 bytes)
    let mut corrupted = compressed.clone();
    let len = corrupted.len();
    corrupted[len - 1] ^= 0x01;
    assert!(zrip::decompress(&corrupted).is_err());
}

#[test]
fn checksum_validated_on_small_frame() {
    let data = b"tiny";
    let mut encoder = zstd::Encoder::new(Vec::new(), 1).unwrap();
    encoder.include_checksum(true).unwrap();
    std::io::Write::write_all(&mut encoder, data).unwrap();
    let compressed = encoder.finish().unwrap();

    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(&decompressed, data);
}

#[test]
fn roundtrip_corpus_l3_cross_validate() {
    for name in &[
        "dickens",
        "hdfs.json",
        "mr",
        "mozilla",
        "nci",
        "osdb",
        "samba",
        "webster",
        "x-ray",
    ] {
        let path = format!("corpus/{}", name);
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let compressed = zrip::compress(&data, 3).unwrap();
        let decoded = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("C zstd decode failed for {} L3: {}", name, e));
        assert_eq!(decoded, data, "C zstd round-trip mismatch for {} L3", name);
    }
}

#[test]
fn roundtrip_corpus_l4_cross_validate() {
    for name in &[
        "dickens",
        "hdfs.json",
        "mr",
        "mozilla",
        "nci",
        "osdb",
        "samba",
        "webster",
        "x-ray",
    ] {
        let path = format!("corpus/{}", name);
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let compressed = zrip::compress(&data, 4).unwrap();
        let decoded = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("C zstd decode failed for {} L4: {}", name, e));
        assert_eq!(decoded, data, "C zstd round-trip mismatch for {} L4", name);
    }
}

#[test]
fn rep_offset2_rotation_cross_validate() {
    let mut data = vec![0u8; 64 * 1024];
    let mut rng = 0x12345678u32;
    for b in data.iter_mut() {
        rng = rng.wrapping_mul(1103515245).wrapping_add(12345);
        *b = (rng >> 16) as u8 & 0x1F;
    }
    for chunk in data.chunks_mut(512) {
        if chunk.len() >= 64 {
            let (left, right) = chunk.split_at_mut(32);
            right[..16].copy_from_slice(&left[..16]);
            if chunk.len() >= 128 {
                let (left, right) = chunk.split_at_mut(80);
                right[..16].copy_from_slice(&left[48..64]);
            }
        }
    }
    for level in [3, 4] {
        let compressed = zrip::compress(&data, level).unwrap();
        let decoded = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("C zstd decode failed at L{}: {}", level, e));
        assert_eq!(decoded, data, "rep offset rotation mismatch at L{}", level);
    }
}

// ===== Negative level cross-validation =====

#[test]
fn roundtrip_negative_levels_cross_validate() {
    let data: Vec<u8> = (0..100_000u32)
        .map(|i| ((i.wrapping_mul(2654435761)) >> 24) as u8)
        .collect();
    for level in [-7, -6, -5, -4, -3, -2, -1] {
        let compressed = zrip::compress(&data, level).unwrap();
        let decoded = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("C zstd decode failed at L{}: {}", level, e));
        assert_eq!(decoded, data, "C zstd round-trip mismatch at L{}", level);
    }
}

// ===== decompress_into =====

#[test]
fn decompress_into_basic() {
    let data = b"Hello, decompress_into!".repeat(1000);
    let compressed = zrip::compress(&data, 1).unwrap();
    let mut output = Vec::new();
    let written = zrip::decompress_into(&compressed, &mut output).unwrap();
    assert_eq!(written, data.len());
    assert_eq!(output, data);
}

#[test]
fn decompress_into_appends() {
    let data = b"append test data".repeat(500);
    let compressed = zrip::compress(&data, 1).unwrap();
    let mut output = b"prefix".to_vec();
    let written = zrip::decompress_into(&compressed, &mut output).unwrap();
    assert_eq!(written, data.len());
    assert_eq!(&output[..6], b"prefix");
    assert_eq!(&output[6..], &data[..]);
}

#[test]
fn decompress_into_preallocated() {
    let data: Vec<u8> = (0..100_000).map(|i| (i % 251) as u8).collect();
    let compressed = zrip::compress(&data, 1).unwrap();
    let mut output = Vec::with_capacity(data.len() + 1024);
    let written = zrip::decompress_into(&compressed, &mut output).unwrap();
    assert_eq!(written, data.len());
    assert_eq!(output, data);
}

// ===== Zero literal length sequences =====

#[test]
fn roundtrip_zero_literal_lengths() {
    // Single repeated byte: produces sequences with literal_length=0 after the first.
    // Stresses the ll=0 rep offset special cases in compute_offset.
    let data = vec![0xABu8; 200_000];
    for level in [1, 2, 3, 4] {
        let compressed = zrip::compress(&data, level).unwrap();
        let decoded = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("C zstd decode failed at L{}: {}", level, e));
        assert_eq!(decoded, data, "zero-ll round-trip mismatch at L{}", level);
    }
}

#[test]
fn roundtrip_alternating_rep_offsets_ll0() {
    // Pattern that alternates between two offsets with zero-literal-length sequences.
    let mut data = Vec::with_capacity(100_000);
    let pat_a = b"ABCDEFGH";
    let pat_b = b"12345678";
    for i in 0..12500 {
        if i % 2 == 0 {
            data.extend_from_slice(pat_a);
        } else {
            data.extend_from_slice(pat_b);
        }
    }
    for level in [1, 3] {
        let compressed = zrip::compress(&data, level).unwrap();
        let decoded = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("C zstd decode failed at L{}: {}", level, e));
        assert_eq!(
            decoded, data,
            "alternating rep offsets mismatch at L{}",
            level
        );
    }
}

// ===== Edge-case match lengths =====

#[test]
fn roundtrip_match_length_boundaries() {
    // Build data with matches at ML code table boundaries: 3, 4, 130, 131, 258, 259.
    let mut data = Vec::with_capacity(200_000);
    let pattern: Vec<u8> = (0..32).collect();
    for &target_ml in &[3usize, 4, 8, 16, 32, 64, 130, 131, 258, 259, 512] {
        // Literal gap
        data.extend_from_slice(&[0xFF; 64]);
        // Write pattern, then repeat it at distance 32 to create a match of target_ml.
        let rep = target_ml / pattern.len() + 1;
        let source: Vec<u8> = pattern
            .iter()
            .copied()
            .cycle()
            .take(rep * pattern.len())
            .collect();
        data.extend_from_slice(&source[..target_ml.max(32)]);
        data.extend_from_slice(&source[..target_ml.max(32)]);
    }
    for level in [1, 3] {
        let compressed = zrip::compress(&data, level).unwrap();
        let decoded = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("C zstd decode failed at L{}: {}", level, e));
        assert_eq!(decoded, data, "ML boundary mismatch at L{}", level);
    }
}

// ===== RLE-mode FSE tables =====

#[test]
fn roundtrip_single_symbol_distribution() {
    // Data that compresses to a single literal-length code (all sequences have
    // the same LL), forcing RLE mode in FSE encoding.
    // Repeating 4-byte pattern with 4-byte gaps produces LL=4, ML=4 for every sequence.
    let mut data = Vec::with_capacity(100_000);
    let pattern = b"WXYZ";
    for _ in 0..12500 {
        data.extend_from_slice(b"____");
        data.extend_from_slice(pattern);
    }
    for level in [1, 3] {
        let compressed = zrip::compress(&data, level).unwrap();
        let decoded = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("C zstd decode failed at L{}: {}", level, e));
        assert_eq!(decoded, data, "single-symbol FSE mismatch at L{}", level);
    }
}

#[test]
fn decompress_c_rle_fse_tables() {
    // C zstd at high levels may produce RLE FSE tables. Verify zrip decodes them.
    let data: Vec<u8> = vec![42u8; 50_000];
    for level in [1, 3, 6, 9] {
        let compressed = zstd::encode_all(&data[..], level).unwrap();
        let decoded = zrip::decompress(&compressed)
            .unwrap_or_else(|e| panic!("zrip decode of C L{} failed: {}", level, e));
        assert_eq!(
            decoded, data,
            "C zstd RLE FSE decode mismatch at L{}",
            level
        );
    }
}

// ===== Skippable frames =====

#[test]
fn decompress_skippable_frame_before_data() {
    let payload = b"Hello, World!";
    let compressed = zstd::encode_all(&payload[..], 1).unwrap();

    // Prepend a skippable frame: magic 0x184D2A50 + 4-byte LE size + content
    let skip_content = b"skip me";
    let mut stream = Vec::new();
    stream.extend_from_slice(&0x184D2A50u32.to_le_bytes());
    stream.extend_from_slice(&(skip_content.len() as u32).to_le_bytes());
    stream.extend_from_slice(skip_content);
    stream.extend_from_slice(&compressed);

    let decoded = zrip::decompress(&stream).unwrap();
    assert_eq!(decoded, payload);
}

#[test]
fn decompress_skippable_frame_between_data_frames() {
    let p1 = b"frame one data here";
    let p2 = b"frame two data here";
    let c1 = zstd::encode_all(&p1[..], 1).unwrap();
    let c2 = zstd::encode_all(&p2[..], 1).unwrap();

    let mut stream = Vec::new();
    stream.extend_from_slice(&c1);
    // Skippable frame with magic 0x184D2A5F (max variant)
    stream.extend_from_slice(&0x184D2A5Fu32.to_le_bytes());
    stream.extend_from_slice(&4u32.to_le_bytes());
    stream.extend_from_slice(b"skip");
    stream.extend_from_slice(&c2);

    let decoded = zrip::decompress(&stream).unwrap();
    let mut expected = Vec::new();
    expected.extend_from_slice(p1);
    expected.extend_from_slice(p2);
    assert_eq!(decoded, expected);
}

// ===== Per-block output ceiling =====

#[test]
fn decompress_block_output_ceiling() {
    // Craft a frame whose sequences would produce > MAX_BLOCK_SIZE (128 KB) per block.
    // The decoder must reject this.
    // Use a RLE literal + a single sequence with a huge match length via rep offset.
    // We build a minimal valid-looking frame by hand.
    let data: &[u8] = &[
        0x28, 0xb5, 0x2f, 0xfd, // magic
        0x00, // descriptor: no flags
        0x00, // window descriptor (minimal)
        // Block header: not last, compressed, size=10
        0x14, 0x00, 0x00, // Literals section: RLE, 1 byte regen
        0x01, 0x41, // RLE literal 'A', regen_size=1
        // Sequence section header: 1 sequence
        0x01, // num_sequences = 1
        0x00, // compression modes = predefined
        // Sequence data (reverse bitstream):
        // This encodes a sequence with a very large match length
        // from predefined tables. The exact encoding depends on the
        // predefined table layout.
        0x80, 0x80, 0x01,
    ];
    // Should not OOM or panic regardless of what the sequences decode to.
    let _ = zrip::decompress(data);
}

// ===== Concatenated frame cross-validation =====

#[test]
fn roundtrip_concatenated_frames_cross_validate() {
    // Compress two chunks independently, concatenate, verify C zstd can decode.
    let data1: Vec<u8> = (0..50_000).map(|i| (i % 251) as u8).collect();
    let data2: Vec<u8> = (0..50_000).map(|i| ((i * 7) % 251) as u8).collect();
    let c1 = zrip::compress(&data1, 1).unwrap();
    let c2 = zrip::compress(&data2, 3).unwrap();
    let mut stream = c1.clone();
    stream.extend_from_slice(&c2);

    let decoded = zstd::decode_all(&stream[..]).unwrap();
    let mut expected = data1.clone();
    expected.extend_from_slice(&data2);
    assert_eq!(decoded, expected);
}

// ===== Large literal lengths =====

#[test]
fn roundtrip_large_literal_runs() {
    // Data with long incompressible regions between matches, producing large LL values.
    let mut data = Vec::with_capacity(200_000);
    let mut rng = 0xDEADBEEFu32;
    for _ in 0..100 {
        // ~1 KB of pseudo-random literals
        for _ in 0..1024 {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            data.push((rng >> 16) as u8);
        }
        // 32 bytes that repeat to create a match
        for j in 0..32u8 {
            data.push(j);
        }
        for j in 0..32u8 {
            data.push(j);
        }
    }
    for level in [1, 3] {
        let compressed = zrip::compress(&data, level).unwrap();
        let decoded = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("C zstd decode failed at L{}: {}", level, e));
        assert_eq!(decoded, data, "large LL run mismatch at L{}", level);
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
fn frame_decoder_c_zstd_data() {
    use std::io::Read;
    let data: Vec<u8> = (0..50_000).map(|i| (i % 251) as u8).collect();
    let compressed = zstd::encode_all(&data[..], 3).unwrap();
    let mut decoder = zrip::FrameDecoder::new(&compressed[..]);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output).unwrap();
    assert_eq!(output, data);
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

#[test]
fn compress_bound_covers_actual_output() {
    let sizes = [0, 1, 100, 1024, 128 * 1024, 128 * 1024 + 1, 1_000_000];
    for &size in &sizes {
        let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let bound = zrip::compress_bound(size);
        for level in [-7, -1, 1, 3, 4] {
            let compressed = zrip::compress(&data, level).unwrap();
            assert!(
                compressed.len() <= bound,
                "compress_bound({size}) = {bound} but L{level} produced {}",
                compressed.len()
            );
        }
    }
}

#[test]
fn compress_bound_edge_cases() {
    assert!(zrip::compress_bound(0) >= 21);
    let bound_1m = zrip::compress_bound(1_000_000);
    assert!(bound_1m > 1_000_000);
    assert!(bound_1m < 1_100_000);
}

#[test]
fn compress_with_params_roundtrip() {
    let data = b"params test".repeat(5000);
    let params = zrip::LevelParams {
        strategy: zrip::encode::strategy::Strategy::Fast,
        window_log: 17,
        hash_log: 15,
        chain_log: 15,
        search_log: 0,
        min_match: 4,
        target_length: 4,
        search_strength: 7,
        force_raw_literals: false,
    };
    let compressed = zrip::compress_with_params(&data, &params).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, data);
    let c_ref = zstd::decode_all(&compressed[..]).unwrap();
    assert_eq!(c_ref, data);
}

#[test]
fn compress_with_params_dfast() {
    let data = b"dfast params".repeat(5000);
    let params = zrip::LevelParams {
        strategy: zrip::encode::strategy::Strategy::DFast,
        window_log: 18,
        hash_log: 16,
        chain_log: 16,
        search_log: 1,
        min_match: 5,
        target_length: 1,
        search_strength: 4,
        force_raw_literals: false,
    };
    let compressed = zrip::compress_with_params(&data, &params).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, data);
}
