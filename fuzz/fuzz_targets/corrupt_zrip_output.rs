#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 10 || data.len() > 50_000 {
        return;
    }

    // First 2 bytes: corruption params, rest is payload
    let seed = u64::from_le_bytes({
        let mut buf = [0u8; 8];
        buf[..2].copy_from_slice(&data[..2]);
        buf
    });
    let payload = &data[2..];

    // Compress with zrip at multiple levels
    for level in [-7, -1, 1, 3] {
        let Ok(compressed) = zrip::compress(payload, level) else {
            continue;
        };
        if compressed.is_empty() {
            continue;
        }

        // Flip 1-8 bits
        let num_flips = (seed % 8) as usize + 1;
        let mut corrupted = compressed.clone();
        for i in 0..num_flips {
            let bit_seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(i as u64);
            let byte_pos = (bit_seed as usize) % corrupted.len();
            let bit_pos = ((bit_seed >> 32) % 8) as u8;
            corrupted[byte_pos] ^= 1 << bit_pos;
        }
        let _ = zrip::decompress(&corrupted);

        // Also try truncation
        let trunc = (seed as usize) % compressed.len();
        let _ = zrip::decompress(&compressed[..trunc]);

        // Overwrite a random interior region
        if compressed.len() > 8 {
            let pos = 4 + (seed as usize % (compressed.len() - 8));
            let mut overwritten = compressed.clone();
            let end = (pos + 4).min(overwritten.len());
            for j in pos..end {
                overwritten[j] = payload[j % payload.len()];
            }
            let _ = zrip::decompress(&overwritten);
        }
    }
});
