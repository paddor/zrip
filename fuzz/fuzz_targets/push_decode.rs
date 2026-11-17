#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::Read;
use zrip::push::{PushDecoder, Status};

// Arbitrary bytes through PushDecoder in fuzzer-chosen chunks. Must not
// panic, and must agree with FrameDecoder whenever both succeed.
fuzz_target!(|data: &[u8]| {
    if data.len() < 2 || data.len() > 64 * 1024 {
        return;
    }
    let in_chunk = usize::from(data[0]) + 1;
    let out_buf = usize::from(data[1]) * 7 + 1;
    let stream = &data[2..];

    let mut dec = PushDecoder::with_limit(1 << 24);
    let mut out = Vec::new();
    let mut buf = vec![0u8; out_buf];
    let mut push_ok = true;
    'feed: for chunk in stream.chunks(in_chunk) {
        let mut input = chunk;
        loop {
            match dec.decode(input, &mut buf) {
                Ok(p) => {
                    out.extend_from_slice(&buf[..p.produced]);
                    input = &input[p.consumed..];
                    if p.status == Status::NeedsInput {
                        break;
                    }
                }
                Err(_) => {
                    push_ok = false;
                    break 'feed;
                }
            }
        }
    }
    push_ok = push_ok && dec.finish().is_ok();

    let mut reference = Vec::new();
    let frame_ok = zrip::FrameDecoder::with_limit(stream, 1 << 24)
        .read_to_end(&mut reference)
        .is_ok();
    if push_ok && frame_ok {
        assert_eq!(out, reference);
    }
});
