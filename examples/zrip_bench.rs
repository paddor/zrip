extern crate libc;

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

const ZRIP_LEVELS: &[i32] = &[-7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4];
const C_ZSTD_LEVELS: &[i32] = &[-7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4, 5];

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

fn append_cache(results: &[BenchResult], codec: &str) {
    let entries: Vec<_> = results.iter().filter(|r| r.codec == codec).collect();
    if entries.is_empty() {
        return;
    }
    let path = codec_cache_path(codec);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap();
    for r in &entries {
        writeln!(f, "{}", r.to_json()).unwrap();
    }
    eprintln!("appended {} results to {}", entries.len(), path.display());
}

const CODECS: &[&str] = &["C zstd", "zrip", "ruzstd", "structured-zstd", "lz4rip"];

fn levels_for_codec<'a>(codec: &str, level_filter: &'a [i32]) -> &'a [i32] {
    match codec {
        "ruzstd" | "lz4rip" => &[1],
        _ if !level_filter.is_empty() => level_filter,
        "zrip" => ZRIP_LEVELS,
        "C zstd" | "structured-zstd" => C_ZSTD_LEVELS,
        _ => &[1],
    }
}

fn fmt_mbs(input_size: usize, ns: f64) -> String {
    let mbs = input_size as f64 / ns * 1000.0;
    if mbs >= 1000.0 {
        format!("{:.0}", mbs)
    } else if mbs >= 100.0 {
        format!("{:.0}", mbs)
    } else {
        format!("{:.1}", mbs)
    }
}

fn display_codec(codec: &str) -> &str {
    match codec {
        "structured-zstd" => "s-zstd",
        other => other,
    }
}

fn print_live_line(file: &str, level: i32, results: &[&BenchResult]) {
    use std::io::Write as _;
    let stderr = std::io::stderr();
    let mut err = stderr.lock();

    write!(err, "  L{level:>3} {file:<16}").unwrap();
    for r in results {
        let ratio = r.input_size as f64 / r.compressed_size as f64;
        let enc = fmt_mbs(r.input_size, r.compress_ns);
        let dec = fmt_mbs(r.input_size, r.decompress_ns);
        write!(
            err,
            "  {:>8} {:>5} enc {:>5} dec {:.2}x",
            display_codec(&r.codec),
            enc,
            dec,
            ratio
        )
        .unwrap();
    }
    writeln!(err).unwrap();
}

fn main() {
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

    ensure_corpus();

    if !impl_specified {
        only.push("zrip".into());
    } else if only.iter().any(|o| o == "all") {
        only.clear();
    }

    let active_codecs: Vec<&str> = CODECS
        .iter()
        .copied()
        .filter(|c| only.is_empty() || only.iter().any(|o| c.contains(o.as_str())))
        .collect();

    let all_levels: Vec<i32> = {
        let mut lvls: Vec<i32> = active_codecs
            .iter()
            .flat_map(|c| levels_for_codec(c, &level_filter).iter().copied())
            .collect();
        lvls.sort();
        lvls.dedup();
        lvls
    };

    let target_ns = 20_000_000u64;

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

        eprintln!("{name} ({} bytes)", data.len());

        for &level in &all_levels {
            let mut level_batch: Vec<BenchResult> = Vec::new();

            for &codec in &active_codecs {
                let codec_levels = levels_for_codec(codec, &level_filter);
                if !codec_levels.contains(&level) {
                    continue;
                }

                let r = match codec {
                    "C zstd" => bench_c_zstd(&data, name, level, target_ns),
                    "zrip" => bench_zrip(&data, name, level, target_ns),
                    "ruzstd" => bench_ruzstd(&data, name, level, target_ns),
                    "structured-zstd" => bench_structured_zstd(&data, name, level, target_ns),
                    "lz4rip" => bench_lz4rip(&data, name, level, target_ns),
                    _ => unreachable!(),
                };
                level_batch.push(r);
            }

            if !level_batch.is_empty() {
                let refs: Vec<&BenchResult> = level_batch.iter().collect();
                print_live_line(name, level, &refs);
                results.extend(level_batch);
            }
        }
    }

    for codec in CODECS {
        append_cache(&results, codec);
    }
}
