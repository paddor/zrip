#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::{Read, Write};

fuzz_target!(|data: &[u8]| {
    if data.len() < 200 || data.len() > 64 * 1024 {
        return;
    }

    let mid = data.len() / 2;
    let samples_data = &data[..mid];
    let payload = &data[mid..];

    let chunk_size = 64.max(samples_data.len() / 8);
    let samples: Vec<&[u8]> = samples_data.chunks(chunk_size).collect();
    if samples.len() < 2 {
        return;
    }

    let dict = zrip::dict::train_dict_fastcover(
        &samples,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );

    for level in [1, 3] {
        // Streaming encode with dict, varied chunk sizes
        let mut enc = match zrip::FrameEncoder::with_dict(Vec::new(), level, dict.clone()) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for chunk in payload.chunks(chunk_size.max(1)) {
            enc.write_all(chunk).expect("streaming dict encode failed");
        }
        let compressed = enc.finish().expect("streaming dict finish failed");

        // Streaming decode with dict
        let mut dec = zrip::FrameDecoder::with_dict(&compressed[..], dict.clone());
        let mut decompressed = Vec::new();
        dec.read_to_end(&mut decompressed)
            .expect("streaming dict decode failed");
        assert_eq!(payload, &decompressed[..]);

        // One-shot decode with dict must also agree
        let oneshot = zrip::decompress_with_dict(&compressed, &dict)
            .expect("oneshot dict decode of streaming output failed");
        assert_eq!(payload, &oneshot[..]);
    }
});
