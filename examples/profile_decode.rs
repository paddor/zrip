use std::hint::black_box;
fn main() {
    let data = std::fs::read("corpus/silesia/mozilla").unwrap();
    let compressed = zrip::compress(&data, 1).unwrap();
    let mut ctx = zrip::DecompressContext::new();
    // warmup
    for _ in 0..3 {
        let _ = black_box(ctx.decompress(black_box(&compressed)).unwrap());
    }
    // timed
    let start = std::time::Instant::now();
    for _ in 0..20 {
        let _ = black_box(ctx.decompress(black_box(&compressed)).unwrap());
    }
    let elapsed = start.elapsed();
    let mbs = (data.len() as f64 * 20.0) / elapsed.as_secs_f64() / 1e6;
    eprintln!("{:.0} MB/s decode (DecompressContext)", mbs);
}
