use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn expected_len(path: &Path) -> Option<usize> {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.split(".out").nth(1))
        .and_then(|tail| tail.split('.').next())
        .and_then(|len| len.parse().ok())
}

fn collect_targets(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let entries = std::fs::read_dir(root).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", root.display());
    });
    for entry in entries {
        let path = entry.unwrap().path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("zstd-magicless") {
            paths.push(path);
        }
    }
    paths.sort();
    paths
}

fn measure(paths: &[PathBuf], label: &str) {
    let inputs: Vec<(PathBuf, Vec<u8>, usize)> = paths
        .iter()
        .map(|path| {
            let input = std::fs::read(path).unwrap();
            let expected = expected_len(path).unwrap_or_else(|| {
                panic!("missing .out length in {}", path.display());
            });
            (path.clone(), input, expected)
        })
        .collect();

    let total_output: usize = inputs.iter().map(|(_, _, expected)| *expected).sum();
    let mut ctx = zrip::DecompressContext::new();
    let mut output = Vec::new();

    for (_, input, expected) in &inputs {
        output.clear();
        let written = ctx
            .decompress_after_magic_into(black_box(input), &mut output, usize::MAX)
            .unwrap();
        assert_eq!(written, *expected);
        assert_eq!(output.len(), *expected);
    }

    let target = Duration::from_millis(800);
    let mut iters = 1usize;
    loop {
        let start = Instant::now();
        for _ in 0..iters {
            for (_, input, _) in &inputs {
                output.clear();
                let written = ctx
                    .decompress_after_magic_into(black_box(input), &mut output, usize::MAX)
                    .unwrap();
                black_box(written);
                black_box(output.as_ptr());
            }
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
            for (_, input, _) in &inputs {
                output.clear();
                let written = ctx
                    .decompress_after_magic_into(black_box(input), &mut output, usize::MAX)
                    .unwrap();
                black_box(written);
                black_box(output.as_ptr());
            }
        }
        best = best.min(start.elapsed().as_secs_f64());
    }

    let bytes = total_output as f64 * iters as f64;
    println!("{label}\t{}\t{:.1} MB/s", inputs.len(), bytes / best / 1e6);
}

fn main() {
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let paths: Vec<PathBuf> = if let Some(first) = args.next() {
        let mut paths = vec![first];
        paths.extend(args);
        paths
    } else {
        collect_targets(Path::new("tmp/ozlrip-zstd-targets"))
    };

    let tiny: Vec<PathBuf> = paths
        .iter()
        .filter(|path| std::fs::metadata(path).unwrap().len() <= 512)
        .cloned()
        .collect();
    if !tiny.is_empty() {
        measure(&tiny, "tiny");
    }
    measure(&paths, "all");
}
