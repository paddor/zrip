extern crate libc;

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

const LEVELS: &[i32] = &[1, 3];

const SILESIA_DOWNLOADS: &[(&str, &str)] = &[
    (
        "corpus/dickens.txt",
        "https://sun.aei.polsl.pl/~sdeor/corpus/dickens.bz2",
    ),
    (
        "corpus/silesia/mr",
        "https://sun.aei.polsl.pl/~sdeor/corpus/mr.bz2",
    ),
    (
        "corpus/silesia/mozilla",
        "https://sun.aei.polsl.pl/~sdeor/corpus/mozilla.bz2",
    ),
    (
        "corpus/silesia/nci",
        "https://sun.aei.polsl.pl/~sdeor/corpus/nci.bz2",
    ),
    (
        "corpus/silesia/osdb",
        "https://sun.aei.polsl.pl/~sdeor/corpus/osdb.bz2",
    ),
    (
        "corpus/silesia/samba",
        "https://sun.aei.polsl.pl/~sdeor/corpus/samba.bz2",
    ),
    (
        "corpus/silesia/sao",
        "https://sun.aei.polsl.pl/~sdeor/corpus/sao.bz2",
    ),
    (
        "corpus/silesia/webster",
        "https://sun.aei.polsl.pl/~sdeor/corpus/webster.bz2",
    ),
    (
        "corpus/silesia/x-ray",
        "https://sun.aei.polsl.pl/~sdeor/corpus/x-ray.bz2",
    ),
];

const ALL_FILES: &[&str] = &[
    "corpus/compression_1k.txt",
    "corpus/compression_34k.txt",
    "corpus/compression_65k.txt",
    "corpus/compression_66k_JSON.txt",
    "corpus/dickens.txt",
    "corpus/hdfs.json",
    "corpus/reymont.pdf",
    "corpus/xml_collection.xml",
    "corpus/silesia/mr",
    "corpus/silesia/mozilla",
    "corpus/silesia/nci",
    "corpus/silesia/osdb",
    "corpus/silesia/samba",
    "corpus/silesia/sao",
    "corpus/silesia/webster",
    "corpus/silesia/x-ray",
];

fn cpu_nanos() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

fn bench_loop<F: FnMut()>(warmup: usize, target_ns: u64, rounds: usize, mut f: F) -> f64 {
    for _ in 0..warmup {
        f();
    }
    let mut best = f64::MAX;
    for _ in 0..rounds {
        let mut iters = 0u64;
        let start = cpu_nanos();
        loop {
            std::hint::black_box(&mut f)();
            iters += 1;
            if cpu_nanos() - start >= target_ns {
                break;
            }
        }
        let elapsed = cpu_nanos() - start;
        let ns_per_op = elapsed as f64 / iters as f64;
        if ns_per_op < best {
            best = ns_per_op;
        }
    }
    best
}

#[derive(Clone)]
struct BenchResult {
    codec: String,
    input_name: String,
    level: i32,
    input_size: usize,
    compressed_size: usize,
    compress_ns: f64,
    decompress_ns: f64,
}

impl BenchResult {
    fn to_json(&self) -> String {
        format!(
            concat!(
                r#"{{"codec": "{}", "input": "{}", "level": {}, "#,
                r#""input_size": {}, "compressed_size": {}, "#,
                r#""compress_ns": {:.1}, "decompress_ns": {:.1}}}"#,
            ),
            self.codec,
            self.input_name,
            self.level,
            self.input_size,
            self.compressed_size,
            self.compress_ns,
            self.decompress_ns,
        )
    }
}

fn bench_zrip(data: &[u8], name: &str, level: i32, target_ns: u64) -> BenchResult {
    let mut ctx = zrip::CompressContext::new(level).unwrap();
    let compressed = ctx.compress(data).unwrap().to_vec();
    let compress_ns = bench_loop(3, target_ns, 7, || {
        let _ = std::hint::black_box(ctx.compress(std::hint::black_box(data)).unwrap());
    });
    let mut dec_ctx = zrip::DecompressContext::new();
    let decompress_ns = bench_loop(3, target_ns, 7, || {
        let _ = std::hint::black_box(
            dec_ctx
                .decompress(std::hint::black_box(&compressed))
                .unwrap(),
        );
    });
    BenchResult {
        codec: "zrip".into(),
        input_name: name.into(),
        level,
        input_size: data.len(),
        compressed_size: compressed.len(),
        compress_ns,
        decompress_ns,
    }
}

fn bench_lz4rip(data: &[u8], name: &str, level: i32, target_ns: u64) -> BenchResult {
    let compressed = lz4rip::block::compress(data);
    let compress_ns = bench_loop(3, target_ns, 7, || {
        let _ = std::hint::black_box(lz4rip::block::compress(std::hint::black_box(data)));
    });
    let decompress_ns = bench_loop(3, target_ns, 7, || {
        let _ = std::hint::black_box(
            lz4rip::block::decompress(std::hint::black_box(&compressed), data.len()).unwrap(),
        );
    });
    BenchResult {
        codec: "lz4rip".into(),
        input_name: name.into(),
        level,
        input_size: data.len(),
        compressed_size: compressed.len(),
        compress_ns,
        decompress_ns,
    }
}

fn bench_ruzstd(data: &[u8], name: &str, level: i32, target_ns: u64) -> BenchResult {
    use ruzstd::encoding::CompressionLevel;
    let ruz_level = match level {
        1 => CompressionLevel::Fastest,
        _ => CompressionLevel::Uncompressed,
    };
    let compressed = ruzstd::encoding::compress_to_vec(data, ruz_level);
    let compress_ns = bench_loop(3, target_ns, 7, || {
        let _ = std::hint::black_box(ruzstd::encoding::compress_to_vec(
            std::hint::black_box(data),
            ruz_level,
        ));
    });
    let decompress_ns = bench_loop(3, target_ns, 7, || {
        let mut dec = ruzstd::decoding::FrameDecoder::new();
        let mut out = Vec::with_capacity(data.len() + 1024);
        dec.decode_all_to_vec(std::hint::black_box(&compressed), &mut out)
            .unwrap();
        std::hint::black_box(&out);
    });
    BenchResult {
        codec: "ruzstd".into(),
        input_name: name.into(),
        level,
        input_size: data.len(),
        compressed_size: compressed.len(),
        compress_ns,
        decompress_ns,
    }
}

fn bench_structured_zstd(data: &[u8], name: &str, level: i32, target_ns: u64) -> BenchResult {
    use structured_zstd::encoding::CompressionLevel;
    let sz_level = CompressionLevel::Level(level);
    let compressed = structured_zstd::encoding::compress_slice_to_vec(data, sz_level);
    let compress_ns = bench_loop(3, target_ns, 7, || {
        let _ = std::hint::black_box(structured_zstd::encoding::compress_slice_to_vec(
            std::hint::black_box(data),
            sz_level,
        ));
    });
    let decompress_ns = bench_loop(3, target_ns, 7, || {
        let mut dec = structured_zstd::decoding::FrameDecoder::new();
        let mut out = vec![0u8; data.len() + 1024];
        dec.decode_all(std::hint::black_box(&compressed), &mut out)
            .unwrap();
        std::hint::black_box(&out);
    });
    BenchResult {
        codec: "structured-zstd".into(),
        input_name: name.into(),
        level,
        input_size: data.len(),
        compressed_size: compressed.len(),
        compress_ns,
        decompress_ns,
    }
}

fn bench_c_zstd(data: &[u8], name: &str, level: i32, target_ns: u64) -> BenchResult {
    let mut compressor = zstd::bulk::Compressor::new(level).unwrap();
    let compressed = compressor.compress(data).unwrap();
    let mut decompressor = zstd::bulk::Decompressor::new().unwrap();
    let mut decomp_buf = Vec::with_capacity(data.len() + 1024);
    let compress_ns = bench_loop(3, target_ns, 7, || {
        let _ = std::hint::black_box(compressor.compress(std::hint::black_box(data)).unwrap());
    });
    let decompress_ns = bench_loop(3, target_ns, 7, || {
        decomp_buf.clear();
        let _ = std::hint::black_box(
            decompressor
                .decompress_to_buffer(std::hint::black_box(&compressed), &mut decomp_buf)
                .unwrap(),
        );
    });
    BenchResult {
        codec: "C zstd".into(),
        input_name: name.into(),
        level,
        input_size: data.len(),
        compressed_size: compressed.len(),
        compress_ns,
        decompress_ns,
    }
}

fn ensure_corpus() {
    for &(path, url) in SILESIA_DOWNLOADS {
        if std::fs::metadata(path).is_ok() {
            continue;
        }
        eprintln!("downloading {url} ...");
        let dir = PathBuf::from(path).parent().unwrap().to_owned();
        std::fs::create_dir_all(&dir).ok();
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("curl -fSL '{url}' | bzip2 -d > '{path}'"))
            .status();
        match status {
            Ok(s) if s.success() => {
                let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                eprintln!("  saved {path} ({size} bytes)");
            }
            _ => {
                eprintln!("  failed to download {path}, skipping");
                std::fs::remove_file(path).ok();
            }
        }
    }
}

fn cache_dir() -> PathBuf {
    let dir = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
        .join(".cache")
        .join("zrip");
    std::fs::create_dir_all(&dir).ok();
    dir
}

fn codec_cache_path(codec: &str) -> PathBuf {
    cache_dir().join(format!("{}.jsonl", codec.replace(' ', "_")))
}

fn load_cache(codec: &str) -> Vec<BenchResult> {
    let path = codec_cache_path(codec);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    content.lines().filter_map(parse_json_line).collect()
}

fn save_cache(results: &[BenchResult], codec: &str) {
    let entries: Vec<_> = results.iter().filter(|r| r.codec == codec).collect();
    if entries.is_empty() {
        return;
    }
    let path = codec_cache_path(codec);
    let mut f = std::fs::File::create(&path).unwrap();
    for r in &entries {
        writeln!(f, "{}", r.to_json()).unwrap();
    }
    eprintln!("cached {} results to {}", entries.len(), path.display());
}

fn parse_json_line(line: &str) -> Option<BenchResult> {
    let line = line.trim().trim_matches(',');
    if line == "[" || line == "]" || line.is_empty() {
        return None;
    }
    let get = |key: &str| -> Option<String> {
        let prefix = format!("\"{key}\": ");
        let start = line.find(&prefix)? + prefix.len();
        let rest = &line[start..];
        if let Some(stripped) = rest.strip_prefix('"') {
            let end = stripped.find('"')?;
            Some(stripped[..end].to_string())
        } else {
            let end = rest.find([',', '}']).unwrap_or(rest.len());
            Some(rest[..end].to_string())
        }
    };
    Some(BenchResult {
        codec: get("codec")?,
        input_name: get("input")?,
        level: get("level")?.parse().ok()?,
        input_size: get("input_size")?.parse().ok()?,
        compressed_size: get("compressed_size")?.parse().ok()?,
        compress_ns: get("compress_ns")?.parse().ok()?,
        decompress_ns: get("decompress_ns")?.parse().ok()?,
    })
}

const CODECS: &[&str] = &["C zstd", "zrip", "ruzstd", "structured-zstd", "lz4rip"];

fn main() {
    ensure_corpus();

    let args: Vec<String> = std::env::args().collect();
    let mut only: Vec<String> = Vec::new();
    let mut impl_specified = false;
    let mut file_filter: Vec<String> = Vec::new();
    let mut level_filter: Vec<i32> = Vec::new();
    let mut extra_files: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--impl" => {
                i += 1;
                impl_specified = true;
                if i < args.len() {
                    only.push(args[i].clone());
                }
            }
            "--files" => {
                i += 1;
                if i < args.len() {
                    file_filter.extend(args[i].split(',').map(|s| s.to_string()));
                }
            }
            "--levels" => {
                i += 1;
                if i < args.len() {
                    level_filter.extend(
                        args[i]
                            .split(',')
                            .filter_map(|s| s.trim().parse::<i32>().ok()),
                    );
                }
            }
            "--extra" => {
                i += 1;
                if i < args.len() {
                    extra_files.push(args[i].clone());
                }
            }
            _ => {}
        }
        i += 1;
    }

    if !impl_specified {
        only.push("zrip".into());
    } else if only.iter().any(|o| o == "all") {
        only.clear();
    }

    let levels: &[i32] = if level_filter.is_empty() {
        LEVELS
    } else {
        &level_filter
    };

    let target_ns = 20_000_000u64;

    let cached: Vec<Vec<BenchResult>> = CODECS.iter().map(|c| load_cache(c)).collect();
    let mut results: Vec<BenchResult> = Vec::new();

    let all_paths: Vec<&str> = ALL_FILES
        .iter()
        .copied()
        .chain(extra_files.iter().map(|s| s.as_str()))
        .collect();

    for path in &all_paths {
        let name = path.rsplit('/').next().unwrap();
        if !file_filter.is_empty() && !file_filter.iter().any(|f| f == name) {
            continue;
        }

        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(_) => {
                eprintln!("skipping {path}: not found");
                continue;
            }
        };

        for &level in levels {
            for (ci, &codec) in CODECS.iter().enumerate() {
                let should_run = only.is_empty() || only.iter().any(|o| codec.contains(o.as_str()));

                if !should_run {
                    if let Some(c) = cached[ci]
                        .iter()
                        .find(|c| c.input_name == name && c.level == level)
                    {
                        eprintln!("  {codec} x {name} @{level}: cached");
                        results.push(c.clone());
                    }
                    continue;
                }

                eprintln!("  {codec} x {name} @{level}: benchmarking...");
                let r = match codec {
                    "C zstd" => bench_c_zstd(&data, name, level, target_ns),
                    "zrip" => bench_zrip(&data, name, level, target_ns),
                    "ruzstd" => bench_ruzstd(&data, name, level, target_ns),
                    "structured-zstd" => bench_structured_zstd(&data, name, level, target_ns),
                    "lz4rip" => bench_lz4rip(&data, name, level, target_ns),
                    _ => unreachable!(),
                };
                results.push(r);
            }
        }
    }

    for codec in CODECS {
        save_cache(&results, codec);
    }

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "[").unwrap();
    for (i, r) in results.iter().enumerate() {
        let comma = if i + 1 < results.len() { "," } else { "" };
        writeln!(out, "  {}{}", r.to_json(), comma).unwrap();
    }
    writeln!(out, "]").unwrap();
}
