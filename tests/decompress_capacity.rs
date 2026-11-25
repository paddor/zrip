//! A frame that declares its content size decodes into one allocation of that
//! size plus the block writer's slack.

use zrip::DecompressError;

/// Spare bytes the block writer keeps past the end of its output.
const SLACK: usize = 64;

/// Returns `len` bytes: runs of a repeated pattern mixed with random letters,
/// so the frame has compressed blocks with sequences.
fn compressible(len: usize) -> Vec<u8> {
    let mut x = 0x1234_5678u32;
    (0..len)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            if i % 64 < 48 {
                b"abcdefghijklmnop"[i % 16]
            } else {
                b'a' + (x % 26) as u8
            }
        })
        .collect()
}

/// Returns `len` random bytes, which compress to raw blocks.
fn incompressible(len: usize) -> Vec<u8> {
    let mut x = 0x9e37_79b9u32;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

/// Rewrites the frame content size field of a single-frame `frame`.
fn with_declared_size(frame: &[u8], size: u64) -> Vec<u8> {
    let descriptor = frame[4];
    let single_segment = descriptor & 0x20 != 0;
    let dict_id_len = [0, 1, 2, 4][usize::from(descriptor & 3)];
    let offset = 5 + usize::from(!single_segment) + dict_id_len;
    let width = [usize::from(single_segment), 2, 4, 8][usize::from(descriptor >> 6)];
    let value = if width == 2 { size - 256 } else { size };
    let mut frame = frame.to_vec();
    frame[offset..offset + width].copy_from_slice(&value.to_le_bytes()[..width]);
    frame
}

#[test]
fn decompress_allocates_the_declared_size() {
    for len in [2_001, 1_000_000] {
        let data = compressible(len);
        let frame = zrip::compress(&data, 1).unwrap();
        let output = zrip::decompress(&frame).unwrap();
        assert_eq!(output, data);
        assert!(
            output.capacity() <= len + SLACK,
            "{len} B decoded into a {} B buffer",
            output.capacity()
        );
    }
}

#[test]
fn context_decompress_into_allocates_the_declared_size() {
    let mut ctx = zrip::DecompressContext::new();
    for len in [2_001, 1_000_000] {
        let data = compressible(len);
        let frame = zrip::compress(&data, 1).unwrap();
        let mut output = Vec::new();
        ctx.decompress_into(&frame, &mut output).unwrap();
        assert_eq!(output, data);
        assert!(
            output.capacity() <= len + SLACK,
            "{len} B decoded into a {} B buffer",
            output.capacity()
        );
    }
}

#[test]
fn small_blocks_near_the_end_allocate_the_declared_size() {
    use std::io::Write;

    // C zstd ends a block at every flush, so several blocks start less than
    // a full block before the declared end.
    let data = compressible(300_000);
    let mut encoder = zstd::stream::Encoder::new(Vec::new(), 1).unwrap();
    encoder.include_contentsize(true).unwrap();
    encoder
        .set_pledged_src_size(Some(data.len() as u64))
        .unwrap();
    for chunk in data.chunks(10_000) {
        encoder.write_all(chunk).unwrap();
        encoder.flush().unwrap();
    }
    let frame = encoder.finish().unwrap();

    let output = zrip::decompress(&frame).unwrap();
    assert_eq!(output, data);
    assert!(
        output.capacity() <= data.len() + SLACK,
        "{} B decoded into a {} B buffer",
        data.len(),
        output.capacity()
    );
}

#[test]
fn compressed_blocks_past_the_declared_size_are_rejected() {
    let data = compressible(1_000_000);
    let frame = zrip::compress(&data, 1).unwrap();
    let short = with_declared_size(&frame, data.len() as u64 - 1_000);
    // A sequence past the declared size fails as a size mismatch.
    assert_eq!(
        zrip::decompress(&short),
        Err(DecompressError::FrameSizeMismatch)
    );
}

#[test]
fn raw_blocks_past_the_declared_size_are_rejected() {
    let data = incompressible(1_000_000);
    let frame = zrip::compress(&data, 1).unwrap();
    let short = with_declared_size(&frame, data.len() as u64 - 1_000);
    assert_eq!(
        zrip::decompress(&short),
        Err(DecompressError::FrameSizeMismatch)
    );
}

#[test]
fn output_short_of_the_declared_size_is_rejected() {
    let data = compressible(1_000_000);
    let frame = zrip::compress(&data, 1).unwrap();
    let long = with_declared_size(&frame, data.len() as u64 + 1_000);
    assert_eq!(
        zrip::decompress(&long),
        Err(DecompressError::FrameSizeMismatch)
    );
}

/// Compresses `data` into a frame without a content size, as streaming
/// encoders write them.
fn without_content_size(data: &[u8], level: i32) -> Vec<u8> {
    use std::io::Write;

    let mut encoder = zstd::stream::Encoder::new(Vec::new(), level).unwrap();
    encoder.include_contentsize(false).unwrap();
    encoder.write_all(data).unwrap();
    let frame = encoder.finish().unwrap();
    assert_eq!(frame[4] >> 5, 0, "the frame declares its size");
    frame
}

/// Returns `len` bytes in eight runs, each a 36-byte pattern repeated: a few
/// sequences with long matches.
fn long_matches(len: usize) -> Vec<u8> {
    let mut x = 0x2545_f491u32;
    let mut data = Vec::with_capacity(len);
    for run in 0..8 {
        let pattern: Vec<u8> = (0..36)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        let end = len * (run + 1) / 8;
        while data.len() < end {
            data.push(pattern[data.len() % 36]);
        }
    }
    data
}

#[test]
fn small_frames_without_a_size_decode_into_small_buffers() {
    let mut ctx = zrip::DecompressContext::new();
    for len in [300, 2_000, 10_000] {
        let data = compressible(len);
        let frame = without_content_size(&data, 3);
        // The room a block's output gets follows the block's size. It used
        // to be a whole block's 128 KiB, zeroed for every block.
        let most = 64 * frame.len() + SLACK;

        let output = zrip::decompress(&frame).unwrap();
        assert_eq!(output, data);
        assert!(
            output.capacity() <= most,
            "{len} B decoded into a {} B buffer",
            output.capacity()
        );

        let mut output = Vec::new();
        ctx.decompress_into(&frame, &mut output).unwrap();
        assert_eq!(output, data);
        assert!(
            output.capacity() <= most,
            "{len} B decoded into a {} B buffer",
            output.capacity()
        );
    }
}

#[test]
fn a_lone_long_match_decodes() {
    // One sequence, which sets the repeat offsets before it runs, in a block
    // that starts over.
    for len in [264, 1_000, 40_000, 131_072] {
        let data: Vec<u8> = (0..len).map(|i| (i % 4) as u8).collect();
        for level in [1, 3] {
            let frame = zrip::compress(&data, level).unwrap();
            assert_eq!(zrip::decompress(&frame).unwrap(), data);
            let frame = without_content_size(&data, level);
            assert_eq!(zrip::decompress(&frame).unwrap(), data);
        }
    }
}

#[test]
fn long_matches_without_a_size_decode() {
    // These blocks write far more than their first room holds, and start
    // over with all the room a block may use.
    for len in [40_000, 131_072, 400_000] {
        let data = long_matches(len);
        for level in [1, 3] {
            let frame = without_content_size(&data, level);
            assert!(frame.len() * 64 < len, "{len} B at level {level}");
            assert_eq!(zrip::decompress(&frame).unwrap(), data);
            assert_eq!(zrip::decompress_with_limit(&frame, len).unwrap(), data);
            assert_eq!(
                zrip::decompress_with_limit(&frame, len - 1),
                Err(DecompressError::OutputTooSmall)
            );
        }
    }
}
