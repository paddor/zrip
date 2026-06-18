use std::hint::black_box;
fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or("corpus/dickens.txt".into());
    let data = std::fs::read(&path).unwrap();
    let compressed = zrip::compress(&data, 1).unwrap();
    let mut ctx = zrip::DecompressContext::new();
    let iters = 100;
    // warmup
    for _ in 0..3 {
        let _ = black_box(ctx.decompress(black_box(&compressed)).unwrap());
    }
    // timed
    let start = std::time::Instant::now();
    for _ in 0..iters {
        let _ = black_box(ctx.decompress(black_box(&compressed)).unwrap());
    }
    let elapsed = start.elapsed();
    let mbs = (data.len() as f64 * f64::from(iters)) / elapsed.as_secs_f64() / 1e6;
    eprintln!("{mbs:.0} MB/s decode L1 ({path})");
}
