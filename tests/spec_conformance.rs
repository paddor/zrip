// Tests for RFC 8878 decoder conformance checks.
//
// Each test constructs or modifies a frame to trigger a specific check that
// was previously missing: reserved bits, per-type FSE accuracy limits,
// Repeat_Mode without a prior table, bitstream exact consumption, and
// streaming FCS mismatch.

#[test]
fn explicit_zero_dictionary_id_needs_no_dictionary() {
    for (flag, width) in [(1, 1), (2, 2), (3, 4)] {
        let mut frame = zrip::compress(b"hello", 1).unwrap();
        assert_eq!(frame[4] & 3, 0);
        let offset = if frame[4] & 0x20 != 0 { 5 } else { 6 };
        frame[4] |= flag;
        frame.splice(offset..offset, vec![0; width]);
        assert_eq!(zrip::decompress(&frame).unwrap(), b"hello");
        #[cfg(feature = "std")]
        {
            use std::io::Read;
            let mut output = Vec::new();
            zrip::FrameDecoder::new(frame.as_slice())
                .read_to_end(&mut output)
                .unwrap();
            assert_eq!(output, b"hello");
        }
        frame[offset] = 1;
        assert!(matches!(
            zrip::decompress(&frame),
            Err(zrip::DecompressError::DictRequired)
        ));
    }
}

#[test]
fn ignore_unused_frame_descriptor_bit() {
    let mut frame = zrip::compress(b"hello", 1).unwrap();
    frame[4] |= 0x10;
    assert_eq!(zrip::decompress(&frame).unwrap(), b"hello");
    #[cfg(feature = "std")]
    {
        use std::io::Read;
        let mut output = Vec::new();
        zrip::FrameDecoder::new(frame.as_slice())
            .read_to_end(&mut output)
            .unwrap();
        assert_eq!(output, b"hello");
    }
    frame[4] |= 0x08;
    assert!(zrip::decompress(&frame).is_err());
}

/// Parse a compressed frame far enough to return the offset of the sequence
/// section's mode byte inside the first compressed block. Panics if the first
/// block is not a compressed block or has zero sequences.
fn mode_byte_offset(compressed: &[u8]) -> usize {
    first_block_mode_byte(compressed).expect("first block has no mode byte")
}

/// Same as `mode_byte_offset`, but returns `None` if the first block is not a
/// compressed block or has zero sequences.
fn first_block_mode_byte(compressed: &[u8]) -> Option<usize> {
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
    if block_type != 2 {
        return None;
    }
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
    if b0 == 0 {
        return None;
    }
    off += if (b0 as usize) < 128 {
        1
    } else if (b0 as usize) < 255 {
        2
    } else {
        3
    };

    Some(off)
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

// ===== Predefined_Mode with a dictionary =====

/// Small protobuf-style records. A dictionary trained on them gets its own
/// sequence tables, and encoders often prefer the predefined ones for a record
/// this short.
#[cfg(feature = "std")]
fn protobuf_records(count: usize) -> Vec<Vec<u8>> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut rand = move |n: u64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state % n
    };
    let varint = |out: &mut Vec<u8>, mut v: u64| {
        while v >= 0x80 {
            out.push(v as u8 | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    };
    let symbols: Vec<Vec<u8>> = (0..500)
        .map(|_| (0..2 + rand(3)).map(|_| b'A' + rand(26) as u8).collect())
        .collect();
    let exchanges: [&[u8]; 6] = [b"XNAS", b"XNYS", b"ARCX", b"BATS", b"IEXG", b"EDGX"];

    (0..count as u64)
        .map(|i| {
            let symbol = &symbols[(rand(1000).pow(3) * 500 / 1_000_000_000) as usize];
            let bid = (500 + rand(49_500)) as f64 / 100.0;
            let mut record = vec![0x0a, symbol.len() as u8];
            record.extend_from_slice(symbol);
            record.extend_from_slice(&[0x12, 4]);
            record.extend_from_slice(exchanges[rand(6) as usize]);
            record.push(0x19);
            record.extend_from_slice(&bid.to_le_bytes());
            record.push(0x21);
            record.extend_from_slice(&(bid + (1 + rand(20)) as f64 / 100.0).to_le_bytes());
            record.push(0x28);
            varint(&mut record, (1 + rand(50)) * 100);
            record.push(0x30);
            varint(&mut record, (1 + rand(50)) * 100);
            record.push(0x38);
            varint(
                &mut record,
                1_790_000_000_000_000_000 + (i + 1) * 1_000_000 + rand(1_000_000),
            );
            record
        })
        .collect()
}

/// Trains a dictionary with C zstd. zrip's trainer stores the predefined
/// sequence tables, which hides a decoder that keeps the dictionary's table.
#[cfg(feature = "std")]
fn trained_dict_and_records() -> (Vec<u8>, Vec<Vec<u8>>) {
    const TRAIN: usize = 5_000;
    const TEST: usize = 1_000;

    let mut records = protobuf_records(TRAIN + TEST);
    let test = records.split_off(TRAIN);
    (zstd::dict::from_samples(&records, 8192).unwrap(), test)
}

/// A block can leave the dictionary's sequence tables for the predefined ones.
/// Every decode path must then load the predefined table. Decoding with the
/// dictionary's table fails, or returns wrong bytes with no error.
#[cfg(feature = "std")]
fn assert_dict_paths_decode(frames: &[Vec<u8>], records: &[Vec<u8>], dict_bytes: &[u8]) {
    use std::io::Read;

    const LIMIT: usize = 1 << 20;

    fn check<E: core::fmt::Debug>(path: &str, i: usize, decoded: Result<&[u8], E>, want: &[u8]) {
        match decoded {
            Ok(got) => assert_eq!(got, want, "{path}: frame {i} decoded to wrong bytes"),
            Err(e) => panic!("{path}: frame {i} failed: {e:?}"),
        }
    }

    let uses_predefined_table = |frame: &[u8]| {
        first_block_mode_byte(frame).is_some_and(|off| {
            [6, 4, 2].iter().any(|shift| {
                let mode = (frame[off] >> shift) & 3;
                mode == 0
            })
        })
    };
    assert!(
        frames.iter().any(|frame| uses_predefined_table(frame)),
        "no frame selects Predefined_Mode, so this test proves nothing"
    );

    let dict = zrip::Dictionary::from_bytes(dict_bytes).unwrap();
    let mut c_decoder = zstd::bulk::Decompressor::with_dictionary(dict_bytes).unwrap();
    let mut context = zrip::DecompressContext::with_dict(dict.clone());
    let mut streaming = zrip::FrameDecoder::with_dict(&[][..], dict.clone());
    let mut out = Vec::new();

    for (i, (frame, want)) in frames.iter().zip(records).enumerate() {
        // C zstd and the one-shot path show the frame itself is fine.
        let decoded = c_decoder.decompress(frame, LIMIT);
        check("C zstd", i, decoded.as_deref(), want);
        let decoded = zrip::decompress_with_dict_and_limit(frame, &dict, LIMIT);
        check("one-shot", i, decoded.as_deref(), want);

        let decoded = context.decompress_with_limit(frame, LIMIT);
        check("reused context", i, decoded.as_deref(), want);
        let mut fresh = zrip::DecompressContext::with_dict(dict.clone());
        let decoded = fresh.decompress_with_limit(frame, LIMIT);
        check("fresh context", i, decoded.as_deref(), want);

        out.clear();
        streaming.reset(frame.as_slice());
        let decoded = streaming.read_to_end(&mut out).map(|_| out.as_slice());
        check("reused streaming", i, decoded, want);
        out.clear();
        let decoded = zrip::FrameDecoder::with_dict(frame.as_slice(), dict.clone())
            .read_to_end(&mut out)
            .map(|_| out.as_slice());
        check("fresh streaming", i, decoded, want);
    }
}

#[cfg(feature = "std")]
#[test]
fn predefined_mode_replaces_dictionary_tables_in_c_frames() {
    let (dict_bytes, records) = trained_dict_and_records();
    for level in [1, 3] {
        let mut encoder = zstd::bulk::Compressor::with_dictionary(level, &dict_bytes).unwrap();
        let frames: Vec<Vec<u8>> = records
            .iter()
            .map(|record| encoder.compress(record).unwrap())
            .collect();
        assert_dict_paths_decode(&frames, &records, &dict_bytes);
    }
}

#[cfg(feature = "std")]
#[test]
fn predefined_mode_replaces_dictionary_tables_in_zrip_frames() {
    let (dict_bytes, records) = trained_dict_and_records();
    let dict = zrip::Dictionary::from_bytes(&dict_bytes).unwrap();
    for level in [1, 3] {
        let frames: Vec<Vec<u8>> = records
            .iter()
            .map(|record| zrip::compress_with_dict(record, level, &dict).unwrap())
            .collect();
        assert_dict_paths_decode(&frames, &records, &dict_bytes);
    }
}

/// A frame with a 1 KiB window and one compressed block: "Hello" as raw
/// literals, followed by `seq`, the sequence section.
fn hello_frame(seq: &[u8]) -> Vec<u8> {
    let block_size = 6 + seq.len() as u32;
    let header = (block_size << 3) | (2 << 1) | 1;
    let mut frame = vec![0x28, 0xB5, 0x2F, 0xFD, 0, 0];
    frame.extend_from_slice(&header.to_le_bytes()[..3]);
    frame.push(5 << 3); // raw literals, 5 bytes
    frame.extend_from_slice(b"Hello");
    frame.extend_from_slice(seq);
    frame
}

#[test]
fn sequence_section_is_required_and_ends_after_zero_count() {
    let ok = hello_frame(&[0]);
    assert_eq!(zrip::decompress(&ok).unwrap(), b"Hello");
    #[cfg(feature = "std")]
    assert_eq!(zstd::stream::decode_all(&ok[..]).unwrap(), b"Hello");

    // As in C zstd: no sequence count at all, or bytes after a zero count.
    for seq in [&[][..], &[0, 0], &[0, 0x80, 0, 0]] {
        let bad = hello_frame(seq);
        assert_eq!(
            zrip::decompress(&bad),
            Err(zrip::DecompressError::CorruptSequences),
            "{seq:?}"
        );
        #[cfg(feature = "std")]
        {
            use std::io::Read;
            assert!(zstd::stream::decode_all(&bad[..]).is_err(), "C {seq:?}");
            let mut out = Vec::new();
            assert!(
                zrip::FrameDecoder::new(bad.as_slice())
                    .read_to_end(&mut out)
                    .is_err()
            );
            let mut dec = zrip::push::PushDecoder::new();
            assert_eq!(
                dec.decode_to_vec(&bad, &mut out),
                Err(zrip::DecompressError::CorruptSequences)
            );
        }
    }
}

/// `Block_Maximum_Size` is the window, at most 128 KiB, and bounds the
/// compressed size too. A single-segment frame's window is its content
/// size, so a 7-byte block for 5 bytes of content is too large.
#[test]
fn block_larger_than_the_window_is_rejected() {
    let mut frame = hello_frame(&[0]);
    frame[4] = 0x20; // single segment, 1-byte content size
    frame[5] = 5;
    assert_eq!(
        zrip::decompress(&frame),
        Err(zrip::DecompressError::BlockTooLarge)
    );
    #[cfg(feature = "std")]
    {
        use std::io::Read;
        assert!(zstd::stream::decode_all(&frame[..]).is_err());
        let mut out = Vec::new();
        assert!(
            zrip::FrameDecoder::new(frame.as_slice())
                .read_to_end(&mut out)
                .is_err()
        );
        assert_eq!(
            zrip::push::PushDecoder::new().decode_to_vec(&frame, &mut out),
            Err(zrip::DecompressError::BlockTooLarge)
        );
    }
}
