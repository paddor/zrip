#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::Read;

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 {
        return;
    }

    // Feed arbitrary bytes to FrameDecoder. Must not panic.
    let mut dec = zrip::FrameDecoder::new(data);
    let mut buf = vec![0u8; 256];
    loop {
        match dec.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
});
