// Tests for RFC 8878 decoder conformance checks.
//
// Each test constructs or modifies a frame to trigger a specific check that
// was previously missing: reserved bits, per-type FSE accuracy limits,
// Repeat_Mode without a prior table, bitstream exact consumption, and
// streaming FCS mismatch.

/// Parse a compressed frame far enough to return the offset of the sequence
/// section's mode byte inside the first compressed block. Panics if the first
/// block is not a compressed block or has zero sequences.
fn mode_byte_offset(compressed: &[u8]) -> usize {
    assert_eq!(
        &compressed[..4],
        &[0x28, 0xB5, 0x2F, 0xFD],
        "not a zstd frame"
    );
    let descriptor = compressed[4];
    let single_segment = (descriptor & 0x20) != 0;
    let dict_id_flag = descriptor & 0x03;
    let fcs_flag = (descriptor >> 6) & 0x03;

    let mut off = 5;
    if !single_segment {
        off += 1;
    }
    off += match dict_id_flag {
        0 => 0,
        1 => 1,
        2 => 2,
        _ => 4,
    };
    off += match fcs_flag {
        0 if single_segment => 1,
        0 => 0,
        1 => 2,
        2 => 4,
        _ => 8,
    };

    // Block header
    let bh = compressed[off] as u32
        | ((compressed[off + 1] as u32) << 8)
        | ((compressed[off + 2] as u32) << 16);
    let block_type = (bh >> 1) & 0x03;
    assert_eq!(block_type, 2, "first block is not compressed");
    off += 3;

    // Literals section header
    let lit_type = compressed[off] & 0x03;
    let size_fmt = (compressed[off] >> 2) & 0x03;
    let (lit_stream_bytes, lit_hdr_size) = match lit_type {
        0 => {
            // Raw
            let (regen, hdr) = raw_rle_size(compressed, off, size_fmt);
            (regen, hdr)
        }
        1 => {
            // RLE
            let (_regen, hdr) = raw_rle_size(compressed, off, size_fmt);
            (1, hdr) // 1 byte for the RLE value
        }
        2 | 3 => {
            // Compressed / Treeless
            let (_regen, comp, hdr) = compressed_lit_sizes(compressed, off, size_fmt);
            (comp, hdr)
        }
        _ => unreachable!(),
    };
    off += lit_hdr_size + lit_stream_bytes;

    // Sequence count
    let b0 = compressed[off];
    assert!(b0 != 0, "block has 0 sequences, no mode byte");
    off += if (b0 as usize) < 128 {
        1
    } else if (b0 as usize) < 255 {
        2
    } else {
        3
    };

    off
}

fn raw_rle_size(data: &[u8], off: usize, size_fmt: u8) -> (usize, usize) {
    match size_fmt {
        0 | 2 => ((data[off] >> 3) as usize, 1),
        1 => {
            let s = ((data[off] >> 4) as usize) | ((data[off + 1] as usize) << 4);
            (s, 2)
        }
        _ => {
            let s = ((data[off] >> 4) as usize)
                | ((data[off + 1] as usize) << 4)
                | ((data[off + 2] as usize) << 12);
            (s, 3)
        }
    }
}

fn compressed_lit_sizes(data: &[u8], off: usize, size_fmt: u8) -> (usize, usize, usize) {
    match size_fmt {
        0 => {
            let both = (data[off] as usize >> 4)
                | ((data[off + 1] as usize) << 4)
                | ((data[off + 2] as usize) << 12);
            (both & 0x3FF, both >> 10, 3)
        }
        1 => {
            let both = (data[off] as usize >> 4)
                | ((data[off + 1] as usize) << 4)
                | ((data[off + 2] as usize) << 12);
            (both & 0x3FF, both >> 10, 3)
        }
        2 => {
            let both = (data[off] as usize >> 4)
                | ((data[off + 1] as usize) << 4)
                | ((data[off + 2] as usize) << 12)
                | ((data[off + 3] as usize) << 20);
            (both & 0x3FFF, both >> 14, 4)
        }
        _ => {
            let both = (data[off] as usize >> 4)
                | ((data[off + 1] as usize) << 4)
                | ((data[off + 2] as usize) << 12)
                | ((data[off + 3] as usize) << 20)
                | ((data[off + 4] as usize) << 28);
            (both & 0x3FFFF, both >> 18, 5)
        }
    }
}

// ===== Fix 1: Reserved bits in sequence mode byte (bits 1:0 must be zero) =====

#[test]
fn reject_reserved_bits_in_mode_byte() {
    let data: Vec<u8> = b"ABCDEFGH".iter().cycle().take(8000).copied().collect();
    let compressed = zrip::compress(&data, 1).unwrap();

    // Sanity: unmodified frame decompresses fine.
    assert!(zrip::decompress(&compressed).is_ok());

    let mb = mode_byte_offset(&compressed);

    // Set reserved bit 0
    let mut bad = compressed.clone();
    bad[mb] |= 0x01;
    assert!(
        zrip::decompress(&bad).is_err(),
        "should reject reserved bit 0 set in mode byte"
    );

    // Set reserved bit 1
    let mut bad = compressed.clone();
    bad[mb] |= 0x02;
    assert!(
        zrip::decompress(&bad).is_err(),
        "should reject reserved bit 1 set in mode byte"
    );

    // Set both reserved bits
    let mut bad = compressed.clone();
    bad[mb] |= 0x03;
    assert!(
        zrip::decompress(&bad).is_err(),
        "should reject both reserved bits set in mode byte"
    );
}

// ===== Fix 2: Per-type FSE accuracy_log limits =====
//
// The mode byte tells the decoder which FSE mode each table uses.
// Mode 2 = FSE_Compressed, where the next field is the FSE table description.
// The first 4 bits of that description encode (accuracy_log - 5).
// Spec limits: LL max=9, OF max=8, ML max=9.
//
// We set mode=2 (FSE_Compressed) for the target table, then plant a 4-bit
// accuracy value that exceeds the limit.

#[test]
fn reject_ll_accuracy_log_above_9() {
    let data: Vec<u8> = b"ABCDEFGH".iter().cycle().take(8000).copied().collect();
    let compressed = zrip::compress(&data, 1).unwrap();
    let mb = mode_byte_offset(&compressed);

    // Force LL mode to FSE_Compressed (mode 2 = bits 7:6 = 10)
    let mut bad = compressed.clone();
    bad[mb] = (bad[mb] & 0x3C) | (0b10 << 6);
    // The FSE table description starts at mb+1 (in the bitstream after mode byte).
    // First 4 bits encode (accuracy_log - 5). Set to 5 => accuracy_log=10 (> 9 limit).
    // The bit reader reads the bitstream byte-aligned after the mode byte.
    // Byte at mb+1, low 4 bits = accuracy_log - 5.
    bad[mb + 1] = (bad[mb + 1] & 0xF0) | 5; // accuracy_log = 10
    let result = zrip::decompress(&bad);
    assert!(
        result.is_err(),
        "should reject LL accuracy_log=10 (max is 9)"
    );
}

#[test]
fn reject_of_accuracy_log_above_8() {
    let data: Vec<u8> = b"ABCDEFGH".iter().cycle().take(8000).copied().collect();
    let compressed = zrip::compress(&data, 1).unwrap();
    let mb = mode_byte_offset(&compressed);

    // Force OF mode to FSE_Compressed (mode 2 = bits 5:4 = 10)
    // Keep LL as predefined (00), ML as predefined (00)
    let mut bad = compressed.clone();
    #[allow(clippy::identity_op)]
    {
        bad[mb] = (0b00 << 6) | (0b10 << 4) | (0b00 << 2);
    }
    // OF FSE table starts right after the mode byte in the bit reader.
    // But since LL is predefined (mode 0), no LL table data is present.
    // The OF table description starts at the first bit after the mode byte.
    // Set first 4 bits to 4 => accuracy_log = 9 (> 8 limit for OF).
    bad[mb + 1] = (bad[mb + 1] & 0xF0) | 4; // accuracy_log = 9
    let result = zrip::decompress(&bad);
    assert!(
        result.is_err(),
        "should reject OF accuracy_log=9 (max is 8)"
    );
}

#[test]
fn reject_ml_accuracy_log_above_9() {
    let data: Vec<u8> = b"ABCDEFGH".iter().cycle().take(8000).copied().collect();
    let compressed = zrip::compress(&data, 1).unwrap();
    let mb = mode_byte_offset(&compressed);

    // Force ML mode to FSE_Compressed (mode 2 = bits 3:2 = 10)
    // Keep LL and OF as predefined (00)
    let mut bad = compressed.clone();
    #[allow(clippy::identity_op)]
    {
        bad[mb] = (0b00 << 6) | (0b00 << 4) | (0b10 << 2);
    }
    // LL and OF are predefined, so no table data for them.
    // ML FSE table starts at first bit after mode byte.
    bad[mb + 1] = (bad[mb + 1] & 0xF0) | 5; // accuracy_log = 10
    let result = zrip::decompress(&bad);
    assert!(
        result.is_err(),
        "should reject ML accuracy_log=10 (max is 9)"
    );
}

// ===== Fix 3: Repeat_Mode (mode 3) on first block without prior table =====

#[test]
fn reject_repeat_mode_ll_on_first_block() {
    let data: Vec<u8> = b"ABCDEFGH".iter().cycle().take(8000).copied().collect();
    let compressed = zrip::compress(&data, 1).unwrap();
    let mb = mode_byte_offset(&compressed);

    // Set LL mode to Repeat (11), keep OF and ML as predefined (00)
    let mut bad = compressed.clone();
    #[allow(clippy::identity_op)]
    {
        bad[mb] = (0b11 << 6) | (0b00 << 4) | (0b00 << 2);
    }
    let result = zrip::decompress(&bad);
    assert!(
        result.is_err(),
        "should reject Repeat_Mode for LL on first block"
    );
}

#[test]
fn reject_repeat_mode_of_on_first_block() {
    let data: Vec<u8> = b"ABCDEFGH".iter().cycle().take(8000).copied().collect();
    let compressed = zrip::compress(&data, 1).unwrap();
    let mb = mode_byte_offset(&compressed);

    let mut bad = compressed.clone();
    #[allow(clippy::identity_op)]
    {
        bad[mb] = (0b00 << 6) | (0b11 << 4) | (0b00 << 2);
    }
    let result = zrip::decompress(&bad);
    assert!(
        result.is_err(),
        "should reject Repeat_Mode for OF on first block"
    );
}

#[test]
fn reject_repeat_mode_ml_on_first_block() {
    let data: Vec<u8> = b"ABCDEFGH".iter().cycle().take(8000).copied().collect();
    let compressed = zrip::compress(&data, 1).unwrap();
    let mb = mode_byte_offset(&compressed);

    let mut bad = compressed.clone();
    #[allow(clippy::identity_op)]
    {
        bad[mb] = (0b00 << 6) | (0b00 << 4) | (0b11 << 2);
    }
    let result = zrip::decompress(&bad);
    assert!(
        result.is_err(),
        "should reject Repeat_Mode for ML on first block"
    );
}

#[test]
fn accept_repeat_mode_on_second_block() {
    // Multi-block data: first block sets tables, second can use Repeat.
    // If the second block's mode byte uses Repeat, it should succeed.
    // We verify this by compressing data large enough for 2+ blocks and
    // checking the roundtrip still works (the encoder may use Repeat naturally).
    let data: Vec<u8> = b"ABCDEFGH".iter().cycle().take(200_000).copied().collect();
    let compressed = zrip::compress(&data, 1).unwrap();
    assert!(
        zrip::decompress(&compressed).is_ok(),
        "multi-block roundtrip should succeed"
    );
}

// ===== Fix 4: Sequence bitstream exact consumption =====

#[test]
fn reject_unconsumed_sequence_bitstream_bits() {
    // Compress data that produces a compressed block with sequences.
    let data: Vec<u8> = b"ABCDEFGH".iter().cycle().take(8000).copied().collect();
    let compressed = zrip::compress(&data, 1).unwrap();

    // Find block header offset
    let descriptor = compressed[4];
    let single_segment = (descriptor & 0x20) != 0;
    let dict_id_flag = descriptor & 0x03;
    let fcs_flag = (descriptor >> 6) & 0x03;
    let mut hdr_off = 5;
    if !single_segment {
        hdr_off += 1;
    }
    hdr_off += match dict_id_flag {
        0 => 0,
        1 => 1,
        2 => 2,
        _ => 4,
    };
    hdr_off += match fcs_flag {
        0 if single_segment => 1,
        0 => 0,
        1 => 2,
        2 => 4,
        _ => 8,
    };

    let bh = compressed[hdr_off] as u32
        | ((compressed[hdr_off + 1] as u32) << 8)
        | ((compressed[hdr_off + 2] as u32) << 16);
    let block_size = (bh >> 3) as usize;
    let block_start = hdr_off + 3;
    let block_end = block_start + block_size;

    // Insert a padding byte right before the last byte of the block data
    // (the sequence bitstream is read backwards from the end of the block).
    // This adds an extra byte of unconsumed data.
    let mut bad = compressed[..block_end].to_vec();
    bad.insert(block_end - 1, 0x00); // extra byte in bitstream region

    // Update block size (+1)
    let new_bh = (((block_size + 1) as u32) << 3) | (bh & 0x07);
    bad[hdr_off] = (new_bh & 0xFF) as u8;
    bad[hdr_off + 1] = ((new_bh >> 8) & 0xFF) as u8;
    bad[hdr_off + 2] = ((new_bh >> 16) & 0xFF) as u8;

    // Append the rest (checksum)
    bad.extend_from_slice(&compressed[block_end..]);

    // Also update FCS if single_segment (our content size didn't change,
    // but the *compressed* size did, which doesn't affect FCS).
    let result = zrip::decompress(&bad);
    assert!(
        result.is_err(),
        "should reject frame with unconsumed sequence bitstream bits"
    );
}

// ===== Fix 5: Huffman bitstream exact consumption =====

#[test]
fn reject_unconsumed_huffman_bitstream_bits() {
    // Compress data that produces Huffman-compressed literals.
    // Then expand the Huffman compressed stream size by 1 byte.
    let data: Vec<u8> = b"the quick brown fox jumps over the lazy dog "
        .iter()
        .cycle()
        .take(8000)
        .copied()
        .collect();
    let compressed = zrip::compress(&data, 1).unwrap();

    // Sanity
    assert!(zrip::decompress(&compressed).is_ok());

    // Parse to the literals section
    let descriptor = compressed[4];
    let single_segment = (descriptor & 0x20) != 0;
    let dict_id_flag = descriptor & 0x03;
    let fcs_flag = (descriptor >> 6) & 0x03;
    let mut off = 5;
    if !single_segment {
        off += 1;
    }
    off += match dict_id_flag {
        0 => 0,
        1 => 1,
        2 => 2,
        _ => 4,
    };
    off += match fcs_flag {
        0 if single_segment => 1,
        0 => 0,
        1 => 2,
        2 => 4,
        _ => 8,
    };

    let bh_off = off;
    let bh = compressed[off] as u32
        | ((compressed[off + 1] as u32) << 8)
        | ((compressed[off + 2] as u32) << 16);
    let block_type = (bh >> 1) & 0x03;
    let block_size = (bh >> 3) as usize;
    off += 3;

    if block_type != 2 {
        // Not compressed; skip this test on this data (shouldn't happen for
        // 8 KB of text at L1).
        return;
    }

    let lit_type = compressed[off] & 0x03;
    if lit_type != 2 {
        // Literals are not Huffman-compressed; skip.
        return;
    }

    let size_fmt = (compressed[off] >> 2) & 0x03;
    let (regen_size, comp_size, hdr_size) = compressed_lit_sizes(&compressed, off, size_fmt);
    let lit_stream_start = off + hdr_size;
    let lit_stream_end = lit_stream_start + comp_size;

    // Insert a byte into the Huffman stream to create unconsumed bits.
    let mut bad = compressed[..lit_stream_end].to_vec();
    bad.insert(lit_stream_end - 1, 0x00);

    // Update compressed literals size (+1).
    // Rewrite the literals header with comp_size + 1.
    let new_comp = comp_size + 1;
    match size_fmt {
        0 | 1 => {
            let both = regen_size | (new_comp << 10);
            bad[off] = (bad[off] & 0x0F) | ((both & 0x0F) << 4) as u8;
            bad[off + 1] = ((both >> 4) & 0xFF) as u8;
            bad[off + 2] = ((both >> 12) & 0xFF) as u8;
        }
        2 => {
            let both = regen_size | (new_comp << 14);
            bad[off] = (bad[off] & 0x0F) | ((both & 0x0F) << 4) as u8;
            bad[off + 1] = ((both >> 4) & 0xFF) as u8;
            bad[off + 2] = ((both >> 12) & 0xFF) as u8;
            bad[off + 3] = ((both >> 20) & 0xFF) as u8;
        }
        _ => return, // 5-byte header is rare; skip if encountered
    }

    // Update block size (+1)
    let new_bh = (((block_size + 1) as u32) << 3) | (bh & 0x07);
    bad[bh_off] = (new_bh & 0xFF) as u8;
    bad[bh_off + 1] = ((new_bh >> 8) & 0xFF) as u8;
    bad[bh_off + 2] = ((new_bh >> 16) & 0xFF) as u8;

    // Append rest of original frame
    bad.extend_from_slice(&compressed[lit_stream_end..]);

    let result = zrip::decompress(&bad);
    assert!(
        result.is_err(),
        "should reject frame with unconsumed Huffman bitstream bits"
    );
}

// ===== Fix 6: Streaming decoder FCS mismatch =====

#[cfg(feature = "std")]
#[test]
fn streaming_decoder_rejects_fcs_mismatch() {
    use std::io::Read;

    // Compress some data (the encoder writes FCS in single-segment frames).
    let data = b"hello streaming fcs check ".repeat(100);
    let compressed = zrip::compress(&data, 1).unwrap();

    // Sanity: FrameDecoder works on the valid frame.
    let mut dec = zrip::FrameDecoder::new(&compressed[..]);
    let mut out = Vec::new();
    dec.read_to_end(&mut out).unwrap();
    assert_eq!(out, data);

    // Corrupt the FCS field. For a single-segment frame (descriptor byte has
    // bit 5 set), FCS is the last field of the frame header. Its position
    // depends on dict_id_flag and fcs_field_size.
    let descriptor = compressed[4];
    let single_segment = (descriptor & 0x20) != 0;
    assert!(single_segment, "expected single-segment frame");
    let dict_id_flag = descriptor & 0x03;
    let fcs_flag = (descriptor >> 6) & 0x03;

    let mut fcs_off = 5; // after descriptor
    // No window descriptor for single-segment
    fcs_off += match dict_id_flag {
        0 => 0,
        1 => 1,
        2 => 2,
        _ => 4,
    };
    let fcs_len = match fcs_flag {
        0 if single_segment => 1,
        1 => 2,
        2 => 4,
        3 => 8,
        _ => 0,
    };

    // Modify FCS to a wrong value (increment by 1).
    let mut bad = compressed.clone();
    bad[fcs_off] = bad[fcs_off].wrapping_add(1);

    let mut dec = zrip::FrameDecoder::new(&bad[..]);
    let mut out = Vec::new();
    let result = dec.read_to_end(&mut out);
    assert!(
        result.is_err(),
        "FrameDecoder should reject FCS mismatch (fcs_len={fcs_len})"
    );
}

// ===== Fix 7: Huffman weight FSE accuracy_log limit (max 6) =====

#[test]
fn reject_huffman_weight_fse_accuracy_above_6() {
    // Compress text data that produces Huffman-compressed literals with an
    // FSE-encoded weight table. Then modify the accuracy_log in the weight
    // table's FSE description to exceed 6.
    let data: Vec<u8> = b"the quick brown fox jumps over the lazy dog "
        .iter()
        .cycle()
        .take(8000)
        .copied()
        .collect();
    let compressed = zrip::compress(&data, 1).unwrap();

    let descriptor = compressed[4];
    let single_segment = (descriptor & 0x20) != 0;
    let dict_id_flag = descriptor & 0x03;
    let fcs_flag = (descriptor >> 6) & 0x03;
    let mut off = 5;
    if !single_segment {
        off += 1;
    }
    off += match dict_id_flag {
        0 => 0,
        1 => 1,
        2 => 2,
        _ => 4,
    };
    off += match fcs_flag {
        0 if single_segment => 1,
        0 => 0,
        1 => 2,
        2 => 4,
        _ => 8,
    };

    let bh = compressed[off] as u32
        | ((compressed[off + 1] as u32) << 8)
        | ((compressed[off + 2] as u32) << 16);
    let block_type = (bh >> 1) & 0x03;
    off += 3;

    if block_type != 2 {
        return;
    }

    let lit_type = compressed[off] & 0x03;
    if lit_type != 2 {
        // Not Huffman-compressed literals
        return;
    }

    let size_fmt = (compressed[off] >> 2) & 0x03;
    let (_regen, _comp, hdr_size) = compressed_lit_sizes(&compressed, off, size_fmt);
    let stream_start = off + hdr_size;

    // The Huffman tree description is at the start of the compressed stream.
    // First byte: if < 128, it's FSE-compressed weights. The byte value is the
    // compressed size of the weight table.
    let weight_header = compressed[stream_start];
    if weight_header >= 128 {
        // Direct representation, no FSE table to corrupt.
        return;
    }

    // FSE table description starts at stream_start + 1.
    // First 4 bits encode (accuracy_log - 5). Set to 2 => accuracy_log=7 (> 6).
    let mut bad = compressed.clone();
    let fse_byte = stream_start + 1;
    bad[fse_byte] = (bad[fse_byte] & 0xF0) | 2; // accuracy_log = 7

    let result = zrip::decompress(&bad);
    assert!(
        result.is_err(),
        "should reject Huffman weight FSE accuracy_log=7 (max is 6)"
    );
}

// ===== Regression: valid frames still decompress correctly =====

#[test]
fn valid_frames_unaffected_by_stricter_checks() {
    // Ensure that the stricter checks don't break any valid frames.
    let patterns: &[(&[u8], i32)] = &[
        (b"ABCDEFGH", 1),
        (b"ABCDEFGH", -7),
        (b"ABCDEFGH", 3),
        (b"hello world ", 1),
        (b"hello world ", -1),
    ];
    for &(pat, level) in patterns {
        let data: Vec<u8> = pat.iter().cycle().take(200_000).copied().collect();
        let compressed = zrip::compress(&data, level).unwrap();
        let result = zrip::decompress(&compressed)
            .unwrap_or_else(|e| panic!("pat={:?} level={level}: {e}", std::str::from_utf8(pat)));
        assert_eq!(
            result,
            data,
            "pat={:?} level={level}",
            std::str::from_utf8(pat)
        );
    }
}

#[cfg(feature = "std")]
#[test]
fn streaming_valid_frames_unaffected() {
    use std::io::{Read, Write};

    for level in [-7, -3, -1, 1, 2, 3, 4] {
        let data: Vec<u8> = b"streaming valid "
            .iter()
            .cycle()
            .take(50_000)
            .copied()
            .collect();
        let mut enc = zrip::FrameEncoder::new(Vec::new(), level).unwrap();
        enc.write_all(&data).unwrap();
        let compressed = enc.finish().unwrap();
        let mut dec = zrip::FrameDecoder::new(&compressed[..]);
        let mut out = Vec::new();
        dec.read_to_end(&mut out)
            .unwrap_or_else(|e| panic!("level {level}: {e}"));
        assert_eq!(out, data, "level {level}");
    }
}
