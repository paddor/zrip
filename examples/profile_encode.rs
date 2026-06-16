use std::hint::black_box;
fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or("corpus/dickens.txt".into());
    let data = std::fs::read(&path).unwrap();
    let iters = 50;
    // warmup
    for _ in 0..3 {
        let _ = black_box(zrip::compress(black_box(&data), 1).unwrap());
    }
    // timed
    let start = std::time::Instant::now();
    for _ in 0..iters {
        let _ = black_box(zrip::compress(black_box(&data), 1).unwrap());
    }
    let elapsed = start.elapsed();
    let mbs = (data.len() as f64 * iters as f64) / elapsed.as_secs_f64() / 1e6;
    eprintln!("{:.0} MB/s encode L1 ({})", mbs, path);
}
