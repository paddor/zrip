#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 || data.len() > 50_000 {
        return;
    }

    // Last byte determines truncation fraction, rest is payload
    let trunc_byte = data[data.len() - 1];
    let payload = &data[..data.len() - 1];

    let Ok(compressed) = zstd::bulk::compress(payload, 1) else {
        return;
    };

    if compressed.is_empty() {
        return;
    }

    let trunc_at = (trunc_byte as usize * compressed.len()) / 256;
    let _ = zrip::decompress(&compressed[..trunc_at], 4 * 1024 * 1024);
});
