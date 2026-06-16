#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 20 || data.len() > 30_000 {
        return;
    }

    // Split input in half, compress each with C zstd, splice the compressed frames
    let split = data.len() / 2;
    let (a, b) = (&data[..split], &data[split..]);

    let Ok(comp_a) = zstd::bulk::compress(a, 1) else {
        return;
    };
    let Ok(comp_b) = zstd::bulk::compress(b, 1) else {
        return;
    };

    if comp_a.len() < 8 || comp_b.len() < 8 {
        return;
    }

    // Splice: first half of frame A + second half of frame B
    let mid_a = comp_a.len() / 2;
    let mid_b = comp_b.len() / 2;
    let mut spliced = comp_a[..mid_a].to_vec();
    spliced.extend_from_slice(&comp_b[mid_b..]);

    let _ = zrip::decompress(&spliced);
});
