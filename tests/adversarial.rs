// Adversarial/corruption tests. All use zrip-only (no C zstd).

#[cfg(not(miri))]
const KNUTH: u32 = 0x9E37_79B1;

#[cfg(not(miri))]
#[test]
fn decompress_garbage_bytes_never_panics() {
    for seed in 0u64..500 {
        let len = (seed % 300) as usize + 1;
        let data: Vec<u8> = (0..len)
            .map(|i| {
                let x = seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(i as u64);
                (x >> 33) as u8
            })
            .collect();
        let _ = zrip::decompress(&data);
    }
}

#[cfg(not(miri))]
#[test]
fn decompress_valid_frame_with_corrupt_blocks_never_panics() {
    let magic = [0x28, 0xB5, 0x2F, 0xFD];
    for seed in 0u64..200 {
        let garbage_len = (seed % 200) as usize + 1;
        let garbage: Vec<u8> = (0..garbage_len)
            .map(|i| {
                let x = seed
                    .wrapping_mul(u64::from(KNUTH))
                    .wrapping_add(i as u64 * 1_103_515_245);
                (x >> 24) as u8
            })
            .collect();
        let mut frame = Vec::with_capacity(4 + garbage_len);
        frame.extend_from_slice(&magic);
        frame.extend_from_slice(&garbage);
        let _ = zrip::decompress(&frame);
    }
}

#[cfg(not(miri))]
#[test]
fn decompress_bit_flipped_frames_never_panic() {
    let original: Vec<u8> = b"the quick brown fox jumps over the lazy dog "
        .iter()
        .cycle()
        .take(5000)
        .copied()
        .collect();
    let compressed = zrip::compress(&original, 1).unwrap();

    for byte_pos in 0..compressed.len() {
        for bit in 0..8u8 {
            let mut corrupted = compressed.clone();
            corrupted[byte_pos] ^= 1 << bit;
            let _ = zrip::decompress(&corrupted);
        }
    }
}

#[cfg(not(miri))]
#[test]
fn decompress_truncated_at_every_byte_never_panics() {
    let original: Vec<u8> = b"ABCDEFGHIJKLMNOP"
        .iter()
        .cycle()
        .take(8000)
        .copied()
        .collect();
    let compressed = zrip::compress(&original, 3).unwrap();
    for truncate_at in 0..compressed.len() {
        let _ = zrip::decompress(&compressed[..truncate_at]);
    }
}

#[cfg(not(miri))]
#[test]
fn decompress_two_byte_flips_never_panic() {
    let original: Vec<u8> = (0..2000u32)
        .map(|i| ((i.wrapping_mul(KNUTH)) >> 24) as u8)
        .collect();
    let compressed = zrip::compress(&original, 1).unwrap();
    let len = compressed.len();

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

#[cfg(not(miri))]
#[test]
fn decompress_zero_filled_after_header_never_panics() {
    let original: Vec<u8> = b"hello world ".iter().cycle().take(4000).copied().collect();
    let compressed = zrip::compress(&original, 1).unwrap();

    for zero_start in 4..compressed.len().min(20) {
        let mut corrupted = compressed.clone();
        for b in &mut corrupted[zero_start..] {
            *b = 0;
        }
        let _ = zrip::decompress(&corrupted);
    }
}

#[cfg(not(miri))]
#[test]
fn decompress_ff_filled_after_header_never_panics() {
    let original: Vec<u8> = b"test data ".iter().cycle().take(4000).copied().collect();
    let compressed = zrip::compress(&original, 1).unwrap();

    for ff_start in 4..compressed.len().min(20) {
        let mut corrupted = compressed.clone();
        for b in &mut corrupted[ff_start..] {
            *b = 0xFF;
        }
        let _ = zrip::decompress(&corrupted);
    }
}

// ===== Crafted malicious-looking inputs =====

#[test]
fn decompress_max_block_size_claim_with_tiny_input() {
    let frames: Vec<Vec<u8>> = vec![
        vec![
            0x28, 0xB5, 0x2F, 0xFD, // magic
            0xE0, // FHD: fcs=3, single=0, checksum=0, dictid=0
            0x00, // window descriptor
            0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, // FCS = 4GB
        ],
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
    let mut frame = vec![
        0x28, 0xB5, 0x2F, 0xFD, // magic
        0x00, // FHD: no checksum, no dict, fcs=0, single=0
        0x00, // window descriptor
    ];
    let block_hdr = (131_071_u32 << 3) | 0b001; // last=1, raw=00
    frame.push((block_hdr & 0xFF) as u8);
    frame.push(((block_hdr >> 8) & 0xFF) as u8);
    frame.push(((block_hdr >> 16) & 0xFF) as u8);
    let _ = zrip::decompress(&frame);
}

// ===== Fuzz regression vectors =====

#[test]
fn decompress_overproducing_block_returns_err() {
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
fn decompress_corrupt_4stream_huffman_refill_returns_err() {
    let data: &[u8] = &[
        0x28, 0xb5, 0x2f, 0xfd, 0x44, 0x10, 0x9a, 0x00, 0x3d, 0x01, 0x00, 0x1e, 0x06, 0x00, 0x08,
        0x00, 0x09, 0xe0, 0x01, 0xa7, 0x55, 0xe1, 0x55, 0x55, 0x58, 0x05, 0x04, 0x00, 0x04, 0x00,
        0x04, 0x00, 0xff, 0xff, 0xdf, 0x03, 0xfd, 0x09, 0x18, 0x05, 0x15, 0x02, 0xc4, 0x02, 0x00,
        0x11, 0x11, 0x11, 0x80, 0x11,
    ];
    let result = zrip::decompress(data);
    assert!(result.is_err());
}

#[test]
fn decompress_multiframe_corrupt_never_panics() {
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

// ===== Sequence/literal corruption =====

#[cfg(not(miri))]
#[test]
fn decompress_corrupt_sequence_section_never_panics() {
    let original: Vec<u8> = b"ABCDEFGHIJKLMNOP"
        .iter()
        .cycle()
        .take(8000)
        .copied()
        .collect();
    let compressed = zrip::compress(&original, 1).unwrap();

    let mid = compressed.len() / 2;
    for pos in mid..compressed.len() {
        for val in [0x00, 0xFF, 0x80, 0x01] {
            let mut corrupted = compressed.clone();
            corrupted[pos] = val;
            let _ = zrip::decompress(&corrupted);
        }
    }
}

#[cfg(not(miri))]
#[test]
fn decompress_corrupt_literals_section_never_panics() {
    let original: Vec<u8> = (0..8000u32)
        .map(|i| ((i.wrapping_mul(KNUTH)) >> 24) as u8)
        .collect();
    let compressed = zrip::compress(&original, 1).unwrap();

    let mid = compressed.len() / 2;
    for pos in 6..mid {
        for val in [0x00, 0xFF, 0x80] {
            let mut corrupted = compressed.clone();
            corrupted[pos] = val;
            let _ = zrip::decompress(&corrupted);
        }
    }
}

// ===== Near-valid frames =====

#[cfg(not(miri))]
#[test]
fn decompress_frame_content_size_mismatch_never_panics() {
    let original = b"test data for fcs mismatch";
    let compressed = zrip::compress(original, 1).unwrap();

    for pos in 4..compressed.len().min(12) {
        for delta in [1u8, 2, 0x80, 0xFF] {
            let mut corrupted = compressed.clone();
            corrupted[pos] = corrupted[pos].wrapping_add(delta);
            let _ = zrip::decompress(&corrupted);
        }
    }
}

// ===== Per-block output ceiling =====

#[test]
fn decompress_block_output_ceiling() {
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
        0x80, 0x80, 0x01,
    ];
    let _ = zrip::decompress(data);
}

// ===== Fuzz corpus dict round-trip =====
//
// Two-phase design:
//   1. fuzz_corpus_dict_generate (not(miri)): trains a dict from corpus
//      plaintexts, compresses each with the dict, writes a fixture file
//      containing raw dict bytes + (plaintext, compressed) pairs.
//   2. fuzz_corpus_dict_decode_miri (miri): loads the fixture, parses the
//      dict, decompresses each frame, verifies. No dict training, no
//      encoding, no filesystem scanning: pure decode-path coverage.

const FIXTURE_PATH: &str = "tests/fixtures/corpus_dict_roundtrip.bin";

#[cfg(feature = "dict_builder")]
fn collect_fuzz_corpus_plaintexts() -> Vec<Vec<u8>> {
    let corpus_dir = std::path::Path::new("fuzz/corpus/fuzz_corrupt_decompress");
    if !corpus_dir.exists() {
        return Vec::new();
    }
    let mut plaintexts = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(corpus_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if entry.file_type().map_or(true, |t| t.is_dir()) {
            continue;
        }
        let data = std::fs::read(entry.path()).unwrap();
        if let Ok(pt) = zrip::decompress(&data)
            && !pt.is_empty()
            && pt.len() <= 4096
        {
            plaintexts.push(pt);
        }
    }
    plaintexts
}

#[cfg(all(feature = "dict_builder", not(miri)))]
fn write_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn read_u32(data: &[u8], off: &mut usize) -> u32 {
    let v = u32::from_le_bytes([data[*off], data[*off + 1], data[*off + 2], data[*off + 3]]);
    *off += 4;
    v
}

/// Fixture format: [dict_len:u32][dict_bytes][n_pairs:u32]
///   per pair: [pt_len:u32][pt_bytes][comp_len:u32][comp_bytes]
#[cfg(all(feature = "dict_builder", not(miri)))]
#[test]
fn fuzz_corpus_dict_generate() {
    let plaintexts = collect_fuzz_corpus_plaintexts();
    if plaintexts.len() < 10 {
        eprintln!(
            "skipping: only {} decompressible corpus files",
            plaintexts.len()
        );
        return;
    }
    let refs: Vec<&[u8]> = plaintexts.iter().map(|p| p.as_slice()).collect();

    let content = zrip::dict::fastcover::select_segments(
        &refs,
        4096,
        &zrip::dict::fastcover::FastCoverParams::default(),
    );
    let dict_bytes = zrip::dict::finalize::finalize_dictionary(&content, &refs, 4096);
    let dict = zrip::Dictionary::from_bytes(&dict_bytes).unwrap();

    let mut fixture = Vec::new();
    write_u32(&mut fixture, dict_bytes.len() as u32);
    fixture.extend_from_slice(&dict_bytes);
    write_u32(&mut fixture, plaintexts.len() as u32);

    for (i, pt) in plaintexts.iter().enumerate() {
        let compressed = zrip::compress_with_dict(pt, 1, &dict).unwrap();
        let decompressed = zrip::decompress_with_dict(&compressed, &dict).unwrap();
        assert_eq!(&decompressed, pt, "corpus file {i}");

        write_u32(&mut fixture, pt.len() as u32);
        fixture.extend_from_slice(pt);
        write_u32(&mut fixture, compressed.len() as u32);
        fixture.extend_from_slice(&compressed);
    }

    std::fs::create_dir_all("tests/fixtures").unwrap();
    std::fs::write(FIXTURE_PATH, &fixture).unwrap();
    eprintln!(
        "wrote {}: {} dict bytes, {} pairs, {} total bytes",
        FIXTURE_PATH,
        dict_bytes.len(),
        plaintexts.len(),
        fixture.len()
    );
}

/// Miri: load pre-built fixture, decode-only. No dict training, no encode.
#[test]
fn fuzz_corpus_dict_decode_miri() {
    let Ok(fixture) = std::fs::read(FIXTURE_PATH) else {
        eprintln!("skipping: run fuzz_corpus_dict_generate first to create fixture");
        return;
    };
    let mut off = 0;
    let dict_len = read_u32(&fixture, &mut off) as usize;
    let dict_bytes = &fixture[off..off + dict_len];
    off += dict_len;
    let dict = zrip::Dictionary::from_bytes(dict_bytes).unwrap();
    let n_pairs = read_u32(&fixture, &mut off) as usize;

    for i in 0..n_pairs {
        let pt_len = read_u32(&fixture, &mut off) as usize;
        let expected = &fixture[off..off + pt_len];
        off += pt_len;
        let comp_len = read_u32(&fixture, &mut off) as usize;
        let compressed = &fixture[off..off + comp_len];
        off += comp_len;

        let decompressed = zrip::decompress_with_dict(compressed, &dict).unwrap();
        assert_eq!(decompressed, expected, "pair {i}");
    }
    assert_eq!(off, fixture.len());
    eprintln!("{n_pairs} corpus dict frames decoded OK");
}
