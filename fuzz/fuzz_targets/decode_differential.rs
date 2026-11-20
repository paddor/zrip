#![no_main]
//! zrip's decoders must accept exactly the inputs C zstd's accept, with the
//! same output (see `tests/differential/check.rs`). The first byte picks
//! whether a C-trained dictionary is loaded; the rest is the input.
use libfuzzer_sys::fuzz_target;
use std::sync::LazyLock;

#[path = "../../tests/differential/check.rs"]
mod check;

static DICT: LazyLock<Vec<u8>> = LazyLock::new(|| {
    const WORDS: [&str; 8] = [
        "zstd ", "frame ", "block ", "literal ", "match ", "offset ", "window ", "table ",
    ];
    let samples: Vec<Vec<u8>> = (0..300u32)
        .map(|i| {
            let mut state = i.wrapping_mul(0x9E37_79B1) | 1;
            let mut sample = Vec::new();
            while sample.len() < 400 {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                sample.extend_from_slice(WORDS[(state >> 29) as usize].as_bytes());
            }
            sample
        })
        .collect();
    zstd::dict::from_samples(&samples, 8192).expect("dictionary training failed")
});

fuzz_target!(|data: &[u8]| {
    let Some((&mode, input)) = data.split_first() else {
        return;
    };
    let dict = (mode & 1 == 1).then(|| DICT.as_slice());
    if let Err(e) = check::check(input, dict) {
        panic!("{e}");
    }
});
