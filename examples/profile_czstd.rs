use std::hint::black_box;
fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or("corpus/silesia/dickens".into());
    let data = std::fs::read(&path).unwrap();
    let iters = 50;
    let mut ctx = zstd::bulk::Compressor::new(1).unwrap();
    for _ in 0..3 {
        let _ = black_box(ctx.compress(black_box(&data)).unwrap());
    }
    let start = std::time::Instant::now();
    for _ in 0..iters {
        let _ = black_box(ctx.compress(black_box(&data)).unwrap());
    }
    let elapsed = start.elapsed();
    let mbs = (data.len() as f64 * f64::from(iters)) / elapsed.as_secs_f64() / 1e6;
    eprintln!("{mbs:.0} MB/s C zstd encode L1 ({path})");
}
