use std::hint::black_box;
fn main() {
    let data = std::fs::read("corpus/silesia/mozilla").unwrap();
    let mut ctx = zstd::bulk::Compressor::new(1).unwrap();
    for _ in 0..20 {
        let _ = black_box(ctx.compress(black_box(&data)).unwrap());
    }
}
