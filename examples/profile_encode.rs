use std::hint::black_box;
fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or("corpus/silesia/dickens".into());
    let level = args.next().map_or(1, |s| s.parse().unwrap());
    let iters = args.next().map_or(50, |s| s.parse().unwrap());
    let data = std::fs::read(&path).unwrap();
    let mut ctx = zrip::CompressContext::new(level).unwrap();
    // warmup
    for _ in 0..3 {
        let _ = black_box(ctx.compress(black_box(&data)).unwrap());
    }
    // timed
    let start = std::time::Instant::now();
    for _ in 0..iters {
        let _ = black_box(ctx.compress(black_box(&data)).unwrap());
    }
    let elapsed = start.elapsed();
    let mbs = (data.len() as f64 * f64::from(iters)) / elapsed.as_secs_f64() / 1e6;
    eprintln!("{mbs:.0} MB/s encode L{level} ({path})");
}
