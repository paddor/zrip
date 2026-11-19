#![no_main]
use libfuzzer_sys::fuzz_target;
use zrip::push::{EndOp, PushDecoder, PushEncoder, Status};

/// Encodes `payload` as one frame in `in_chunk`-byte chunks, flushing every
/// `flush_every` chunks, with an `out_buf`-byte output buffer.
fn encode_frame(
    enc: &mut PushEncoder,
    payload: &[u8],
    in_chunk: usize,
    out_buf: usize,
    flush_every: usize,
) -> Vec<u8> {
    let mut frame = Vec::new();
    let mut buf = vec![0u8; out_buf];
    for (i, chunk) in payload.chunks(in_chunk).enumerate() {
        let op = if flush_every != 0 && i % flush_every == 0 {
            EndOp::Flush
        } else {
            EndOp::Continue
        };
        let mut input = chunk;
        loop {
            let p = enc.encode(input, &mut buf, op).unwrap();
            frame.extend_from_slice(&buf[..p.produced]);
            input = &input[p.consumed..];
            if p.status == Status::NeedsInput {
                assert!(input.is_empty());
                break;
            }
        }
    }
    loop {
        let p = enc.encode(&[], &mut buf, EndOp::End).unwrap();
        frame.extend_from_slice(&buf[..p.produced]);
        if p.status == Status::Finished {
            break;
        }
    }
    frame
}

// Push encode and decode with fuzzer-chosen level, chunk sizes, output
// buffer sizes and flush points. C zstd must decode the frame too. The
// encoder then takes a second frame, which must match a new encoder's.
fuzz_target!(|data: &[u8]| {
    if data.len() < 4 || data.len() > 512 * 1024 {
        return;
    }
    let level = i32::from(data[0] % 12) - 7;
    let in_chunk = usize::from(data[1]) * 97 + 1;
    let out_buf = usize::from(data[2]) * 61 + 1;
    let flush_every = usize::from(data[3] % 8);
    let payload = &data[4..];

    let mut enc = PushEncoder::new(level).unwrap();
    let frame = encode_frame(&mut enc, payload, in_chunk, out_buf, flush_every);

    assert_eq!(zstd::decode_all(&frame[..]).unwrap(), payload);

    let mut dec = PushDecoder::new();
    let mut out = Vec::new();
    let mut dbuf = vec![0u8; in_chunk];
    for chunk in frame.chunks(out_buf) {
        let mut input = chunk;
        loop {
            let p = dec.decode(input, &mut dbuf).unwrap();
            out.extend_from_slice(&dbuf[..p.produced]);
            input = &input[p.consumed..];
            if p.status == Status::NeedsInput {
                break;
            }
        }
    }
    dec.finish().unwrap();
    assert_eq!(out, payload);

    // Other bytes for the second frame, so it hashes to other slots.
    let half = payload.len() / 2;
    let second = [&payload[half..], &payload[..half]].concat();
    let reused = encode_frame(&mut enc, &second, in_chunk, out_buf, flush_every);
    let mut fresh_enc = PushEncoder::new(level).unwrap();
    let fresh = encode_frame(&mut fresh_enc, &second, in_chunk, out_buf, flush_every);
    assert!(reused == fresh, "a reused encoder wrote another frame");
});
