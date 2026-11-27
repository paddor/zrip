// Pure zrip roundtrip tests (no C zstd dependency).

const BLOCK: usize = zrip::frame::MAX_BLOCK_SIZE; // 128 KiB
const KNUTH: u32 = 0x9E37_79B1; // xxHash PRIME32_1, used as cheap pseudo-random scatter

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
        .map(|i| ((i.wrapping_mul(KNUTH)) >> 24) as u8)
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
    for size in [BLOCK - 1, BLOCK, BLOCK + 1, 2 * BLOCK, 2 * BLOCK + 1] {
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
        let size = BLOCK as i32 + delta;
        if size <= 0 {
            continue;
        }
        let size = size as usize;
        let data: Vec<u8> = (0..size as u32)
            .map(|i| ((i.wrapping_mul(KNUTH)) >> 24) as u8)
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
    assert_eq!(result, Vec::<u8>::new());
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
    assert!(zrip::compress(b"hello", -9).is_err());
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
            .map(|i| ((i.wrapping_mul(KNUTH)) >> 24) as u8)
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

#[test]
fn compress_context_can_omit_content_checksum() {
    let data = b"enclosing format already authenticates this payload".repeat(1_000);
    let mut ctx = zrip::CompressContext::new(1).unwrap();
    assert!(ctx.content_checksum());

    ctx.set_content_checksum(false);
    let compressed = ctx.compress(&data).unwrap().to_vec();
    assert_eq!(compressed[4] & 0x04, 0);
    assert_eq!(zrip::decompress(&compressed).unwrap(), data);

    ctx.set_content_checksum(true);
    let compressed = ctx.compress(&data).unwrap().to_vec();
    assert_ne!(compressed[4] & 0x04, 0);
    assert_eq!(zrip::decompress(&compressed).unwrap(), data);
}

#[test]
fn options_can_omit_content_checksum() {
    use zrip::push::{EndOp, PushEncoder};

    let text = b"enclosing format already authenticates this payload ".repeat(20);
    let large: Vec<u8> = text.iter().cycle().take(300_000).copied().collect();
    let on = zrip::Options::default();
    let off = zrip::Options::default().content_checksum(false);
    for data in [&b""[..], b"x", &text, &large] {
        for level in [-7, 1, 3, 4] {
            // One pass: the same frame, less the flag and the 4 checksum bytes.
            let with = zrip::compress_opts(data, level, &on).unwrap();
            let without = zrip::compress_opts(data, level, &off).unwrap();
            assert_ne!(with[4] & 0x04, 0);
            assert_eq!(without[4] & 0x04, 0, "level {level}, {} bytes", data.len());
            assert_eq!(without.len() + 4, with.len());
            assert_eq!(zrip::decompress(&without).unwrap(), data);

            let mut pushed = Vec::new();
            let mut enc = PushEncoder::with_options(level, &off).unwrap();
            enc.encode_to_vec(data, &mut pushed, EndOp::End).unwrap();
            assert_eq!(pushed, without);

            // Streamed: no content size, so the frame is written block by block.
            let mut streamed = Vec::new();
            let mut enc = PushEncoder::with_options(level, &off).unwrap();
            for chunk in data.chunks(5000) {
                enc.encode_to_vec(chunk, &mut streamed, EndOp::Continue)
                    .unwrap();
            }
            enc.encode_to_vec(&[], &mut streamed, EndOp::End).unwrap();
            assert_eq!(streamed[4] & 0x04, 0);
            assert_eq!(zrip::decompress(&streamed).unwrap(), data);
            let mut with_streamed = Vec::new();
            let mut enc = PushEncoder::with_options(level, &on).unwrap();
            for chunk in data.chunks(5000) {
                enc.encode_to_vec(chunk, &mut with_streamed, EndOp::Continue)
                    .unwrap();
            }
            enc.encode_to_vec(&[], &mut with_streamed, EndOp::End)
                .unwrap();
            assert_eq!(streamed.len() + 4, with_streamed.len());
        }
    }
}

#[test]
fn compress_context_compress_into_matches_compress() {
    let text = b"caller-owned output buffers for compressed frames ".repeat(40);
    let large: Vec<u8> = text.iter().cycle().take(300_000).copied().collect();
    let mut inputs: Vec<&[u8]> = vec![b"", b"x", &text];
    inputs.push(&large);

    for level in [-7, -1, 1, 3, 4] {
        let mut ctx = zrip::CompressContext::new(level).unwrap();
        let mut ctx_into = zrip::CompressContext::new(level).unwrap();
        for input in &inputs {
            let expected = ctx.compress(input).unwrap().to_vec();
            let mut buf = vec![0u8; zrip::compress_bound(input.len())];
            let n = ctx_into.compress_into(input, &mut buf).unwrap();
            assert_eq!(&buf[..n], &expected[..], "L{level} len {}", input.len());
            assert_eq!(zrip::decompress(&buf[..n]).unwrap(), *input);
        }
    }
}

#[test]
fn compress_context_compress_into_writes_at_slice_start() {
    let data = b"frame written into the middle of a larger buffer".repeat(20);
    let mut ctx = zrip::CompressContext::new(1).unwrap();
    let mut buf = vec![0xAAu8; 16 + zrip::compress_bound(data.len()) + 16];
    let n = ctx.compress_into(&data, &mut buf[16..]).unwrap();
    assert!(buf[..16].iter().all(|&b| b == 0xAA));
    assert!(buf[16 + n..].iter().all(|&b| b == 0xAA));
    assert_eq!(zrip::decompress(&buf[16..16 + n]).unwrap(), data);
}

#[test]
fn compress_context_compress_into_rejects_small_output() {
    let data = b"not enough room for this frame".repeat(10);
    let mut ctx = zrip::CompressContext::new(1).unwrap();
    let frame_len = ctx.compress(&data).unwrap().len();

    let mut short = vec![0u8; frame_len - 1];
    assert!(matches!(
        ctx.compress_into(&data, &mut short),
        Err(zrip::CompressError::OutputTooSmall)
    ));

    let mut exact = vec![0u8; frame_len];
    assert_eq!(ctx.compress_into(&data, &mut exact).unwrap(), frame_len);
    assert_eq!(zrip::decompress(&exact).unwrap(), data);
}

#[test]
fn decompress_context_borrows_large_output_for_reuse() {
    let data = vec![0x42u8; 600 * 1024];
    let compressed = zrip::compress(&data, 1).unwrap();

    let mut ctx = zrip::DecompressContext::new();
    let decompressed = ctx.decompress(&compressed).unwrap();

    assert!(matches!(decompressed, std::borrow::Cow::Borrowed(_)));
    assert_eq!(&*decompressed, &data[..]);
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

#[cfg(feature = "std")]
#[test]
fn decompress_with_limit_rejects_oversized_concatenated_output() {
    let data_a = vec![b'a'; 100];
    let data_b = vec![b'b'; 100];
    let mut stream = zrip::compress(&data_a, 1).unwrap();
    stream.extend_from_slice(&zrip::compress(&data_b, 1).unwrap());

    assert!(zrip::decompress_with_limit(&stream, 199).is_err());
    assert_eq!(
        zrip::decompress_with_limit(&stream, 200).unwrap().len(),
        200
    );
}

#[test]
fn decompress_context_with_limit_rejects_oversized_concatenated_output() {
    let data_a = vec![b'a'; 100];
    let data_b = vec![b'b'; 100];
    let mut stream = zrip::compress(&data_a, 1).unwrap();
    stream.extend_from_slice(&zrip::compress(&data_b, 1).unwrap());

    let mut ctx = zrip::DecompressContext::new();
    assert!(ctx.decompress_with_limit(&stream, 199).is_err());
    assert_eq!(ctx.decompress_with_limit(&stream, 200).unwrap().len(), 200);
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

// ===== decompress_frame_into =====

fn single_frame_data() -> Vec<u8> {
    (0..20_000u32)
        .flat_map(|i| ((i * 7) % 251).to_le_bytes())
        .collect()
}

#[test]
fn decompress_frame_into_ignores_trailing_bytes() {
    let data = single_frame_data();
    let frame = zrip::compress(&data, 1).unwrap();
    for tail in [&[0u8][..], b"garbage after the frame", &[0xFF; 3]] {
        let input = [&frame[..], tail].concat();
        assert!(zrip::decompress(&input).is_err());
        let mut out = Vec::new();
        let decoded = zrip::decompress_frame_into(&input, &mut out, usize::MAX).unwrap();
        assert_eq!(
            decoded,
            zrip::DecodedFrame {
                consumed: frame.len(),
                produced: data.len()
            }
        );
        assert_eq!(out, data);
    }
}

#[test]
fn decompress_frame_into_decodes_only_the_first_frame() {
    let data = single_frame_data();
    let first = zrip::compress(&data, 1).unwrap();
    let second = zrip::compress(b"second frame", 3).unwrap();
    let both = [&first[..], &second[..]].concat();
    assert_eq!(
        zrip::frame::find_frame_compressed_size(&both),
        Ok(first.len())
    );

    let mut out = b"prefix".to_vec();
    let decoded = zrip::decompress_frame_into(&both, &mut out, usize::MAX).unwrap();
    assert_eq!(decoded.consumed, first.len());
    assert_eq!(decoded.produced, data.len());
    assert_eq!(&out[..6], b"prefix");
    assert_eq!(&out[6..], &data[..]);

    out.clear();
    let decoded =
        zrip::decompress_frame_into(&both[decoded.consumed..], &mut out, usize::MAX).unwrap();
    assert_eq!(decoded.consumed, second.len());
    assert_eq!(out, b"second frame");
}

#[test]
fn decompress_frame_into_skips_leading_skippable_frames() {
    let frame = zrip::compress(b"after the skippable frames", 1).unwrap();
    let mut input = Vec::new();
    for (magic, body) in [(0x184D_2A50u32, &b"meta"[..]), (0x184D_2A5F, &b""[..])] {
        input.extend_from_slice(&magic.to_le_bytes());
        input.extend_from_slice(&(body.len() as u32).to_le_bytes());
        input.extend_from_slice(body);
    }
    let skipped = input.len();
    input.extend_from_slice(&frame);
    input.extend_from_slice(&frame);

    let mut out = Vec::new();
    let decoded = zrip::decompress_frame_into(&input, &mut out, usize::MAX).unwrap();
    assert_eq!(decoded.consumed, skipped + frame.len());
    assert_eq!(out, b"after the skippable frames");

    // Only skippable frames, or nothing: no frame to decode.
    for input in [&input[..skipped], &[][..]] {
        let mut out = b"kept".to_vec();
        assert_eq!(
            zrip::decompress_frame_into(input, &mut out, usize::MAX),
            Err(zrip::DecompressError::InputExhausted)
        );
        assert_eq!(out, b"kept");
    }
}

#[test]
fn decompress_frame_into_checks_the_frame() {
    let data = single_frame_data();
    let frame = zrip::compress(&data, 1).unwrap();

    // Wrong checksum, then trailing bytes: the checksum still fails it.
    let mut bad = frame.clone();
    let n = bad.len();
    bad[n - 1] ^= 0xFF;
    bad.push(0);
    let mut out = b"kept".to_vec();
    assert!(matches!(
        zrip::decompress_frame_into(&bad, &mut out, usize::MAX),
        Err(zrip::DecompressError::ChecksumMismatch { .. })
    ));
    assert_eq!(out, b"kept");

    // A cut frame.
    assert_eq!(
        zrip::decompress_frame_into(&frame[..n - 1], &mut out, usize::MAX),
        Err(zrip::DecompressError::InputExhausted)
    );
    assert_eq!(
        zrip::decompress_frame_into(b"not a zstd frame", &mut out, usize::MAX),
        Err(zrip::DecompressError::BadMagic)
    );
    assert_eq!(out, b"kept");
}

#[test]
fn decompress_frame_into_limit() {
    let data = single_frame_data();
    let input = [&zrip::compress(&data, 1).unwrap()[..], &[0]].concat();
    let mut out = b"existing".to_vec();
    assert_eq!(
        zrip::decompress_frame_into(&input, &mut out, data.len() - 1),
        Err(zrip::DecompressError::OutputTooSmall)
    );
    assert_eq!(out, b"existing");
    // The limit counts only this call's output.
    let decoded = zrip::decompress_frame_into(&input, &mut out, data.len()).unwrap();
    assert_eq!(decoded.produced, data.len());
    assert_eq!(&out[8..], &data[..]);

    // Without a content size the limit still holds: three raw blocks of
    // 1000 bytes, a 128 KiB window, no checksum.
    let mut input = 0xFD2F_B528u32.to_le_bytes().to_vec();
    input.extend_from_slice(&[0x00, 7 << 3]);
    for i in 0..3u32 {
        let header = (1000 << 3) | u32::from(i == 2);
        input.extend_from_slice(&header.to_le_bytes()[..3]);
        input.extend_from_slice(&data[..1000]);
    }
    input.push(0);
    out.clear();
    assert_eq!(
        zrip::decompress_frame_into(&input, &mut out, 2999),
        Err(zrip::DecompressError::OutputTooSmall)
    );
    assert_eq!(out, b"");
    let decoded = zrip::decompress_frame_into(&input, &mut out, 3000).unwrap();
    assert_eq!(decoded.consumed, input.len() - 1);
    assert_eq!(out, [&data[..1000]; 3].concat());
}

#[test]
fn decompress_frame_into_context_with_dict() {
    let records: Vec<Vec<u8>> = (0..300)
        .map(|i| {
            format!(r#"{{"id":{i},"name":"user_{i}","email":"user{i}@example.com","active":true}}"#)
                .into_bytes()
        })
        .collect();
    let dict_bytes = zstd::dict::from_samples(&records, 4096).unwrap();
    let dict = zrip::Dictionary::from_bytes(&dict_bytes).unwrap();

    let mut ctx = zrip::DecompressContext::with_dict(dict.clone());
    let mut plain = zrip::DecompressContext::new();
    for record in &records[..20] {
        let frame = zrip::compress_with_dict(record, 1, &dict).unwrap();
        let input = [&frame[..], b"trailing"].concat();
        let mut out = Vec::new();
        let decoded = ctx
            .decompress_frame_into(&input, &mut out, 1 << 20)
            .unwrap();
        assert_eq!(decoded.consumed, frame.len());
        assert_eq!(decoded.produced, record.len());
        assert_eq!(&out, record);

        // Frames that name a dictionary need it.
        assert_eq!(
            plain.decompress_frame_into(&input, &mut Vec::new(), 1 << 20),
            Err(zrip::DecompressError::DictRequired)
        );
    }
}

// ===== Small-input decode path exercisers =====

#[test]
fn roundtrip_small_offset_matches() {
    for offset in 2..=8 {
        let mut data = vec![0u8; offset];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(37);
        }
        for _ in 0..20 {
            let tail: Vec<u8> = data[data.len() - offset..].to_vec();
            data.extend_from_slice(&tail);
        }
        let compressed = zrip::compress(&data, 1).unwrap();
        let decompressed = zrip::decompress(&compressed).unwrap();
        assert_eq!(decompressed, data, "offset={offset}");
    }
}

#[test]
fn roundtrip_rle_like() {
    let data = vec![0x42u8; 512];
    let compressed = zrip::compress(&data, 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
fn roundtrip_varied_literal_sizes() {
    let mut data = Vec::new();
    for chunk_size in [1, 2, 3, 7, 15, 16, 17, 31, 32, 33, 63] {
        let chunk: Vec<u8> = (0..chunk_size as u8).collect();
        data.extend_from_slice(&chunk);
        data.extend_from_slice(&chunk);
    }
    let compressed = zrip::compress(&data, 1).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, data);
}

// ===== compress_bound =====

#[test]
fn compress_bound_covers_actual_output() {
    let sizes = [0, 1, 100, 1024, BLOCK, BLOCK + 1, 1_000_000];
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
        #[cfg(feature = "ldm")]
        ldm_params: None,
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
        #[cfg(feature = "ldm")]
        ldm_params: None,
    };
    let compressed = zrip::compress_with_params(&data, &params).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
fn compress_with_params_clamps_degenerate_hash_log() {
    let data = b"abcdefghijklmnop".repeat(100);
    let mut params = zrip::encode::strategy::level_params(1).unwrap();
    params.hash_log = 0;
    params.chain_log = 0;
    let compressed = zrip::compress_with_params(&data, &params).unwrap();
    let decompressed = zrip::decompress(&compressed).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
fn compress_with_params_clamps_excessive_hash_log() {
    let data = b"abcdefghijklmnop".repeat(100);
    let mut params = zrip::encode::strategy::level_params(1).unwrap();
    params.hash_log = 40;
    params.chain_log = 40;
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

// ===== Block boundary edge cases =====

#[test]
fn roundtrip_exactly_one_block() {
    let data: Vec<u8> = b"block boundary test pattern! "
        .iter()
        .cycle()
        .take(BLOCK)
        .copied()
        .collect();
    for level in [-7, -1, 1, 3, 4] {
        let compressed = zrip::compress(&data, level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, data, "level {level}");
    }
}

#[test]
fn roundtrip_incompressible_multiblock() {
    let data: Vec<u8> = (0..2 * BLOCK as u32)
        .map(|i| ((i.wrapping_mul(KNUTH)) >> 24) as u8)
        .collect();
    for level in [-7, 1, 3] {
        let compressed = zrip::compress(&data, level).unwrap();
        let decompressed =
            zrip::decompress(&compressed).unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(decompressed, data, "level {level}");
    }
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

/// A frame that fails before its sequence tables are read must not change
/// how a reused context decodes the next valid frame.
#[test]
fn decompress_context_recovers_after_a_corrupt_frame() {
    let data: Vec<u8> = (0..1500u32)
        .flat_map(|i| {
            format!(
                "record {i}: sensor {} reads {} at offset {}\n",
                i % 37,
                i.wrapping_mul(2_654_435_761) % 100_000,
                i * 13
            )
            .into_bytes()
        })
        .collect();
    let frame = zrip::compress(&data, 1).unwrap();
    let mut corrupt = frame.clone();
    // Lands in the first block's literals section.
    corrupt[12] ^= 1;

    let mut ctx = zrip::DecompressContext::new();
    for round in 0..3 {
        let mut output = Vec::new();
        ctx.decompress_into(&frame, &mut output).unwrap();
        assert!(output == data, "round {round}");
        let err = ctx.decompress_into(&corrupt, &mut Vec::new()).unwrap_err();
        assert_eq!(err, zrip::DecompressError::CorruptLiterals);
    }
}

#[test]
fn set_level_matches_a_fresh_context() {
    let text: Vec<u8> = (0..20_000)
        .flat_map(|i| format!("line {i} value {}\n", i * 31 % 1000).into_bytes())
        .collect();
    let inputs = [&text[..100], &text[..5_000], &text[..70_000], &text[..]];
    let mut ctx = zrip::CompressContext::new(-1).unwrap();
    for level in [4, -7, 1, 3, -3, 2, -1] {
        ctx.set_level(level).unwrap();
        assert_eq!(ctx.level(), level);
        for input in inputs {
            let mut fresh = zrip::CompressContext::new(level).unwrap();
            assert_eq!(
                ctx.compress(input).unwrap(),
                fresh.compress(input).unwrap(),
                "L{level} {} B",
                input.len()
            );
        }
    }
    assert!(ctx.set_level(5).is_err());
    assert_eq!(ctx.level(), -1);
}
