#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 200 || data.len() > 32 * 1024 {
        return;
    }

    // Split: first third trains dict, second third is payload, last third is corruption seed
    let third = data.len() / 3;
    let dict_data = &data[..third];
    let payload = &data[third..2 * third];
    let corruption = &data[2 * third..];

    // Train a dictionary
    let chunk_size = 64.max(dict_data.len() / 8);
    let samples: Vec<&[u8]> = dict_data.chunks(chunk_size).collect();
    if samples.len() < 2 {
        return;
    }
    let dict = zrip::dict::train_dict_fastcover(
        &samples,
        4096,
        zrip::dict::fastcover::FastCoverParams::default(),
    );

    for level in [1, 3] {
        let Ok(compressed) = zrip::compress_with_dict(payload, level, &dict) else {
            continue;
        };

        // Corrupt the compressed frame, try to decompress with valid dict
        if !corruption.is_empty() && compressed.len() > 4 {
            let mut corrupted = compressed.clone();
            for (i, &b) in corruption.iter().enumerate().take(8) {
                let pos = 4 + (b as usize % (corrupted.len() - 4));
                corrupted[pos] ^= corruption.get(i + 8).copied().unwrap_or(0xFF);
            }
            let _ = zrip::decompress_with_dict(&corrupted, &dict);
        }

        // Feed corrupt dict bytes to Dictionary::from_bytes, then decompress valid frame
        if corruption.len() >= 8 {
            // Build something that looks like a dict: magic + corrupt body
            let mut fake_dict = vec![0x37, 0xa4, 0x30, 0xec]; // ZSTD dict magic
            fake_dict.extend_from_slice(corruption);
            if let Ok(bad_dict) = zrip::Dictionary::from_bytes(&fake_dict) {
                let _ = zrip::decompress_with_dict(&compressed, &bad_dict);
            }
        }
    }
});
