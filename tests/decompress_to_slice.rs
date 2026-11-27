//! `decompress_to_slice` decodes into a caller's buffer exactly what
//! `decompress` returns, fails with `OutputTooSmall` when the buffer is one
//! byte short, and writes nothing past the output when the frame declares
//! its size.

use std::io::Write;
use zrip::DecompressError;

/// Bytes the tests fill buffers with, so stale bytes show.
const FILL: u8 = 0xA5;

fn xorshift(seed: u32) -> impl FnMut() -> u32 {
    let mut x = seed;
    move || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    }
}

/// Inputs of `len` bytes that end in different kinds of sequences.
fn inputs(len: usize) -> Vec<(&'static str, Vec<u8>)> {
    let mut r = xorshift(0x9e37_79b9);
    let random: Vec<u8> = (0..len).map(|_| r() as u8).collect();
    let mut r = xorshift(0x1234_5678);
    let text: Vec<u8> = (0..len)
        .map(|i| {
            if i % 64 < 48 {
                b"abcdefghijklmnop"[i % 16]
            } else {
                b'a' + (r() % 26) as u8
            }
        })
        .collect();
    // A long run at the end: its last match has offset 1.
    let mut run = random[..len / 4].to_vec();
    run.resize(len, 0);
    // A short period at the end: overlapping matches near the buffer end.
    let mut period = random[..len / 2].to_vec();
    period.extend((0..len - len / 2).map(|i| b"xyz"[i % 3]));
    let json: Vec<u8> = (0..)
        .flat_map(|i: u32| format!(r#"{{"id":{i},"v":{}}},"#, i * 7 % 1000).into_bytes())
        .take(len)
        .collect();
    vec![
        ("random", random),
        ("text", text),
        ("run", run),
        ("period", period),
        ("json", json),
    ]
}

/// Decodes `frame` into a buffer of `capacity` fill bytes, both with the
/// free function and with a context, and checks they agree.
fn to_slice(frame: &[u8], capacity: usize) -> (Result<usize, DecompressError>, Vec<u8>) {
    let mut buf = vec![FILL; capacity];
    let r = zrip::decompress_to_slice(frame, &mut buf);
    let mut ctx_buf = vec![FILL; capacity];
    let mut ctx = zrip::DecompressContext::new();
    // Twice, so the second call runs on warm tables.
    for _ in 0..2 {
        ctx_buf.fill(FILL);
        assert_eq!(ctx.decompress_to_slice(frame, &mut ctx_buf), r);
    }
    if r.is_ok() {
        assert_eq!(ctx_buf, buf);
    }
    (r, buf)
}

/// Checks `frame`, which decodes to `data`, into exact, larger and short
/// buffers. With `sized`, the frame declares its size, so nothing past it
/// may change.
fn check(name: &str, frame: &[u8], data: &[u8], sized: bool) {
    assert_eq!(zrip::decompress(frame).unwrap(), data, "{name}");
    let (r, buf) = to_slice(frame, data.len());
    assert_eq!(r, Ok(data.len()), "{name}: exact");
    assert!(buf == data, "{name}: exact output");

    let (r, buf) = to_slice(frame, data.len() + 100);
    assert_eq!(r, Ok(data.len()), "{name}: larger");
    assert!(&buf[..data.len()] == data, "{name}: larger output");
    if sized {
        assert!(
            buf[data.len()..].iter().all(|&b| b == FILL),
            "{name}: past the output"
        );
    }

    if !data.is_empty() {
        // With a declared size the frame fails up front, without one where
        // it runs out of room.
        assert_eq!(
            to_slice(frame, data.len() - 1).0,
            Err(DecompressError::OutputTooSmall),
            "{name}: short"
        );
    }
}

#[test]
fn zrip_frames() {
    for len in [
        0, 1, 7, 31, 63, 64, 65, 100, 1000, 4096, 16384, 131_072, 131_073, 300_000,
    ] {
        for (kind, data) in inputs(len) {
            for level in [-7, 1, 3, 4] {
                let frame = zrip::compress(&data, level).unwrap();
                check(&format!("zrip {kind} {len} L{level}"), &frame, &data, true);
            }
        }
    }
}

#[test]
fn c_frames() {
    for len in [1, 65, 1000, 4096, 16384, 131_073, 300_000] {
        for (kind, data) in inputs(len) {
            for level in [1, 3, 9, 19] {
                let frame = zstd::bulk::compress(&data, level).unwrap();
                check(&format!("C {kind} {len} L{level}"), &frame, &data, true);
            }
        }
    }
}

/// Frames that do not declare their size, so blocks get the room of the
/// whole buffer.
#[test]
fn frames_without_content_size() {
    for len in [1, 1000, 16384, 300_000] {
        for (kind, data) in inputs(len) {
            let mut encoder = zstd::stream::Encoder::new(Vec::new(), 3).unwrap();
            encoder.include_contentsize(false).unwrap();
            encoder.write_all(&data).unwrap();
            let frame = encoder.finish().unwrap();
            check(&format!("C unsized {kind} {len}"), &frame, &data, false);
        }
    }
}

#[test]
fn several_frames_and_skippable_frames() {
    let a = b"first frame ".repeat(50);
    let b = b"second".repeat(300);
    let mut stream = zrip::compress(&a, 1).unwrap();
    stream.extend_from_slice(&[0x50, 0x2A, 0x4D, 0x18, 3, 0, 0, 0, 1, 2, 3]);
    stream.extend_from_slice(&zrip::compress(&b, 3).unwrap());
    let data = [&a[..], &b[..]].concat();
    check("two frames", &stream, &data, true);
    // The second frame does not fit: it fails even though the first did.
    assert_eq!(
        to_slice(&stream, a.len() + 10).0,
        Err(DecompressError::OutputTooSmall)
    );
    // Nothing but skippable frames: no output.
    assert_eq!(to_slice(&[0x50, 0x2A, 0x4D, 0x18, 0, 0, 0, 0], 0).0, Ok(0));
}

#[test]
fn trailing_bytes_fail() {
    let data = b"trailing".repeat(20);
    let mut frame = zrip::compress(&data, 1).unwrap();
    frame.push(0);
    let mut buf = vec![0; data.len()];
    assert!(zrip::decompress_to_slice(&frame, &mut buf).is_err());
}

#[test]
fn dictionary_context() {
    let samples: Vec<Vec<u8>> = (0..200)
        .map(|i| format!(r#"{{"user":"u{i}","plan":"pro","region":"eu-{}"}}"#, i % 7).into_bytes())
        .collect();
    let raw = zstd::dict::from_samples(&samples, 4096).unwrap();
    let dict = zrip::Dictionary::from_bytes(&raw).unwrap();
    let data = br#"{"user":"u77","plan":"pro","region":"eu-0"}"#;
    let frame = zrip::compress_with_dict(data, 3, &dict).unwrap();

    let mut ctx = zrip::DecompressContext::with_dict(dict);
    for _ in 0..3 {
        let mut buf = vec![FILL; data.len()];
        assert_eq!(ctx.decompress_to_slice(&frame, &mut buf), Ok(data.len()));
        assert_eq!(&buf[..], &data[..]);
    }
    let mut buf = vec![FILL; data.len() - 1];
    assert_eq!(
        ctx.decompress_to_slice(&frame, &mut buf),
        Err(DecompressError::OutputTooSmall)
    );
    // Without the dictionary the frame fails.
    let mut buf = vec![FILL; data.len()];
    assert_eq!(
        zrip::decompress_to_slice(&frame, &mut buf),
        Err(DecompressError::DictRequired)
    );
}

/// A frame that declares more than the buffer holds fails before decoding.
#[test]
fn declared_size_over_the_buffer() {
    let data = vec![7u8; 10_000];
    let frame = zrip::compress(&data, 1).unwrap();
    let mut buf = vec![FILL; 9_999];
    assert_eq!(
        zrip::decompress_to_slice(&frame, &mut buf),
        Err(DecompressError::OutputTooSmall)
    );
    assert!(buf.iter().all(|&b| b == FILL));
}
