#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::Read;

fuzz_target!(|data: &[u8]| {
    if data.len() < 200 || data.len() > 64 * 1024 {
        return;
    }

    let third = data.len() / 3;
    let dict_data = &data[..third];
    let payload = &data[third..2 * third];
    let corruption = &data[2 * third..];

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

    // Compress with streaming encoder + dict
    let Ok(mut enc) =
        zrip::FrameEncoder::with_dict(Vec::new(), 1, dict.clone())
    else {
        return;
    };
    std::io::Write::write_all(&mut enc, payload).unwrap();
    let compressed = enc.finish().unwrap();

    // Corrupt the compressed frame, try streaming decode with valid dict. Must not panic.
    if !corruption.is_empty() && compressed.len() > 4 {
        let mut corrupted = compressed.clone();
        for (i, &b) in corruption.iter().enumerate().take(8) {
            let pos = 4 + (b as usize % (corrupted.len() - 4));
            corrupted[pos] ^= corruption.get(i + 8).copied().unwrap_or(0xFF);
        }
        let mut dec = zrip::FrameDecoder::with_dict(&corrupted[..], dict.clone());
        let mut buf = vec![0u8; 256];
        loop {
            match dec.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }

    // Feed arbitrary bytes to FrameDecoder::with_dict. Must not panic.
    let mut dec = zrip::FrameDecoder::with_dict(corruption, dict);
    let mut buf = vec![0u8; 256];
    loop {
        match dec.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
});
