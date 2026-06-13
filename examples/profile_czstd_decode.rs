use std::hint::black_box;
fn main() {
    let data = std::fs::read("corpus/silesia/mozilla").unwrap();
    let compressed = zstd::bulk::compress(&data, 1).unwrap();
    let mut decompressor = zstd::bulk::Decompressor::new().unwrap();
    let mut buf = Vec::with_capacity(data.len() + 1024);
    // warmup
    for _ in 0..3 {
        buf.clear();
        let _ = decompressor
            .decompress_to_buffer(black_box(&compressed), &mut buf)
            .unwrap();
    }
    // timed
    let start = std::time::Instant::now();
    for _ in 0..20 {
        buf.clear();
        let _ = decompressor
            .decompress_to_buffer(black_box(&compressed), &mut buf)
            .unwrap();
    }
    let elapsed = start.elapsed();
    let mbs = (data.len() as f64 * 20.0) / elapsed.as_secs_f64() / 1e6;
    eprintln!("{:.0} MB/s decode", mbs);
}
