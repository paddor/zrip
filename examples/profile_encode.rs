use std::hint::black_box;
fn main() {
    let data = std::fs::read("corpus/silesia/mozilla").unwrap();
    // warmup
    for _ in 0..3 {
        let _ = black_box(zrip::compress(black_box(&data), 1).unwrap());
    }
    // timed
    let start = std::time::Instant::now();
    for _ in 0..10 {
        let _ = black_box(zrip::compress(black_box(&data), 1).unwrap());
    }
    let elapsed = start.elapsed();
    let mbs = (data.len() as f64 * 10.0) / elapsed.as_secs_f64() / 1e6;
    eprintln!("{:.0} MB/s encode L1", mbs);
}
