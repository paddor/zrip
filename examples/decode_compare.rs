use std::time::Instant;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or("corpus/dickens.txt".into());
    let data = std::fs::read(&path).expect("file not found");
    let name = path.rsplit('/').next().unwrap();

    let c_compressed = zstd::encode_all(&data[..], 1).unwrap();
    let mut zrip_ctx = zrip::CompressContext::new(1).unwrap();
    let z_compressed = zrip_ctx.compress(&data).unwrap().to_vec();

    eprintln!(
        "{name}: input={}, C zstd={}, zrip={}",
        data.len(),
        c_compressed.len(),
        z_compressed.len()
    );

    let decoded = zrip::decompress(&c_compressed).unwrap();
    assert_eq!(decoded.len(), data.len());
    assert_eq!(&decoded[..], &data[..]);

    let cap = data.len() + 1024;
    let iters = 200;

    // zrip decoding C-zstd data
    for _ in 0..5 {
        let _ =
            std::hint::black_box(zrip::decompress(std::hint::black_box(&c_compressed)).unwrap());
    }
    let mut times: Vec<u128> = (0..iters)
        .map(|_| {
            let t = Instant::now();
            let _ = std::hint::black_box(
                zrip::decompress(std::hint::black_box(&c_compressed)).unwrap(),
            );
            t.elapsed().as_nanos()
        })
        .collect();
    times.sort();
    let zrip_c_ns = times[iters / 2];

    // zrip decoding zrip data
    for _ in 0..5 {
        let _ =
            std::hint::black_box(zrip::decompress(std::hint::black_box(&z_compressed)).unwrap());
    }
    let mut times: Vec<u128> = (0..iters)
        .map(|_| {
            let t = Instant::now();
            let _ = std::hint::black_box(
                zrip::decompress(std::hint::black_box(&z_compressed)).unwrap(),
            );
            t.elapsed().as_nanos()
        })
        .collect();
    times.sort();
    let zrip_z_ns = times[iters / 2];

    // C zstd decoding C-zstd data
    let mut dec = zstd::bulk::Decompressor::new().unwrap();
    let mut buf = Vec::with_capacity(cap);
    for _ in 0..5 {
        buf.clear();
        dec.decompress_to_buffer(&c_compressed, &mut buf).unwrap();
    }
    let mut times: Vec<u128> = (0..iters)
        .map(|_| {
            buf.clear();
            let t = Instant::now();
            let _ = std::hint::black_box(
                dec.decompress_to_buffer(std::hint::black_box(&c_compressed), &mut buf)
                    .unwrap(),
            );
            t.elapsed().as_nanos()
        })
        .collect();
    times.sort();
    let c_c_ns = times[iters / 2];

    let mb = data.len() as f64 / 1e6;
    eprintln!("--- Same input (C-zstd compressed) ---");
    eprintln!(
        "C zstd:  {:>7.1} MB/s  ({:.0} µs)",
        mb / (c_c_ns as f64 / 1e9),
        c_c_ns as f64 / 1e3
    );
    eprintln!(
        "zrip:    {:>7.1} MB/s  ({:.0} µs)  [{:.0}% of C]",
        mb / (zrip_c_ns as f64 / 1e9),
        zrip_c_ns as f64 / 1e3,
        100.0 * c_c_ns as f64 / zrip_c_ns as f64
    );
    eprintln!();
    eprintln!("--- zrip-compressed input ---");
    eprintln!(
        "zrip:    {:>7.1} MB/s  ({:.0} µs)",
        mb / (zrip_z_ns as f64 / 1e9),
        zrip_z_ns as f64 / 1e3
    );
}
