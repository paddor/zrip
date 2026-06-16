#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 10 || data.len() > 50_000 {
        return;
    }

    // First 2 bytes: overwrite position fraction + length
    let pos_frac = data[0];
    let overwrite_len = (data[1] as usize % 16) + 1;
    let payload = &data[2..];

    if payload.is_empty() {
        return;
    }

    let Ok(compressed) = zstd::bulk::compress(payload, 1) else {
        return;
    };

    if compressed.len() < 4 {
        return;
    }

    // Skip the magic (first 4 bytes), overwrite interior bytes
    let interior_start = 4;
    let interior_len = compressed.len() - interior_start;
    if interior_len == 0 {
        return;
    }
    let pos = interior_start + (pos_frac as usize % interior_len);
    let end = (pos + overwrite_len).min(compressed.len());

    let mut corrupted = compressed;
    for i in pos..end {
        corrupted[i] = data[i % data.len()];
    }

    let _ = zrip::decompress(&corrupted);
});
