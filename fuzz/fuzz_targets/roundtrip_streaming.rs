#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::{Read, Write};

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 || data.len() > 128 * 1024 {
        return;
    }

    // First byte selects level, second byte selects chunk size
    let level_idx = data[0] as usize % 11;
    let levels = [-7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4];
    let level = levels[level_idx];
    let chunk_size = (data[1] as usize % 64) + 1;
    let payload = &data[2..];

    // Encode via streaming with varied chunk sizes
    let mut enc = match zrip::FrameEncoder::new(Vec::new(), level) {
        Ok(e) => e,
        Err(_) => return,
    };
    for chunk in payload.chunks(chunk_size) {
        enc.write_all(chunk).expect("streaming encode failed");
    }
    let compressed = enc.finish().expect("streaming finish failed");

    // Decode via streaming with 1-byte reads
    let mut dec = zrip::FrameDecoder::new(&compressed[..]);
    let mut decompressed = Vec::new();
    let mut buf = [0u8; 1];
    loop {
        match dec.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => decompressed.extend_from_slice(&buf[..n]),
            Err(e) => panic!("streaming decode failed: {e}"),
        }
    }
    assert_eq!(payload, &decompressed[..]);

    // Also verify one-shot decode matches
    let oneshot = zrip::decompress(&compressed).expect("oneshot decode failed");
    assert_eq!(payload, &oneshot[..]);
});
