// Pure zrip roundtrip tests (no C zstd dependency).

#[test]
fn roundtrip_zrip_compress_decompress() {
    let original: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    let compressed = zrip::compress(&original, 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn roundtrip_all_levels_repetitive() {
    let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(200_000).copied().collect();
    for level in [-7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4] {
        let compressed = zrip::compress(&original, level).unwrap();
        let decompressed = zrip::decompress(&compressed)
            .unwrap_or_else(|e| panic!("level {level} zrip decompress: {e}"));
        assert_eq!(decompressed, original, "level {level} zrip roundtrip");
    }
}

#[test]
fn roundtrip_all_levels_random() {
    let original: Vec<u8> = (0..100_000u32)
        .map(|i| ((i.wrapping_mul(2_654_435_761)) >> 24) as u8)
        .collect();
    for level in [-7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4] {
        let compressed = zrip::compress(&original, level).unwrap();
        let decompressed = zrip::decompress(&compressed)
            .unwrap_or_else(|e| panic!("level {level} zrip decompress: {e}"));
        assert_eq!(decompressed, original, "level {level} zrip roundtrip");
    }
}

#[test]
fn roundtrip_zeros() {
    let original = vec![0u8; 100_000];
    let compressed = zrip::compress(&original, 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn roundtrip_empty() {
    let compressed = zrip::compress(b"", 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, b"");
}

#[test]
fn roundtrip_small() {
    for size in [1, 7, 15, 31, 63, 127, 255, 512, 1024] {
        let original: Vec<u8> = (0u8..=255).cycle().take(size).collect();
        let compressed = zrip::compress(&original, 1).unwrap();
        let decompressed = zrip::decompress(&compressed)
            .unwrap_or_else(|e| panic!("size {size}: decompress failed: {e}"));
        assert_eq!(decompressed, original, "size {size}");
    }
}

#[test]
fn roundtrip_large() {
    let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(200_000).copied().collect();
    let compressed = zrip::compress(&original, 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn roundtrip_all_same_bytes() {
    for b in [0u8, 1, 127, 128, 254, 255] {
        let original = vec![b; 50_000];
        let compressed = zrip::compress(&original, 1).unwrap();
        let decompressed = zrip::decompress(&compressed).unwrap();
        assert_eq!(decompressed, original, "byte {b}");
    }
}

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
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("size {size}: {e}"));
        assert_eq!(decompressed, original, "size {size}");
    }
}

#[test]
fn roundtrip_single_bytes() {
    for b in 0u8..=255 {
        let original = vec![b];
        let compressed = zrip::compress(&original, 1).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("byte {b:#04x}: {e}"));
        assert_eq!(decompressed, original, "byte {b:#04x}");
    }
}

#[test]
fn roundtrip_size_sweep() {
    for exp in 0..=17 {
        let size = 1usize << exp;
        let original: Vec<u8> = b"ABCDEFGH".iter().cycle().take(size).copied().collect();
        let compressed = zrip::compress(&original, 1).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("size {size}: {e}"));
        assert_eq!(decompressed, original, "size {size}");
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
            let decompressed = zrip::decompress(&compressed)
                .unwrap_or_else(|e| panic!("pattern={name} level={level} zrip rt: {e}"));
            assert_eq!(&decompressed, data, "pattern={name} level={level} zrip rt");
        }
    }
}

#[test]
fn roundtrip_exact_block_fill() {
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
        let decompressed = zrip::decompress(&compressed).unwrap();
        assert_eq!(decompressed, data, "size {size}");
    }
}

#[test]
fn roundtrip_incompressible_random() {
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
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, data, "level {level}");
    }
}

#[test]
fn roundtrip_mixed_compressible_incompressible() {
    let mut data = Vec::with_capacity(100_000);
    for i in 0u64..200 {
        if i % 2 == 0 {
            data.extend(std::iter::repeat_n(((i * 7) & 0xFF) as u8, 250));
        } else {
            data.extend((0..250u64).map(|j| {
                let x = (i * 1000 + j)
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
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

// ===== Zero literal length sequences =====

#[test]
fn roundtrip_zero_literal_lengths() {
    let data = vec![0xABu8; 200_000];
    for level in [1, 2, 3, 4] {
        let compressed = zrip::compress(&data, level).unwrap();
        let decoded = zrip::decompress(&compressed)
            .unwrap_or_else(|e| panic!("decode failed at L{level}: {e}"));
        assert_eq!(decoded, data, "zero-ll round-trip mismatch at L{level}");
    }
}

#[test]
fn roundtrip_alternating_rep_offsets_ll0() {
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
        let decoded = zrip::decompress(&compressed)
            .unwrap_or_else(|e| panic!("decode failed at L{level}: {e}"));
        assert_eq!(
            decoded, data,
            "alternating rep offsets mismatch at L{level}"
        );
    }
}

// ===== Edge-case match lengths =====

#[test]
fn roundtrip_match_length_boundaries() {
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
        let decoded = zrip::decompress(&compressed)
            .unwrap_or_else(|e| panic!("decode failed at L{level}: {e}"));
        assert_eq!(decoded, data, "ML boundary mismatch at L{level}");
    }
}

// ===== RLE-mode FSE tables =====

#[test]
fn roundtrip_single_symbol_distribution() {
    let mut data = Vec::with_capacity(100_000);
    let pattern = b"WXYZ";
    for _ in 0..12500 {
        data.extend_from_slice(b"____");
        data.extend_from_slice(pattern);
    }
    for level in [1, 3] {
        let compressed = zrip::compress(&data, level).unwrap();
        let decoded = zrip::decompress(&compressed)
            .unwrap_or_else(|e| panic!("decode failed at L{level}: {e}"));
        assert_eq!(decoded, data, "single-symbol FSE mismatch at L{level}");
    }
}

// ===== Large literal lengths =====

#[test]
fn roundtrip_large_literal_runs() {
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
        let decoded = zrip::decompress(&compressed)
            .unwrap_or_else(|e| panic!("decode failed at L{level}: {e}"));
        assert_eq!(decoded, data, "large LL run mismatch at L{level}");
    }
}

// ===== Error handling =====

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
    let decompressed = zrip::decompress(&buf[..n]).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn compress_into_too_small() {
    let original = b"hello world hello world hello world";
    let mut buf = [0u8; 1];
    assert!(zrip::compress_into(original, &mut buf, 1).is_err());
}

// ===== Checksum =====

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
            .map(|i| ((i.wrapping_mul(2_654_435_761)) >> 24) as u8)
            .collect();
        let compressed = zrip::compress(&original, 1).unwrap();
        let decompressed = zrip::decompress(&compressed).unwrap();
        assert_eq!(decompressed, original, "size {size}");
    }
}

#[test]
fn checksum_validated_on_multiblock() {
    let data: Vec<u8> = b"checksum test data "
        .iter()
        .cycle()
        .take(200_000)
        .copied()
        .collect();
    let compressed = zrip::compress(&data, 1).unwrap();

    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, data);

    let mut corrupted = compressed.clone();
    let len = corrupted.len();
    corrupted[len - 1] ^= 0x01;
    assert!(zrip::decompress(&corrupted).is_err());
}

#[test]
fn checksum_validated_on_small_frame() {
    let data = b"tiny";
    let compressed = zrip::compress(data, 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(&decompressed, data);
}

// ===== compress_context =====

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

// ===== Output size limits =====

#[test]
fn decompress_refuses_output_exceeding_max_raw_block() {
    let data = vec![0x42u8; 10_000];
    let compressed = zrip::compress(&data, 1).unwrap();
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

// ===== ReverseBitReader unit tests =====

#[test]
fn reverse_bit_reader_exact_64_bits() {
    let data: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE, 0x80];
    let _ = zrip::decompress(&data);
}

#[test]
fn reverse_bit_reader_1_byte() {
    let data = vec![0x01];
    let _ = zrip::decompress(&data);
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

// ===== compress_bound =====

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

// ===== compress_with_params =====

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

// ===== Skippable frames =====

#[test]
fn decompress_skippable_frame_before_data() {
    let payload = b"Hello, World!";
    let compressed = zrip::compress(payload, 1).unwrap();

    let mut stream = Vec::new();
    stream.extend_from_slice(&0x184D_2A50_u32.to_le_bytes());
    stream.extend_from_slice(&(b"skip me".len() as u32).to_le_bytes());
    stream.extend_from_slice(b"skip me");
    stream.extend_from_slice(&compressed);

    let decoded = zrip::decompress(&stream).unwrap();
    assert_eq!(decoded, payload);
}

#[test]
fn decompress_skippable_frame_between_data_frames() {
    let p1 = b"frame one data here";
    let p2 = b"frame two data here";
    let c1 = zrip::compress(p1, 1).unwrap();
    let c2 = zrip::compress(p2, 1).unwrap();

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

// ===== Concatenated frames =====

#[test]
fn decompress_concatenated_frames() {
    let data_a = b"hello world hello world";
    let data_b = b"foo bar baz foo bar baz";
    let mut buf = Vec::new();
    buf.extend_from_slice(&zrip::compress(data_a, 1).unwrap());
    buf.extend_from_slice(&zrip::compress(data_b, 1).unwrap());
    let decompressed = zrip::decompress(&buf).unwrap();
    let mut expected = Vec::new();
    expected.extend_from_slice(data_a);
    expected.extend_from_slice(data_b);
    assert_eq!(decompressed, expected);
}

#[test]
fn roundtrip_concatenated_frames() {
    let data1: Vec<u8> = (0..50_000).map(|i| (i % 251) as u8).collect();
    let data2: Vec<u8> = (0..50_000).map(|i| ((i * 7) % 251) as u8).collect();
    let c1 = zrip::compress(&data1, 1).unwrap();
    let c2 = zrip::compress(&data2, 3).unwrap();
    let mut stream = c1.clone();
    stream.extend_from_slice(&c2);

    let decoded = zrip::decompress(&stream).unwrap();
    let mut expected = data1.clone();
    expected.extend_from_slice(&data2);
    assert_eq!(decoded, expected);
}

// ===== Truncated frame =====

#[test]
fn decompress_truncated_frame() {
    let original = b"hello world hello world hello world";
    let compressed = zrip::compress(original, 1).unwrap();
    for truncate_at in [1, 2, 3, 4, 5, compressed.len() / 2, compressed.len() - 1] {
        let truncated = &compressed[..truncate_at];
        assert!(
            zrip::decompress(truncated).is_err(),
            "should fail at truncate_at={truncate_at}"
        );
    }
}

// ===== XXH64 checksum =====

#[test]
fn xxh64_checksum_roundtrip() {
    let data = b"test data for checksum validation with zrip";
    let compressed = zrip::compress(data, 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, data);
}
