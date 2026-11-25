// Push encoder/decoder tests, cross-checked against C zstd.
#![cfg(feature = "std")]

use zrip::push::{EndOp, Progress, PushDecoder, PushEncoder, Status};
use zrip::{DecompressError, Dictionary};

/// Text-like data with repeats near and far, plus incompressible stretches.
fn mixed(len: usize, seed: u32) -> Vec<u8> {
    let words: [&[u8]; 8] = [
        b"alpha ",
        b"beta ",
        b"gamma ",
        b"delta ",
        b"{\"id\":",
        b"\"name\":",
        b"0123",
        b"\n",
    ];
    let mut out = Vec::with_capacity(len);
    let mut x = seed | 1;
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        if x.is_multiple_of(16) {
            out.extend((0..64).map(|i| (x >> (i % 24)) as u8 ^ i as u8));
        } else {
            out.extend_from_slice(words[(x % 8) as usize]);
        }
    }
    out.truncate(len);
    out
}

/// Encodes with input chunks of `in_chunk` and output buffers of `out_buf`.
fn push_encode(enc: &mut PushEncoder, data: &[u8], in_chunk: usize, out_buf: usize) -> Vec<u8> {
    let mut frame = Vec::new();
    let mut buf = vec![0u8; out_buf];
    for chunk in data.chunks(in_chunk.max(1)) {
        let mut input = chunk;
        loop {
            let p = enc.encode(input, &mut buf, EndOp::Continue).unwrap();
            frame.extend_from_slice(&buf[..p.produced]);
            input = &input[p.consumed..];
            if p.status == Status::NeedsInput {
                assert_eq!(input, &[] as &[u8]);
                break;
            }
            assert_eq!(p.status, Status::NeedsOutput);
        }
    }
    loop {
        let p = enc.encode(&[], &mut buf, EndOp::End).unwrap();
        frame.extend_from_slice(&buf[..p.produced]);
        if p.status == Status::Finished {
            break;
        }
        assert_eq!(p.status, Status::NeedsOutput);
    }
    frame
}

/// Decodes with input chunks of `in_chunk` and output buffers of `out_buf`.
/// Returns the output and how many frames ended.
fn push_decode(
    dec: &mut PushDecoder,
    data: &[u8],
    in_chunk: usize,
    out_buf: usize,
) -> Result<(Vec<u8>, usize), DecompressError> {
    let mut out = Vec::new();
    let mut frames = 0;
    let mut buf = vec![0u8; out_buf];
    for chunk in data.chunks(in_chunk.max(1)) {
        let mut input = chunk;
        loop {
            let p: Progress = dec.decode(input, &mut buf)?;
            out.extend_from_slice(&buf[..p.produced]);
            input = &input[p.consumed..];
            match p.status {
                Status::NeedsInput => {
                    assert_eq!(input, &[] as &[u8]);
                    break;
                }
                Status::Finished => frames += 1,
                Status::NeedsOutput => assert_eq!(p.produced, out_buf),
            }
        }
    }
    loop {
        let p = dec.decode(&[], &mut buf)?;
        out.extend_from_slice(&buf[..p.produced]);
        match p.status {
            Status::NeedsInput => break,
            Status::Finished => frames += 1,
            Status::NeedsOutput => {}
        }
    }
    dec.finish()?;
    Ok((out, frames))
}

#[test]
fn encode_roundtrip_chunk_and_buffer_sizes() {
    let data = mixed(300_000, 7);
    for level in [-7, -1, 1, 3, 4] {
        let mut enc = PushEncoder::new(level).unwrap();
        for (in_chunk, out_buf) in [(1 << 20, 1 << 20), (1, 4096), (4096, 1), (7, 13)] {
            let frame = push_encode(&mut enc, &data, in_chunk, out_buf);
            assert_eq!(
                zstd::decode_all(&frame[..]).unwrap(),
                data,
                "L{level} in={in_chunk} out={out_buf}"
            );
            assert_eq!(zrip::decompress(&frame).unwrap(), data);
        }
    }
}

#[test]
fn encode_sizes_around_block_boundaries() {
    let block = zrip::frame::MAX_BLOCK_SIZE;
    let mut enc = PushEncoder::new(1).unwrap();
    for len in [0, 1, block - 1, block, block + 1, 2 * block, 2 * block + 5] {
        let data = mixed(len, len as u32);
        for in_chunk in [1 << 20, block, 1000] {
            let frame = push_encode(&mut enc, &data, in_chunk, 1 << 20);
            assert_eq!(zstd::decode_all(&frame[..]).unwrap(), data, "len={len}");
        }
    }
}

#[test]
fn encode_to_vec_matches_bounded_encode() {
    let data = mixed(400_000, 3);
    let mut a = PushEncoder::new(3).unwrap();
    let mut b = PushEncoder::new(3).unwrap();
    let bounded = push_encode(&mut a, &data, 5000, 333);
    let mut to_vec = Vec::new();
    for chunk in data.chunks(5000) {
        b.encode_to_vec(chunk, &mut to_vec, EndOp::Continue)
            .unwrap();
    }
    b.encode_to_vec(&[], &mut to_vec, EndOp::End).unwrap();
    assert_eq!(bounded, to_vec);
}

#[test]
fn encode_flush_makes_prefix_decodable() {
    let data = mixed(50_000, 11);
    let mut enc = PushEncoder::new(1).unwrap();
    let mut dec = PushDecoder::new();
    let mut frame = Vec::new();
    let mut decoded = Vec::new();
    for chunk in data.chunks(3000) {
        let before = frame.len();
        enc.encode_to_vec(chunk, &mut frame, EndOp::Flush).unwrap();
        // Everything written so far decodes, without the frame end.
        dec.decode_to_vec(&frame[before..], &mut decoded).unwrap();
        assert_eq!(decoded, data[..decoded.len()]);
        assert_eq!(
            decoded.len(),
            (chunk.as_ptr() as usize - data.as_ptr() as usize) + chunk.len()
        );
    }
    enc.encode_to_vec(&[], &mut frame, EndOp::End).unwrap();
    assert_eq!(zstd::decode_all(&frame[..]).unwrap(), data);
}

#[test]
fn encode_frames_back_to_back_and_reset() {
    let a = mixed(10_000, 1);
    let b = mixed(200_000, 2);
    let mut enc = PushEncoder::new(1).unwrap();
    let mut out = Vec::new();
    enc.encode_to_vec(&a, &mut out, EndOp::End).unwrap();
    let first = out.len();
    enc.encode_to_vec(&b, &mut out, EndOp::End).unwrap();
    assert_eq!(zrip::decompress(&out[..first]).unwrap(), a);
    assert_eq!(zrip::decompress(&out[first..]).unwrap(), b);

    // Reset drops a half-written frame.
    let mut partial = Vec::new();
    enc.encode_to_vec(&b, &mut partial, EndOp::Continue)
        .unwrap();
    enc.reset();
    let mut fresh = Vec::new();
    enc.encode_to_vec(&a, &mut fresh, EndOp::End).unwrap();
    assert_eq!(fresh, out[..first]);
}

#[test]
fn encode_options_and_dict() {
    let data = mixed(1 << 20, 5);
    let opts = zrip::Options::default().window_log(20).ldm(true);
    let mut enc = PushEncoder::with_options(1, &opts).unwrap();
    let frame = push_encode(&mut enc, &data, 70_000, 1 << 16);
    assert_eq!(zstd::decode_all(&frame[..]).unwrap(), data);

    let samples: Vec<Vec<u8>> = (0..200).map(|i| mixed(300, i)).collect();
    let dict_bytes = zstd::dict::from_samples(&samples, 4096).unwrap();
    let dict = Dictionary::from_bytes(&dict_bytes).unwrap();
    let mut enc = PushEncoder::with_dict(3, dict.clone()).unwrap();
    let mut dec = PushDecoder::with_dict(dict);
    let mut cdec = zstd::bulk::Decompressor::with_dictionary(&dict_bytes).unwrap();
    for sample in &samples[..20] {
        let frame = push_encode(&mut enc, sample, 50, 17);
        assert_eq!(cdec.decompress(&frame, sample.len()).unwrap(), *sample);
        let (out, frames) = push_decode(&mut dec, &frame, 9, 31).unwrap();
        assert_eq!(out, *sample);
        assert_eq!(frames, 1);
    }
}

#[test]
fn encode_invalid_level() {
    assert!(PushEncoder::new(5).is_err());
}

#[test]
fn decode_c_frames_every_chunking() {
    let data = mixed(400_000, 9);
    let mut dec = PushDecoder::new();
    for level in [1, 3, 9, 19] {
        let frame = zstd::bulk::compress(&data, level).unwrap();
        for (in_chunk, out_buf) in [
            (1 << 20, 1 << 20),
            (1, 1 << 20),
            (1 << 20, 1),
            (3, 7),
            (4096, 4096),
            (131_075, 131_072),
        ] {
            let (out, frames) = push_decode(&mut dec, &frame, in_chunk, out_buf).unwrap();
            assert_eq!(out, data, "L{level} in={in_chunk} out={out_buf}");
            assert_eq!(frames, 1);
        }
    }
}

#[test]
fn decode_every_split_point() {
    let data = mixed(20_000, 4);
    let mut frame = zstd::stream::encode_all(&data[..], 3).unwrap();
    let skippable = [0x50, 0x2A, 0x4D, 0x18, 3, 0, 0, 0, 1, 2, 3];
    frame.extend_from_slice(&skippable);
    frame.extend_from_slice(&zrip::compress(&data[..100], 1).unwrap());
    let mut want = data.clone();
    want.extend_from_slice(&data[..100]);

    let mut dec = PushDecoder::new();
    let mut buf = vec![0u8; 1 << 18];
    for split in 0..=frame.len() {
        let mut out = Vec::new();
        for part in [&frame[..split], &frame[split..]] {
            let mut input = part;
            loop {
                let p = dec.decode(input, &mut buf).unwrap();
                out.extend_from_slice(&buf[..p.produced]);
                input = &input[p.consumed..];
                if p.status == Status::NeedsInput {
                    break;
                }
            }
        }
        dec.finish().unwrap();
        assert_eq!(out, want, "split={split}");
    }
}

#[test]
fn decode_multi_frame_and_skippable() {
    let a = mixed(1000, 1);
    let b = mixed(150_000, 2);
    let mut stream = vec![0x5E, 0x2A, 0x4D, 0x18, 0, 0, 0, 0]; // empty skippable
    stream.extend(zstd::bulk::compress(&a, 1).unwrap());
    stream.extend([0x50, 0x2A, 0x4D, 0x18, 5, 0, 0, 0, 9, 9, 9, 9, 9]);
    stream.extend(zstd::stream::encode_all(&b[..], 5).unwrap());
    stream.extend(zrip::compress(&[], 1).unwrap());

    let mut dec = PushDecoder::new();
    for in_chunk in [1, 2, 5, 1 << 20] {
        let (out, frames) = push_decode(&mut dec, &stream, in_chunk, 999).unwrap();
        assert_eq!(out, [&a[..], &b[..]].concat());
        assert_eq!(frames, 5, "in={in_chunk}");
    }

    let mut to_vec = Vec::new();
    for chunk in stream.chunks(777) {
        dec.decode_to_vec(chunk, &mut to_vec).unwrap();
    }
    dec.finish().unwrap();
    assert_eq!(to_vec, [&a[..], &b[..]].concat());
}

#[test]
fn decode_finished_stops_at_frame_end() {
    let a = zrip::compress(b"first", 1).unwrap();
    let b = zrip::compress(b"second", 1).unwrap();
    let both = [&a[..], &b[..]].concat();
    let mut dec = PushDecoder::new();
    let mut buf = [0u8; 64];
    let p = dec.decode(&both, &mut buf).unwrap();
    assert_eq!(p.status, Status::Finished);
    assert_eq!(p.consumed, a.len());
    assert_eq!(&buf[..p.produced], b"first");
    let p = dec.decode(&both[a.len()..], &mut buf).unwrap();
    assert_eq!(p.status, Status::Finished);
    assert_eq!(&buf[..p.produced], b"second");
}

#[test]
fn decode_truncated_frames_fail_finish() {
    let data = mixed(5000, 8);
    let frame = zstd::stream::encode_all(&data[..], 3).unwrap();
    let mut dec = PushDecoder::new();
    for cut in 1..frame.len() {
        dec.reset();
        let mut out = Vec::new();
        dec.decode_to_vec(&frame[..cut], &mut out).unwrap();
        assert_eq!(
            dec.finish(),
            Err(DecompressError::InputExhausted),
            "cut={cut}"
        );
    }
    dec.reset();
    dec.finish().unwrap();
}

#[test]
fn decode_errors_stick_until_reset() {
    let data = mixed(5000, 8);
    let mut c = zstd::stream::Encoder::new(Vec::new(), 3).unwrap();
    c.include_checksum(true).unwrap();
    std::io::Write::write_all(&mut c, &data).unwrap();
    let mut frame = c.finish().unwrap();
    let n = frame.len();
    frame[n - 1] ^= 1; // checksum
    let mut dec = PushDecoder::new();
    let mut out = Vec::new();
    let err = dec.decode_to_vec(&frame, &mut out).unwrap_err();
    assert!(matches!(err, DecompressError::ChecksumMismatch { .. }));
    assert_eq!(dec.decode_to_vec(&[], &mut out), Err(err.clone()));
    assert_eq!(dec.finish(), Err(err));
    dec.reset();
    frame[n - 1] ^= 1;
    out.clear();
    dec.decode_to_vec(&frame, &mut out).unwrap();
    assert_eq!(out, data);
}

#[test]
fn decode_limits() {
    let data = mixed(300_000, 6);
    let frame = zstd::stream::encode_all(&data[..], 3).unwrap();
    let mut out = Vec::new();
    let err = PushDecoder::with_limit(100_000)
        .decode_to_vec(&frame, &mut out)
        .unwrap_err();
    assert_eq!(err, DecompressError::OutputTooSmall);
    let mut reader = zrip::FrameDecoder::with_limit(&frame[..], 100_000);
    let err = std::io::Read::read_to_end(&mut reader, &mut Vec::new()).unwrap_err();
    let inner = err
        .get_ref()
        .and_then(|e| e.downcast_ref::<DecompressError>());
    assert_eq!(inner, Some(&DecompressError::OutputTooSmall));

    let mut c = zstd::stream::Encoder::new(Vec::new(), 3).unwrap();
    c.window_log(20).unwrap();
    std::io::Write::write_all(&mut c, &data).unwrap();
    let frame = c.finish().unwrap();
    let mut out = Vec::new();
    let err = PushDecoder::new()
        .max_window(1 << 19)
        .decode_to_vec(&frame, &mut out)
        .unwrap_err();
    assert!(
        matches!(err, DecompressError::WindowTooLarge { .. }),
        "{err:?}"
    );
    PushDecoder::new()
        .max_window(1 << 20)
        .decode_to_vec(&frame, &mut out)
        .unwrap();
    assert_eq!(out, data);

    // A window above the default 128 MiB needs an explicit cap. C zstd caps
    // windows at 2^30 on 32-bit targets.
    let mut c = zstd::stream::Encoder::new(Vec::new(), 3).unwrap();
    c.window_log(if cfg!(target_pointer_width = "32") {
        30
    } else {
        31
    })
    .unwrap();
    std::io::Write::write_all(&mut c, &data).unwrap();
    let frame = c.finish().unwrap();
    let mut out = Vec::new();
    let err = PushDecoder::new()
        .decode_to_vec(&frame, &mut out)
        .unwrap_err();
    assert!(
        matches!(err, DecompressError::WindowTooLarge { .. }),
        "{err:?}"
    );
    PushDecoder::new()
        .max_window(1 << 31)
        .decode_to_vec(&frame, &mut out)
        .unwrap();
    assert_eq!(out, data);
}

#[test]
fn one_pass_frames_match_one_shot() {
    // A frame that ends before any of it went out is encoded in one pass.
    let block = zrip::frame::MAX_BLOCK_SIZE;
    for level in [-7, 1, 3, 4] {
        for len in [0, 1000, block, block + 1, 300_000] {
            let data = mixed(len, len as u32);
            let want = zrip::compress(&data, level).unwrap();

            let mut enc = PushEncoder::new(level).unwrap();
            let mut out = Vec::new();
            enc.encode_to_vec(&data, &mut out, EndOp::End).unwrap();
            assert_eq!(out, want, "L{level} len={len} one call");

            // Buffered under a block, then ended: still one pass.
            if len <= block {
                let mut out = Vec::new();
                for chunk in data.chunks(333) {
                    enc.encode_to_vec(chunk, &mut out, EndOp::Continue).unwrap();
                }
                enc.encode_to_vec(&[], &mut out, EndOp::End).unwrap();
                assert_eq!(out, want, "L{level} len={len} chunked");
            }

            let frame = push_encode(&mut enc, &data, 1 << 20, 7);
            assert_eq!(zstd::decode_all(&frame[..]).unwrap(), data);
        }
    }

    let samples: Vec<Vec<u8>> = (0..200).map(|i| mixed(300, i)).collect();
    let dict_bytes = zstd::dict::from_samples(&samples, 4096).unwrap();
    let dict = Dictionary::from_bytes(&dict_bytes).unwrap();
    let mut enc = PushEncoder::with_dict(3, dict.clone()).unwrap();
    let mut out = Vec::new();
    enc.encode_to_vec(&samples[0], &mut out, EndOp::End)
        .unwrap();
    assert_eq!(
        out,
        zrip::compress_with_dict(&samples[0], 3, &dict).unwrap()
    );

    let opts = zrip::Options::default().window_log(20).ldm(true);
    let data = mixed(200_000, 3);
    let mut enc = PushEncoder::with_options(1, &opts).unwrap();
    let mut out = Vec::new();
    enc.encode_to_vec(&data, &mut out, EndOp::End).unwrap();
    assert_eq!(out, zrip::compress_opts(&data, 1, &opts).unwrap());
}

#[test]
fn encode_to_vec_after_pending_frame_end() {
    let a = mixed(50_000, 1);
    let b = mixed(1000, 2);
    let want = [
        zrip::compress(&a, 1).unwrap(),
        zrip::compress(&b, 1).unwrap(),
    ]
    .concat();
    let mut enc = PushEncoder::new(1).unwrap();
    let mut buf = [0u8; 16];

    // Input after a pending frame end starts the next frame.
    let p = enc.encode(&a, &mut buf, EndOp::End).unwrap();
    assert_eq!(p.status, Status::NeedsOutput);
    let mut out = buf[..p.produced].to_vec();
    enc.encode_to_vec(&b, &mut out, EndOp::End).unwrap();
    assert_eq!(out, want);

    // No input only finishes the pending frame.
    let p = enc.encode(&a, &mut buf, EndOp::End).unwrap();
    assert_eq!(p.status, Status::NeedsOutput);
    let mut out = buf[..p.produced].to_vec();
    enc.encode_to_vec(&[], &mut out, EndOp::End).unwrap();
    assert!(!enc.has_pending_output());
    enc.encode_to_vec(&b, &mut out, EndOp::End).unwrap();
    assert_eq!(out, want);
}

#[test]
fn decode_bad_magic_fails_early() {
    let mut dec = PushDecoder::new();
    assert_eq!(
        dec.decode(b"hell", &mut [0u8; 8]),
        Err(DecompressError::BadMagic)
    );
    let mut reader = zrip::FrameDecoder::new(&b"hello"[..]);
    let err = std::io::Read::read_to_end(&mut reader, &mut Vec::new()).unwrap_err();
    let inner = err
        .get_ref()
        .and_then(|e| e.downcast_ref::<DecompressError>());
    assert_eq!(inner, Some(&DecompressError::BadMagic));
}

#[test]
fn decode_limit_counts_earlier_frames() {
    let a = zrip::compress(&mixed(1000, 1), 1).unwrap();
    let b = zrip::compress(&mixed(1000, 2), 1).unwrap();
    let header = zrip::frame::header::parse_frame_header(&b).unwrap();
    assert_eq!(header.frame_content_size, Some(1000));
    // The second frame's output crosses the limit. As in C zstd, its
    // content size alone does not fail.
    let input = [&a[..], &b[..header.header_size]].concat();
    let mut out = Vec::new();
    PushDecoder::with_limit(1500)
        .decode_to_vec(&input, &mut out)
        .unwrap();
    let input = [&a[..], &b[..]].concat();
    let mut out = Vec::new();
    let err = PushDecoder::with_limit(1500)
        .decode_to_vec(&input, &mut out)
        .unwrap_err();
    assert_eq!(err, DecompressError::OutputTooSmall);
    assert_eq!(out.len(), 1000);
}

#[test]
fn uninit_buffers_match_initialized_ones() {
    use std::mem::MaybeUninit;

    let data = mixed(300_000, 11);
    for (level, out_buf) in [(1, 13), (3, 4096), (-7, 1 << 20)] {
        let expected = push_encode(&mut PushEncoder::new(level).unwrap(), &data, 1000, out_buf);

        let mut enc = PushEncoder::new(level).unwrap();
        let mut frame = Vec::new();
        let mut buf = vec![MaybeUninit::uninit(); out_buf];
        for (input, op) in [(&data[..], EndOp::Continue), (&[][..], EndOp::End)] {
            let mut input = input;
            loop {
                let p = enc.encode_uninit(input, &mut buf, op).unwrap();
                frame.extend(buf[..p.produced].iter().map(|b| unsafe { b.assume_init() }));
                input = &input[p.consumed..];
                if p.status != Status::NeedsOutput {
                    break;
                }
            }
        }
        assert_eq!(frame, expected, "L{level} out={out_buf}");

        let mut dec = PushDecoder::new();
        let mut out = Vec::new();
        let mut input = &frame[..];
        loop {
            let p = dec.decode_uninit(input, &mut buf).unwrap();
            out.extend(buf[..p.produced].iter().map(|b| unsafe { b.assume_init() }));
            input = &input[p.consumed..];
            if p.status == Status::Finished {
                break;
            }
        }
        dec.finish().unwrap();
        assert_eq!(out, data, "L{level} out={out_buf}");
    }
}

#[test]
fn at_frame_end_holds_until_the_next_frame() {
    let a = zrip::compress(&mixed(200_000, 5), 1).unwrap();
    let b = zrip::compress(b"second", 1).unwrap();
    let both = [&a[..], &b[..]].concat();
    let mut dec = PushDecoder::new();
    assert!(!dec.at_frame_end());

    // Output still pending after all of `a` was read: not at the end yet.
    let mut buf = [0u8; 4096];
    let mut input = &both[..];
    loop {
        let p = dec.decode(input, &mut buf).unwrap();
        input = &input[p.consumed..];
        if p.status == Status::Finished {
            break;
        }
        assert!(!dec.at_frame_end());
    }
    assert!(dec.at_frame_end());

    // The one `Finished` was seen; empty calls keep the frame end.
    assert_eq!(
        dec.decode(&[], &mut buf).unwrap().status,
        Status::NeedsInput
    );
    assert!(dec.at_frame_end());

    // The next frame's first bytes end it.
    let p = dec.decode(&input[..2], &mut buf).unwrap();
    assert_eq!(p.consumed, 2);
    assert!(!dec.at_frame_end());
    let p = dec.decode(&input[2..], &mut buf).unwrap();
    assert_eq!(p.status, Status::Finished);
    assert!(dec.at_frame_end());
    dec.reset();
    assert!(!dec.at_frame_end());

    let mut out = Vec::new();
    dec.decode_to_vec(&both, &mut out).unwrap();
    assert!(dec.at_frame_end());
    dec.decode_to_vec(&b[..3], &mut out).unwrap();
    assert!(!dec.at_frame_end());
}

/// Skewed literals with few repeats: about 5 bits per byte.
fn skewed(range: core::ops::Range<u32>) -> Vec<u8> {
    range
        .map(|i| {
            let r = i
                .wrapping_mul(0x9E37_79B1)
                .rotate_left(13)
                .wrapping_mul(0x9E37_79B1);
            ((r >> 26) & (r >> 20) & 63) as u8 + b' '
        })
        .collect()
}

/// Types of the blocks in a frame, and of their literal sections.
fn block_types(frame: &[u8]) -> Vec<(u8, Option<u8>)> {
    let header = zrip::frame::header::parse_frame_header(frame).unwrap();
    let mut pos = header.header_size;
    let mut types = Vec::new();
    loop {
        let h = u32::from_le_bytes([frame[pos], frame[pos + 1], frame[pos + 2], 0]);
        let (last, kind, size) = (h & 1 == 1, ((h >> 1) & 3) as u8, (h >> 3) as usize);
        pos += 3;
        types.push((kind, (kind == 2).then(|| frame[pos] & 3)));
        pos += if kind == 1 { 1 } else { size };
        if last {
            return types;
        }
    }
}

#[test]
fn treeless_literals_after_a_raw_block_decode() {
    let first = skewed(0..16 * 1024);
    // Under 4 KiB, so the flat-block sampler leaves them to the block
    // encoder, which falls back to raw literals or a raw block.
    let random = |len: usize, mut x: u32| -> Vec<u8> {
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 24) as u8
            })
            .collect()
    };
    let dense = random(3000, 1);
    // Dense bytes, each 256-byte run repeated: a compressed block with raw
    // literals.
    let repeated: Vec<u8> = random(1024, 2)
        .chunks(256)
        .flat_map(|c| [c, c].concat())
        .collect();
    let third = skewed(1 << 20..(1 << 20) + 1024);
    // L3 streams with sparse probes, which miss the repeats after the first
    // block and store the second block raw.
    for level in [1, 4] {
        let mut enc = PushEncoder::new(level).unwrap();
        let mut frame = Vec::new();
        let mut buf = vec![0u8; 1 << 16];
        for (data, op) in [
            (&first, EndOp::Flush),
            (&repeated, EndOp::Flush),
            (&dense, EndOp::Flush),
            (&third, EndOp::End),
        ] {
            let mut input = &data[..];
            loop {
                let p = enc.encode(input, &mut buf, op).unwrap();
                frame.extend_from_slice(&buf[..p.produced]);
                input = &input[p.consumed..];
                if p.status != Status::NeedsOutput {
                    break;
                }
            }
        }
        // Huffman literals with a tree, raw literals, a raw block, then
        // literals that reuse the first block's table across both.
        assert_eq!(
            block_types(&frame),
            [(2, Some(2)), (2, Some(0)), (0, None), (2, Some(3))],
            "level {level}"
        );
        let all = [&first[..], &repeated, &dense, &third].concat();
        assert_eq!(zstd::decode_all(&frame[..]).unwrap(), all, "level {level}");
        assert_eq!(zrip::decompress(&frame).unwrap(), all, "level {level}");
    }
}

#[test]
fn full_buffer_stops_before_the_next_block() {
    // A 64-byte RLE block, then a block header of the reserved type.
    let frame = [
        0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x00, 0x02, 0x02, 0x00, b'a', 0x0f, 0x00, 0x00,
    ];
    let mut dec = PushDecoder::new();
    let mut buf = [0u8; 64];
    let p = dec.decode(&frame, &mut buf).unwrap();
    let full = Progress {
        consumed: 10,
        produced: 64,
        status: Status::NeedsOutput,
    };
    assert_eq!(p, full);
    assert_eq!(buf, [b'a'; 64]);
    assert!(dec.decode(&frame[10..], &mut buf).is_err());

    dec.reset();
    let mut buf = [core::mem::MaybeUninit::uninit(); 64];
    assert_eq!(dec.decode_uninit(&frame, &mut buf).unwrap(), full);

    // A buffer with room for more reaches the corrupt header.
    dec.reset();
    assert!(dec.decode(&frame, &mut [0u8; 65]).is_err());
}

#[test]
fn frames_past_the_header_reserve_decode() {
    // 20 MiB with its content size, past the 8 MiB a header reserves: a
    // single-segment zrip frame, and a C zstd frame with an 8 MiB window.
    let data = mixed(20 << 20, 11);
    let mut c = zstd::stream::Encoder::new(Vec::new(), 1).unwrap();
    c.set_pledged_src_size(Some(data.len() as u64)).unwrap();
    c.window_log(23).unwrap();
    std::io::Write::write_all(&mut c, &data).unwrap();
    let frames = [zrip::compress(&data, 1).unwrap(), c.finish().unwrap()];
    for frame in &frames {
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut zrip::FrameDecoder::new(frame.as_slice()), &mut out)
            .unwrap();
        assert_eq!(out, data);
        for limit in [usize::MAX, data.len()] {
            let mut dec = PushDecoder::with_limit(limit);
            out.clear();
            for part in frame.chunks(100_000) {
                dec.decode_to_vec(part, &mut out).unwrap();
            }
            dec.finish().unwrap();
            assert_eq!(out, data, "limit {limit}");
        }
    }
}

#[test]
fn blocks_decode_over_the_room_earlier_blocks_left() {
    // A streaming decoder keeps the room past its output, which earlier
    // blocks wrote, instead of zeroing it for each block. Large blocks fill
    // it, then small flushed blocks, raw and RLE blocks, and the next frames
    // decode over it, through window shifts.
    use std::io::Write;
    let mut messages = vec![mixed(150_000, 21)];
    for i in 0..300u32 {
        messages.push(mixed(20 + (i as usize * 37) % 600, i));
    }
    messages.push(
        (0..70_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect(),
    );
    messages.push(vec![b'z'; 50_000]);
    messages.push(mixed(90_000, 22));
    messages.extend((0..50).map(|i| mixed(40, 100 + i)));

    let mut c = zstd::stream::Encoder::new(Vec::new(), 3).unwrap();
    c.window_log(16).unwrap();
    c.include_checksum(true).unwrap();
    for m in &messages {
        c.write_all(m).unwrap();
        c.flush().unwrap();
    }
    let mut stream = c.finish().unwrap();
    let mut enc = PushEncoder::new(1).unwrap();
    let mut z = Vec::new();
    for m in &messages {
        enc.encode_to_vec(m, &mut z, EndOp::Flush).unwrap();
    }
    enc.encode_to_vec(&[], &mut z, EndOp::End).unwrap();
    stream.extend(&z);
    stream.extend(zstd::bulk::compress(&messages[1], 3).unwrap());
    let data = messages.concat();
    let expected = [&data[..], &data[..], &messages[1][..]].concat();

    let mut dec = PushDecoder::new();
    for (in_chunk, out_buf) in [(1 << 20, 1 << 20), (333, 999), (7, 64)] {
        let (out, frames) = push_decode(&mut dec, &stream, in_chunk, out_buf).unwrap();
        assert_eq!(out, expected, "in={in_chunk} out={out_buf}");
        assert_eq!(frames, 3);
    }
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut zrip::FrameDecoder::new(stream.as_slice()), &mut out).unwrap();
    assert_eq!(out, expected);
}

#[test]
fn a_failed_table_description_leaves_later_frames_alone() {
    use std::io::Read;

    // One sequence: literal lengths in RLE mode, then an offset table
    // description that ends early. The first table is replaced, the block
    // fails.
    let corrupt = [
        0x28, 0xB5, 0x2F, 0xFD, 0x24, 0x11, 0x3D, 0x00, 0x00, 0x08, 0x17, 0x01, 0x60, 0x08, 0x10,
        0x02, 0x30, 0xCD, 0x5F, 0x23,
    ];
    let data = mixed(5_000, 3);
    // The decoders keep the tables of the last block and reuse them for a
    // block that describes the same ones, as this frame does after itself.
    let valid = zrip::compress(&data, 1).unwrap();

    let mut dec = PushDecoder::new();
    let mut out = Vec::new();
    dec.decode_to_vec(&valid, &mut out).unwrap();
    assert_eq!(out, data);
    dec.reset();
    assert!(dec.decode_to_vec(&corrupt, &mut Vec::new()).is_err());
    dec.reset();
    out.clear();
    dec.decode_to_vec(&valid, &mut out).unwrap();
    assert_eq!(out, data);

    let mut dec = zrip::FrameDecoder::new(&valid[..]);
    out.clear();
    dec.read_to_end(&mut out).unwrap();
    assert_eq!(out, data);
    dec.reset(&corrupt[..]);
    assert!(dec.read_to_end(&mut Vec::new()).is_err());
    dec.reset(&valid[..]);
    out.clear();
    dec.read_to_end(&mut out).unwrap();
    assert_eq!(out, data);
}
