//! A frame that declares its content size decodes into one allocation of that
//! size plus the block writer's slack.

// These tests encode and decode megabyte inputs and call C zstd, which Miri
// cannot run. Miri covers the same decode paths through smaller frames.
#![cfg(not(miri))]

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
    // A sequence past the limit is rejected as corrupt, as it is for the
    // caller's limit.
    assert_eq!(
        zrip::decompress(&short),
        Err(DecompressError::CorruptSequences)
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
