use std::hint::black_box;
fn main() {
    let data = std::fs::read("corpus/silesia/x-ray").unwrap();
    for _ in 0..100 {
        let _ = black_box(zrip::compress(black_box(&data), 1).unwrap());
    }
}
