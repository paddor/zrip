#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 256 * 1024 {
        return;
    }
    for level in [1, 3] {
        let compressed = zstd::bulk::compress(data, level).unwrap();
        let decompressed = zrip::decompress(&compressed, data.len() + 1024)
            .expect("zrip failed to decompress C zstd output");
        assert_eq!(data, &decompressed[..]);
    }
});
