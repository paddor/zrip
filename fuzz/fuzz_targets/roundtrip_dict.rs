#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 200 || data.len() > 64 * 1024 {
        return;
    }

    // Split input: first half as "training" samples, second half as payload
    let mid = data.len() / 2;
    let samples_data = &data[..mid];
    let payload = &data[mid..];

    // Create fake samples from the training portion
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
        if let Ok(compressed) = zrip::compress_with_dict(payload, level, &dict) {
            let decompressed = zrip::decompress_with_dict(&compressed, &dict)
                .expect("failed to decompress dict-compressed output");
            assert_eq!(payload, &decompressed[..]);
        }

        // A reused context builds its dictionary tables on first use, after
        // earlier frames left their input in its buffer. Its frames must
        // match a new context's.
        let mut ctx = zrip::CompressContext::with_dict(level, dict.clone()).unwrap();
        let half = payload.len() / 2;
        for (i, part) in [&payload[..half], payload, &payload[half..]]
            .iter()
            .enumerate()
        {
            let fresh = zrip::CompressContext::with_dict(level, dict.clone())
                .unwrap()
                .compress(part)
                .unwrap()
                .into_owned();
            let frame = ctx.compress(part).unwrap();
            assert_eq!(&frame[..], &fresh[..], "L{level} frame {i}");
            let decompressed = zrip::decompress_with_dict(&frame, &dict)
                .expect("failed to decompress dict context output");
            assert_eq!(*part, &decompressed[..]);
        }
    }
});
