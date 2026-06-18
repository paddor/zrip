// Adversarial/corruption tests. All use zrip-only (no C zstd).

#[test]
fn decompress_garbage_bytes_never_panics() {
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
    let compressed = zrip::compress(&original, 1).unwrap();

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
    let compressed = zrip::compress(&original, 3).unwrap();
    for truncate_at in 0..compressed.len() {
        let _ = zrip::decompress(&compressed[..truncate_at]);
    }
}

#[test]
fn decompress_two_byte_flips_never_panic() {
    let original: Vec<u8> = (0..2000u32)
        .map(|i| ((i.wrapping_mul(2654435761)) >> 24) as u8)
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
    let block_hdr = (131071u32 << 3) | 0b001; // last=1, raw=00
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

#[test]
fn decompress_corrupt_literals_section_never_panics() {
    let original: Vec<u8> = (0..8000u32)
        .map(|i| ((i.wrapping_mul(2654435761)) >> 24) as u8)
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
