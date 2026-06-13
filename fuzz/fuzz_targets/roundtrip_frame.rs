#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    for level in [-5, -1, 1, 2, 3, 4] {
        if let Ok(compressed) = zrip::compress(data, level) {
            let decompressed = zrip::decompress(&compressed, data.len() + 1024)
                .expect("failed to decompress own output");
            assert_eq!(data, &decompressed[..]);
        }
    }
});
