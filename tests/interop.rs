// C zstd cross-validation tests. Skipped under Miri (FFI unsupported).
#![cfg(not(miri))]

// ===== Encoder: zrip compress -> C decompress =====

#[test]
fn zrip_compress_c_decompress() {
    let original: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    let compressed = zrip::compress(&original, 1).unwrap();
    let decompressed = zstd::decode_all(&compressed[..]).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn roundtrip_all_levels_c_cross_validate() {
    let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(200_000).copied().collect();
    for level in [-7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4] {
        let compressed = zrip::compress(&original, level).unwrap();
        let c_decompressed = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("level {level} C decompress: {e}"));
        assert_eq!(c_decompressed, original, "level {level} C roundtrip");
    }
}

#[test]
fn roundtrip_all_levels_random_c_cross_validate() {
    let original: Vec<u8> = (0..100_000u32)
        .map(|i| ((i.wrapping_mul(2_654_435_761)) >> 24) as u8)
        .collect();
    for level in [-7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4] {
        let compressed = zrip::compress(&original, level).unwrap();
        let c_decompressed = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("level {level} C decompress: {e}"));
        assert_eq!(c_decompressed, original, "level {level} C roundtrip");
    }
}

#[test]
fn roundtrip_zeros_c_cross_validate() {
    let original = vec![0u8; 100_000];
    let compressed = zrip::compress(&original, 1).unwrap();
    let c_decompressed = zstd::decode_all(&compressed[..]).unwrap();
    assert_eq!(c_decompressed, original);
}

#[test]
fn roundtrip_empty_c_cross_validate() {
    let compressed = zrip::compress(b"", 1).unwrap();
    let c_decompressed = zstd::decode_all(&compressed[..]).unwrap();
    assert_eq!(c_decompressed, b"");
}

#[test]
fn roundtrip_all_same_bytes_c_cross_validate() {
    for b in [0u8, 1, 127, 128, 254, 255] {
        let original = vec![b; 50_000];
        let compressed = zrip::compress(&original, 1).unwrap();
        let c_dec = zstd::decode_all(&compressed[..]).unwrap();
        assert_eq!(c_dec, original, "byte {b} C");
    }
}

#[test]
fn roundtrip_all_levels_all_patterns_c_cross_validate() {
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
                    (u64::from(i)
                        .wrapping_mul(6_364_136_223_846_793_005_u64)
                        .wrapping_add(1_442_695_040_888_963_407_u64)
                        >> 56) as u8
                })
                .collect(),
        ),
    ];

    for level in [-7, -5, -3, -1, 1, 2, 3, 4] {
        for (name, data) in &patterns {
            let compressed = zrip::compress(data, level).unwrap();
            let c_decompressed = zstd::decode_all(&compressed[..])
                .unwrap_or_else(|e| panic!("pattern={name} level={level} C: {e}"));
            assert_eq!(&c_decompressed, data, "pattern={name} level={level} C rt");
        }
    }
}

#[test]
fn roundtrip_exact_block_fill_c_cross_validate() {
    for delta in [-2i32, -1, 0, 1, 2] {
        let size = 128 * 1024 + delta;
        if size <= 0 {
            continue;
        }
        let size = size as usize;
        let data: Vec<u8> = (0..size as u32)
            .map(|i| ((i.wrapping_mul(2_654_435_761)) >> 24) as u8)
            .collect();
        let compressed = zrip::compress(&data, 1).unwrap();
        let c_dec = zstd::decode_all(&compressed[..]).unwrap();
        assert_eq!(c_dec, data, "size {size} C");
    }
}

#[test]
fn roundtrip_incompressible_random_c_cross_validate() {
    let data: Vec<u8> = (0..50_000u64)
        .map(|i| {
            let x = i
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (x >> 33) as u8
        })
        .collect();
    for level in [-7, -1, 1, 3, 4] {
        let compressed = zrip::compress(&data, level).unwrap();
        let c_dec =
            zstd::decode_all(&compressed[..]).unwrap_or_else(|e| panic!("level {level} C: {e}"));
        assert_eq!(c_dec, data, "level {level} C");
    }
}

#[test]
fn roundtrip_zero_literal_lengths_c_cross_validate() {
    let data = vec![0xABu8; 200_000];
    for level in [1, 2, 3, 4] {
        let compressed = zrip::compress(&data, level).unwrap();
        let decoded = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("C zstd decode failed at L{level}: {e}"));
        assert_eq!(decoded, data, "zero-ll round-trip mismatch at L{level}");
    }
}

#[test]
fn roundtrip_alternating_rep_offsets_ll0_c_cross_validate() {
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
            .unwrap_or_else(|e| panic!("C zstd decode failed at L{level}: {e}"));
        assert_eq!(
            decoded, data,
            "alternating rep offsets mismatch at L{level}"
        );
    }
}

#[test]
fn roundtrip_match_length_boundaries_c_cross_validate() {
    let mut data = Vec::with_capacity(200_000);
    let pattern: Vec<u8> = (0..32).collect();
    for &target_ml in &[3usize, 4, 8, 16, 32, 64, 130, 131, 258, 259, 512] {
        data.extend_from_slice(&[0xFF; 64]);
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
            .unwrap_or_else(|e| panic!("C zstd decode failed at L{level}: {e}"));
        assert_eq!(decoded, data, "ML boundary mismatch at L{level}");
    }
}

#[test]
fn roundtrip_single_symbol_distribution_c_cross_validate() {
    let mut data = Vec::with_capacity(100_000);
    let pattern = b"WXYZ";
    for _ in 0..12500 {
        data.extend_from_slice(b"____");
        data.extend_from_slice(pattern);
    }
    for level in [1, 3] {
        let compressed = zrip::compress(&data, level).unwrap();
        let decoded = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("C zstd decode failed at L{level}: {e}"));
        assert_eq!(decoded, data, "single-symbol FSE mismatch at L{level}");
    }
}

#[test]
fn roundtrip_large_literal_runs_c_cross_validate() {
    let mut data = Vec::with_capacity(200_000);
    let mut rng = 0xDEAD_BEEF_u32;
    for _ in 0..100 {
        for _ in 0..1024 {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            data.push((rng >> 16) as u8);
        }
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
            .unwrap_or_else(|e| panic!("C zstd decode failed at L{level}: {e}"));
        assert_eq!(decoded, data, "large LL run mismatch at L{level}");
    }
}

#[test]
fn roundtrip_concatenated_frames_c_cross_validate() {
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

#[test]
fn roundtrip_block_boundary_sizes_c_cross_validate() {
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
fn roundtrip_single_bytes_c_cross_validate() {
    for b in 0u8..=255 {
        let original = vec![b];
        let compressed = zrip::compress(&original, 1).unwrap();
        let decompressed =
            zstd::decode_all(&compressed[..]).unwrap_or_else(|e| panic!("byte {b:#04x}: {e}"));
        assert_eq!(decompressed, original, "byte {b:#04x}");
    }
}

#[test]
fn roundtrip_size_sweep_c_cross_validate() {
    for exp in 0..=17 {
        let size = 1usize << exp;
        let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(size).copied().collect();
        let compressed = zrip::compress(&original, 1).unwrap();
        let decompressed =
            zstd::decode_all(&compressed[..]).unwrap_or_else(|e| panic!("size {size}: {e}"));
        assert_eq!(decompressed, original, "size {size}");
    }
}

#[test]
fn compress_with_params_roundtrip_c_cross_validate() {
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
    let c_ref = zstd::decode_all(&compressed[..]).unwrap();
    assert_eq!(c_ref, data);
}

#[test]
fn compress_into_c_cross_validate() {
    let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(10000).copied().collect();
    let mut buf = vec![0u8; original.len() + 100];
    let n = zrip::compress_into(&original, &mut buf, 1).unwrap();
    let decompressed = zstd::decode_all(&buf[..n]).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn rep_offset2_rotation_cross_validate() {
    let mut data = vec![0u8; 64 * 1024];
    let mut rng = 0x1234_5678_u32;
    for b in &mut data {
        rng = rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
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
            .unwrap_or_else(|e| panic!("C zstd decode failed at L{level}: {e}"));
        assert_eq!(decoded, data, "rep offset rotation mismatch at L{level}");
    }
}

#[test]
fn roundtrip_negative_levels_cross_validate() {
    let data: Vec<u8> = (0..100_000u32)
        .map(|i| ((i.wrapping_mul(2_654_435_761)) >> 24) as u8)
        .collect();
    for level in [-7, -6, -5, -4, -3, -2, -1] {
        let compressed = zrip::compress(&data, level).unwrap();
        let decoded = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("C zstd decode failed at L{level}: {e}"));
        assert_eq!(decoded, data, "C zstd round-trip mismatch at L{level}");
    }
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
        let path = format!("corpus/{name}");
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        let compressed = zrip::compress(&data, 3).unwrap();
        let decoded = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("C zstd decode failed for {name} L3: {e}"));
        assert_eq!(decoded, data, "C zstd round-trip mismatch for {name} L3");
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
        let path = format!("corpus/{name}");
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        let compressed = zrip::compress(&data, 4).unwrap();
        let decoded = zstd::decode_all(&compressed[..])
            .unwrap_or_else(|e| panic!("C zstd decode failed for {name} L4: {e}"));
        assert_eq!(decoded, data, "C zstd round-trip mismatch for {name} L4");
    }
}

// ===== Decoder: C compress -> zrip decompress =====

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
        .map(|i| ((i.wrapping_mul(2_654_435_761)) >> 24) as u8)
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
                    (u64::from(i)
                        .wrapping_mul(6_364_136_223_846_793_005_u64)
                        .wrapping_add(1_442_695_040_888_963_407_u64)
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
fn decompress_c_block_boundary_sizes() {
    for size in [128 * 1024 - 1, 128 * 1024, 128 * 1024 + 1, 200_000] {
        let original: Vec<u8> = (0..size as u32)
            .map(|i| ((i.wrapping_mul(2_654_435_761)) >> 24) as u8)
            .collect();
        let compressed = zstd::encode_all(&original[..], 1).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("size {size}: {e}"));
        assert_eq!(decompressed, original, "size {size}");
    }
}

#[test]
fn decompress_c_high_entropy() {
    let original: Vec<u8> = (0..50_000u32)
        .map(|i| {
            let x = i
                .wrapping_mul(2_654_435_761)
                .wrapping_add(i.wrapping_mul(1_103_515_245));
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
            original.extend(std::iter::repeat_n(0x42u8, 250));
        } else {
            original.extend((0..250u32).map(|j| ((j + i).wrapping_mul(2_654_435_761) >> 24) as u8));
        }
    }
    for level in [1, 3, 9] {
        let compressed = zstd::encode_all(&original[..], level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, original, "level {level}");
    }
}

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
                .map(|i| ((i.wrapping_mul(2_654_435_761)) >> 24) as u8)
                .collect();
            let compressed = zstd::encode_all(&original[..], 1).unwrap();
            let decompressed =
                zrip::decompress(&compressed).unwrap_or_else(|e| panic!("size {s}: {e}"));
            assert_eq!(decompressed, original, "size {s}");
        }
    }
}

#[test]
fn decompress_c_multiblock_frame() {
    let original: Vec<u8> = (0..200_000u32)
        .map(|i| ((i.wrapping_mul(2_654_435_761)) >> 24) as u8)
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

#[test]
fn decompress_c_long_literal_lengths() {
    let mut data = Vec::with_capacity(50_000);
    for i in 0..50 {
        data.extend(
            (0..500u32)
                .map(|j| ((j.wrapping_add(i * 500).wrapping_mul(2_654_435_761)) >> 16) as u8),
        );
        data.extend(std::iter::repeat_n(0x42u8, 500));
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
    let mut data = vec![0u8; 100];
    for i in 0..100u8 {
        data[i as usize] = i;
    }
    for _ in 0..500 {
        let chunk = data[..100].to_vec();
        data.extend_from_slice(&chunk);
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
    let mut data = Vec::with_capacity(50_000);
    data.push(0xAB);
    data.extend(std::iter::repeat_n(0xAB, 10_000));
    data.push(0xCD);
    data.extend(std::iter::repeat_n(0xCD, 10_000));
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
    for off in 1..8usize {
        let mut data = Vec::with_capacity(10_000);
        for i in 0..off {
            data.push((i + 1) as u8);
        }
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

#[test]
fn decompress_c_single_symbol_huffman() {
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
    let data: Vec<u8> = (0..10_000u32)
        .map(|i| if i % 100 == 0 { 0xFF } else { 0x00 })
        .collect();
    let compressed = zstd::encode_all(&data[..], 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
fn decompress_c_max_symbol_count_huffman() {
    let data: Vec<u8> = (0..10_000).map(|i| (i % 256) as u8).collect();
    let compressed = zstd::encode_all(&data[..], 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
fn decompress_c_4stream_segment_boundary_sizes() {
    for output_size in [
        997, 998, 999, 1000, 1001, 1002, 1003, 4093, 4094, 4095, 4096, 4097, 255, 256, 257,
    ] {
        let data: Vec<u8> = (0..output_size)
            .map(|i| ((i as u32).wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        let compressed = zstd::encode_all(&data[..], 1).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("size {output_size}: {e}"));
        assert_eq!(decompressed, data, "size {output_size}");
    }
}

#[test]
fn decompress_c_predefined_fse_tables() {
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
    let data: Vec<u8> = (0..50_000u32)
        .map(|i| {
            let x = i.wrapping_mul(1_103_515_245).wrapping_add(12345);
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

#[test]
fn decompress_c_multiblock_back_references() {
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

#[test]
fn decompress_c_rle_fse_tables() {
    let data: Vec<u8> = vec![42u8; 50_000];
    for level in [1, 3, 6, 9] {
        let compressed = zstd::encode_all(&data[..], level).unwrap();
        let decoded = zrip::decompress(&compressed)
            .unwrap_or_else(|e| panic!("zrip decode of C L{level} failed: {e}"));
        assert_eq!(decoded, data, "C zstd RLE FSE decode mismatch at L{level}");
    }
}

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

// ===== Streaming interop =====

#[test]
fn streaming_encoder_c_decompress() {
    let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(10000).copied().collect();
    let mut encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    std::io::Write::write_all(&mut encoder, &original).unwrap();
    let compressed = encoder.finish().unwrap();
    let decompressed = zstd::decode_all(&compressed[..]).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn streaming_encoder_chunked_c_decompress() {
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
fn streaming_encoder_empty_c_decompress() {
    let encoder = zrip::FrameEncoder::new(Vec::new(), 1).unwrap();
    let compressed = encoder.finish().unwrap();
    let decompressed = zstd::decode_all(&compressed[..]).unwrap();
    assert_eq!(decompressed, b"");
}

#[test]
fn streaming_encoder_all_levels_c_decompress() {
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
fn streaming_decoder_multiframe_seq_tables_reset() {
    use std::io::Read;

    let data1 = b"The quick brown fox jumps over the lazy dog. ".repeat(500);
    let compressed1 = zstd::encode_all(data1.as_slice(), 9).unwrap();

    let data2 = b"Simple data repeated many times for testing. ".repeat(100);
    let compressed2 = zstd::encode_all(data2.as_slice(), 1).unwrap();

    let mut multi = compressed1.clone();
    multi.extend_from_slice(&compressed2);

    let mut decoder = zrip::FrameDecoder::new(multi.as_slice());
    let mut output = Vec::new();
    decoder.read_to_end(&mut output).unwrap();

    let mut expected = data1.clone();
    expected.extend_from_slice(&data2);
    assert_eq!(output, expected);
}

#[test]
fn streaming_encoder_reset_c_zstd_interop() {
    use std::io::Write;
    let (samples, dict_data) = make_dict_samples();
    let dict = zrip::dict::Dictionary::from_bytes(&dict_data).unwrap();
    let c_dict = zstd::dict::DecoderDictionary::copy(&dict_data);

    let mut encoder = zrip::FrameEncoder::with_dict(Vec::new(), 1, dict).unwrap();
    for sample in &samples[..5] {
        encoder.write_all(sample).unwrap();
        let compressed = encoder.reset(Vec::new()).unwrap();
        let mut c_dec =
            zstd::Decoder::with_prepared_dictionary(compressed.as_slice(), &c_dict).unwrap();
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut c_dec, &mut out).unwrap();
        assert_eq!(&out, sample);
    }
}

// ===== Dictionary interop =====

fn make_dict_samples() -> (Vec<Vec<u8>>, Vec<u8>) {
    let samples: Vec<Vec<u8>> = (0..100)
        .map(|i| {
            format!(r#"{{"id":{i},"name":"user_{i}","email":"user{i}@example.com","active":true}}"#)
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
    dict_buf.truncate(dict_size);
    (samples, dict_buf)
}

#[test]
fn parse_c_trained_dictionary() {
    let (_, dict_data) = make_dict_samples();
    let dict = zrip::dict::Dictionary::from_bytes(&dict_data).unwrap();
    assert_ne!(dict.id(), 0);
    assert!(!dict.content().is_empty());
    assert!(dict.rep_offsets()[0] > 0);
    assert!(dict.rep_offsets()[1] > 0);
    assert!(dict.rep_offsets()[2] > 0);
}

#[test]
fn decompress_c_with_dictionary() {
    let (samples, dict_data) = make_dict_samples();
    let dict = zrip::dict::Dictionary::from_bytes(&dict_data).unwrap();

    let c_dict = zstd::dict::EncoderDictionary::copy(&dict_data, 1);
    for sample in &samples[..10] {
        let mut encoder = zstd::Encoder::with_prepared_dictionary(Vec::new(), &c_dict).unwrap();
        std::io::Write::write_all(&mut encoder, sample).unwrap();
        let compressed = encoder.finish().unwrap();
        let decompressed = zrip::decompress_with_dict(&compressed, &dict)
            .unwrap_or_else(|e| panic!("dict decompress failed: {e}"));
        assert_eq!(&decompressed, sample);
    }
}

#[test]
fn roundtrip_dict_compress_c_decompress() {
    let (samples, dict_data) = make_dict_samples();
    let dict = zrip::dict::Dictionary::from_bytes(&dict_data).unwrap();
    let c_dict = zstd::dict::DecoderDictionary::copy(&dict_data);

    for sample in &samples[..20] {
        let compressed = zrip::compress_with_dict(sample, 1, &dict).unwrap();
        let mut decoder =
            zstd::Decoder::with_prepared_dictionary(compressed.as_slice(), &c_dict).unwrap();
        let mut decompressed = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut decompressed).unwrap();
        assert_eq!(&decompressed, sample);
        let zrip_dec = zrip::decompress_with_dict(&compressed, &dict).unwrap();
        assert_eq!(&zrip_dec, sample);
    }
}

#[test]
fn compress_context_with_dict_c_cross_validate() {
    let (samples, dict_data) = make_dict_samples();
    let dict = zrip::dict::Dictionary::from_bytes(&dict_data).unwrap();
    let c_dict = zstd::dict::DecoderDictionary::copy(&dict_data);

    let dict2 = zrip::dict::Dictionary::from_bytes(&dict_data).unwrap();
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

#[cfg(feature = "dict_builder")]
#[test]
fn fastcover_c_zstd_cross_validates() {
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

#[test]
fn streaming_encoder_dict_c_decompress() {
    use std::io::Write;
    let (samples, dict_data) = make_dict_samples();
    let dict = zrip::dict::Dictionary::from_bytes(&dict_data).unwrap();
    let c_dict = zstd::dict::DecoderDictionary::copy(&dict_data);

    for level in [-1, 1, 3, 4] {
        for sample in &samples[..10] {
            let mut encoder =
                zrip::FrameEncoder::with_dict(Vec::new(), level, dict.clone()).unwrap();
            encoder.write_all(sample).unwrap();
            let compressed = encoder.finish().unwrap();

            let mut decoder =
                zstd::Decoder::with_prepared_dictionary(compressed.as_slice(), &c_dict).unwrap();
            let mut decompressed = Vec::new();
            std::io::Read::read_to_end(&mut decoder, &mut decompressed).unwrap();
            assert_eq!(
                &decompressed, sample,
                "L{level} streaming encoder dict -> C decompress mismatch"
            );
        }
    }
}

#[test]
fn c_compress_dict_streaming_decoder() {
    use std::io::Read;
    let (samples, dict_data) = make_dict_samples();
    let dict = zrip::dict::Dictionary::from_bytes(&dict_data).unwrap();
    let c_dict = zstd::dict::EncoderDictionary::copy(&dict_data, 1);

    for sample in &samples[..10] {
        let mut encoder = zstd::Encoder::with_prepared_dictionary(Vec::new(), &c_dict).unwrap();
        std::io::Write::write_all(&mut encoder, sample).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut decoder = zrip::FrameDecoder::with_dict(compressed.as_slice(), dict.clone());
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();
        assert_eq!(&decompressed, sample);
    }
}

#[test]
fn streaming_dict_multiblock_c_cross_validate() {
    use std::io::Write;
    let (_, dict_data) = make_dict_samples();
    let dict = zrip::dict::Dictionary::from_bytes(&dict_data).unwrap();
    let c_dict = zstd::dict::DecoderDictionary::copy(&dict_data);

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

        let mut decoder =
            zstd::Decoder::with_prepared_dictionary(compressed.as_slice(), &c_dict).unwrap();
        let mut decompressed = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut decompressed).unwrap();
        assert_eq!(
            decompressed.len(),
            data.len(),
            "L{level} multiblock size mismatch"
        );
        assert_eq!(decompressed, data, "L{level} multiblock content mismatch");
    }
}

#[test]
fn streaming_dict_multiblock_reset_c_cross_validate() {
    use std::io::Write;
    let (_, dict_data) = make_dict_samples();
    let dict = zrip::dict::Dictionary::from_bytes(&dict_data).unwrap();
    let c_dict = zstd::dict::DecoderDictionary::copy(&dict_data);

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
        let mut c_dec =
            zstd::Decoder::with_prepared_dictionary(compressed.as_slice(), &c_dict).unwrap();
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut c_dec, &mut out).unwrap();
        assert_eq!(out, data);
    }
}

#[test]
fn streaming_decoder_dict_mismatch_c_trained() {
    use std::io::Read;
    let (samples, dict_data) = make_dict_samples();
    let dict = zrip::dict::Dictionary::from_bytes(&dict_data).unwrap();

    let compressed = zrip::compress_with_dict(&samples[0], 1, &dict).unwrap();

    let mut decoder = zrip::FrameDecoder::new(compressed.as_slice());
    let mut out = Vec::new();
    let err = decoder.read_to_end(&mut out).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

    let other_samples: Vec<Vec<u8>> = (0..100)
        .map(|i| format!("completely different data pattern {i} abcdefghijk").into_bytes())
        .collect();
    let mut concat2 = Vec::new();
    let mut sizes2 = Vec::new();
    for s in &other_samples {
        concat2.extend_from_slice(s);
        sizes2.push(s.len());
    }
    let mut dict_buf2 = vec![0u8; 16384];
    let dict_size2 =
        zstd_safe::train_from_buffer(&mut dict_buf2, &concat2, &sizes2).expect("training failed");
    let other_dict = zrip::dict::Dictionary::from_bytes(&dict_buf2[..dict_size2]).unwrap();

    if other_dict.id() != dict.id() {
        let mut decoder2 = zrip::FrameDecoder::with_dict(compressed.as_slice(), other_dict);
        let mut out2 = Vec::new();
        let err2 = decoder2.read_to_end(&mut out2).unwrap_err();
        assert_eq!(err2.kind(), std::io::ErrorKind::InvalidData);
    }
}

// ===== Skippable/concatenated frames with C zstd =====

#[test]
fn decompress_skippable_frame_before_c_data() {
    let payload = b"Hello, World!";
    let compressed = zstd::encode_all(&payload[..], 1).unwrap();

    let mut stream = Vec::new();
    stream.extend_from_slice(&0x184D_2A50_u32.to_le_bytes());
    stream.extend_from_slice(&(b"skip me".len() as u32).to_le_bytes());
    stream.extend_from_slice(b"skip me");
    stream.extend_from_slice(&compressed);

    let decoded = zrip::decompress(&stream).unwrap();
    assert_eq!(decoded, payload);
}

#[test]
fn decompress_skippable_frame_between_c_data_frames() {
    let p1 = b"frame one data here";
    let p2 = b"frame two data here";
    let c1 = zstd::encode_all(&p1[..], 1).unwrap();
    let c2 = zstd::encode_all(&p2[..], 1).unwrap();

    let mut stream = Vec::new();
    stream.extend_from_slice(&c1);
    stream.extend_from_slice(&0x184D_2A5F_u32.to_le_bytes());
    stream.extend_from_slice(&4u32.to_le_bytes());
    stream.extend_from_slice(b"skip");
    stream.extend_from_slice(&c2);

    let decoded = zrip::decompress(&stream).unwrap();
    let mut expected = Vec::new();
    expected.extend_from_slice(p1);
    expected.extend_from_slice(p2);
    assert_eq!(decoded, expected);
}

#[test]
fn decompress_concatenated_c_frames() {
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

// ===== Checksum with C zstd encoder =====

#[test]
fn checksum_validated_on_multiblock_c() {
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

    let mut corrupted = compressed.clone();
    let len = corrupted.len();
    corrupted[len - 1] ^= 0x01;
    assert!(zrip::decompress(&corrupted).is_err());
}

#[test]
fn checksum_validated_on_small_frame_c() {
    let data = b"tiny";
    let mut encoder = zstd::Encoder::new(Vec::new(), 1).unwrap();
    encoder.include_checksum(true).unwrap();
    std::io::Write::write_all(&mut encoder, data).unwrap();
    let compressed = encoder.finish().unwrap();

    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(&decompressed, data);
}
