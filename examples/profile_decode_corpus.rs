use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const DEFAULT_FILES: &[&str] = &[
    "bench/corpus/silesia/dickens",
    "bench/corpus/silesia/mozilla",
    "bench/corpus/silesia/mr",
    "bench/corpus/silesia/nci",
    "bench/corpus/silesia/ooffice",
    "bench/corpus/silesia/osdb",
    "bench/corpus/silesia/reymont",
    "bench/corpus/silesia/samba",
    "bench/corpus/silesia/sao",
    "bench/corpus/silesia/webster",
    "bench/corpus/silesia/x-ray",
    "bench/corpus/silesia/xml",
];

fn measure(path: &Path) {
    let data = std::fs::read(path).unwrap();
    let compressed = zrip::compress(&data, 1).unwrap();
    let mut ctx = zrip::DecompressContext::new();

    for _ in 0..3 {
        let out = black_box(ctx.decompress(black_box(&compressed)).unwrap());
        assert_eq!(&*out, &data[..]);
    }

    let target = Duration::from_millis(700);
    let mut iters = 1usize;
    loop {
        let start = Instant::now();
        for _ in 0..iters {
            let out = black_box(ctx.decompress(black_box(&compressed)).unwrap());
            black_box(out.len());
        }
        if start.elapsed() >= target {
            break;
        }
        iters *= 2;
    }

    let mut best = f64::MAX;
    for _ in 0..5 {
        let start = Instant::now();
        for _ in 0..iters {
            let out = black_box(ctx.decompress(black_box(&compressed)).unwrap());
            black_box(out.len());
        }
        best = best.min(start.elapsed().as_secs_f64());
    }

    let mbs = (data.len() as f64 * iters as f64) / best / 1e6;
    let name = path.strip_prefix("bench/corpus").unwrap_or(path).display();
    println!("{name}\t{mbs:.0}");
}

fn main() {
    let paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if paths.is_empty() {
        for path in DEFAULT_FILES {
            measure(Path::new(path));
        }
    } else {
        for path in paths {
            measure(&path);
        }
    }
}
