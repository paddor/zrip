use std::env;
use std::io::Read;
use std::time::Instant;

fn main() {
    let args: Vec<String> = env::args().collect();

    let mut iters: u32 = 0;
    let mut label = String::from("stdin");
    let mut level: i32 = 1;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--iters" => {
                i += 1;
                iters = args[i].parse().expect("invalid --iters value");
            }
            "--label" => {
                i += 1;
                label = args[i].clone();
            }
            "--level" => {
                i += 1;
                level = args[i].parse().expect("invalid --level value");
            }
            _ => {
                eprintln!("unknown arg: {}", args[i]);
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let mut raw = Vec::new();
    std::io::stdin()
        .read_to_end(&mut raw)
        .expect("failed to read stdin");

    if raw.is_empty() {
        eprintln!("no data on stdin");
        std::process::exit(1);
    }

    let original_size = raw.len();

    // Encode benchmark
    let compressed = bench_encode("zrip", &label, &raw, level, iters);
    #[cfg(feature = "czstd")]
    let _czstd_compressed = bench_encode_czstd(&label, &raw, level, iters);

    // Decode benchmark (using zrip-compressed data for zrip, same for C zstd)
    bench_decode("zrip", &label, &compressed, original_size, iters);
    #[cfg(feature = "czstd")]
    bench_decode_czstd(&label, &compressed, original_size, iters);
}

fn bench_encode(
    impl_name: &str,
    label: &str,
    raw: &[u8],
    level: i32,
    iters: u32,
) -> Vec<u8> {
    let compressed = zrip::compress(raw, level).expect("zrip compress failed");
    let ratio = raw.len() as f64 / compressed.len() as f64;

    let iters = if iters == 0 {
        calibrate(|| {
            let _ = zrip::compress(raw, level).unwrap();
        })
    } else {
        iters
    };

    let start = Instant::now();
    for _ in 0..iters {
        let _ = zrip::compress(raw, level).unwrap();
    }
    let elapsed = start.elapsed();
    print_result(impl_name, "encode", label, raw.len(), iters, elapsed, ratio);
    compressed
}

#[cfg(feature = "czstd")]
fn bench_encode_czstd(label: &str, raw: &[u8], level: i32, iters: u32) -> Vec<u8> {
    let compressed = zstd::encode_all(raw, level).expect("C zstd compress failed");
    let ratio = raw.len() as f64 / compressed.len() as f64;

    let iters = if iters == 0 {
        calibrate(|| {
            let _ = zstd::encode_all(raw, level).unwrap();
        })
    } else {
        iters
    };

    let start = Instant::now();
    for _ in 0..iters {
        let _ = zstd::encode_all(raw, level).unwrap();
    }
    let elapsed = start.elapsed();
    print_result("C zstd", "encode", label, raw.len(), iters, elapsed, ratio);
    compressed
}

fn bench_decode(
    impl_name: &str,
    label: &str,
    compressed: &[u8],
    original_size: usize,
    iters: u32,
) {
    let iters = if iters == 0 {
        calibrate(|| {
            let _ = zrip::decompress(compressed).unwrap();
        })
    } else {
        iters
    };

    let start = Instant::now();
    for _ in 0..iters {
        let _ = zrip::decompress(compressed).expect("decompression failed");
    }
    let elapsed = start.elapsed();
    print_result(impl_name, "decode", label, original_size, iters, elapsed, 0.0);
}

#[cfg(feature = "czstd")]
fn bench_decode_czstd(label: &str, compressed: &[u8], original_size: usize, iters: u32) {
    let iters = if iters == 0 {
        calibrate(|| {
            let _ = zstd::decode_all(compressed).unwrap();
        })
    } else {
        iters
    };

    let start = Instant::now();
    for _ in 0..iters {
        let _ = zstd::decode_all(compressed).expect("C zstd decompression failed");
    }
    let elapsed = start.elapsed();
    print_result("C zstd", "decode", label, original_size, iters, elapsed, 0.0);
}

fn calibrate(mut f: impl FnMut()) -> u32 {
    let target_ns: u64 = 500_000_000;
    let start = Instant::now();
    f();
    let single_ns = start.elapsed().as_nanos() as u64;
    ((target_ns / single_ns.max(1)) as u32).clamp(4, 10000)
}

fn print_result(
    impl_name: &str,
    op: &str,
    label: &str,
    size: usize,
    iters: u32,
    elapsed: std::time::Duration,
    ratio: f64,
) {
    let total_bytes = size as u64 * iters as u64;
    let throughput_mbps = total_bytes as f64 / elapsed.as_secs_f64() / 1_000_000.0;
    let per_iter_us = elapsed.as_micros() as f64 / iters as f64;
    if ratio > 0.0 {
        println!(
            "{impl_name:>8} {op:>6} {label}: {throughput_mbps:.1} MB/s ({ratio:.2}x, {iters} iters, {per_iter_us:.0} \u{00b5}s/iter)",
        );
    } else {
        println!(
            "{impl_name:>8} {op:>6} {label}: {throughput_mbps:.1} MB/s ({iters} iters, {per_iter_us:.0} \u{00b5}s/iter)",
        );
    }
}
