#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 128 * 1024 {
        return;
    }
    for level in [1, 3] {
        if let Ok(compressed) = zrip::compress(data, level) {
            let decompressed = zrip::decompress(&compressed, data.len() + 1024)
                .expect("failed to decompress own output");
            assert_eq!(data, &decompressed[..]);
        }
    }
});
