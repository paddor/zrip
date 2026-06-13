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
            let decompressed = zrip::decompress_with_dict(&compressed, payload.len() + 1024, &dict)
                .expect("failed to decompress dict-compressed output");
            assert_eq!(payload, &decompressed[..]);
        }
    }
});
