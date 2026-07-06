#[cfg(all(miri, not(feature = "paranoid")))]
#[test]
#[should_panic(expected = "bit count must be <= 25")]
fn bit_writer_large_public_bit_count_panics_before_unsafe_flush() {
    use zrip_core::bitstream::writer::BitWriter;

    // Regression: `write_bits` is a safe public API, so violating its maximum
    // bit-count contract must stop before the unsafe flush path can expose
    // uninitialized bytes via `set_len`.
    let mut writer = BitWriter::new();
    writer.write_bits(0, 255);
}

#[cfg(all(miri, not(feature = "paranoid")))]
#[test]
fn huffman_decode_zero_table_log_is_rejected_before_fast_tail_lookup() {
    use zrip_core::DecompressError;
    use zrip_core::huffman::HuffmanDecodeEntry;
    use zrip_core::huffman::decode::decode_single_stream_into;

    // Regression: a one-entry table satisfies `table.len() >= 1 << table_log`
    // when `table_log == 0`, but zero is not a valid Huffman table log. Reject
    // it before `decode_stream_tail` can compute an unchecked table index from
    // the fast path.
    let table = [HuffmanDecodeEntry {
        symbol: 0,
        num_bits: 1,
    }];
    let data = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x80];
    let mut output = [0u8; 5];

    assert_eq!(
        decode_single_stream_into(&table, 0, &data, &mut output),
        Err(DecompressError::BadHuffmanStream)
    );
}

#[cfg(all(miri, not(feature = "paranoid")))]
#[test]
fn huffman_4stream_zero_table_log_is_rejected_before_tail_lookup() {
    use zrip_core::DecompressError;
    use zrip_core::huffman::HuffmanDecodeEntry;
    use zrip_core::huffman::decode::decode_4_streams_into;

    // Regression: the 4-stream decoder reaches the same tail helper after
    // splitting the output. Reject a zero table log at the public boundary so
    // it cannot reach `huf_table_lookup`'s unchecked read.
    let table = [HuffmanDecodeEntry {
        symbol: 0,
        num_bits: 1,
    }];
    let stream = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x80];
    let mut data = Vec::new();
    data.extend_from_slice(&8u16.to_le_bytes());
    data.extend_from_slice(&8u16.to_le_bytes());
    data.extend_from_slice(&8u16.to_le_bytes());
    data.extend_from_slice(&stream);
    data.extend_from_slice(&stream);
    data.extend_from_slice(&stream);
    data.extend_from_slice(&stream);
    let mut output = Vec::new();

    assert_eq!(
        decode_4_streams_into(&table, 0, &data, 24, &mut output),
        Err(DecompressError::BadHuffmanStream)
    );
}
