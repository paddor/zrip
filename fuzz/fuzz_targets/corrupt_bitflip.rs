#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 10 || data.len() > 50_000 {
        return;
    }

    // Use first 8 bytes as seed for bit-flip positions, rest as payload
    let seed = u64::from_le_bytes(data[..8].try_into().unwrap());
    let payload = &data[8..];

    // Compress with C zstd, then apply targeted bit flips
    let Ok(compressed) = zstd::bulk::compress(payload, 1) else {
        return;
    };

    if compressed.is_empty() {
        return;
    }

    // Flip 1-4 bits determined by seed
    let num_flips = (seed % 4) as usize + 1;
    let mut corrupted = compressed.clone();
    for i in 0..num_flips {
        let bit_seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(i as u64);
        let byte_pos = (bit_seed as usize) % corrupted.len();
        let bit_pos = ((bit_seed >> 32) % 8) as u8;
        corrupted[byte_pos] ^= 1 << bit_pos;
    }

    let _ = zrip::decompress(&corrupted, 4 * 1024 * 1024);
});
