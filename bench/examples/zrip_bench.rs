extern crate libc;

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

const ZRIP_LEVELS: &[i32] = &[-8, -7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4];
const C_ZSTD_LEVELS: &[i32] = &[-7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4];

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

const SMALL_FILES: &[&str] = &[
    "corpus/small/dickens_512",
    "corpus/small/dickens_1k",
    "corpus/small/dickens_2k",
    "corpus/small/dickens_4k",
    "corpus/small/dickens_8k",
    "corpus/small/dickens_16k",
    "corpus/small/dickens_32k",
    "corpus/small/dickens_64k",
    "corpus/small/dickens_128k",
    "corpus/small/hdfs_512",
    "corpus/small/hdfs_1k",
    "corpus/small/hdfs_2k",
    "corpus/small/hdfs_4k",
    "corpus/small/hdfs_8k",
    "corpus/small/hdfs_16k",
    "corpus/small/hdfs_32k",
    "corpus/small/hdfs_64k",
    "corpus/small/hdfs_128k",
    "corpus/small/xml_collection_512",
    "corpus/small/xml_collection_1k",
    "corpus/small/xml_collection_2k",
    "corpus/small/xml_collection_4k",
    "corpus/small/xml_collection_8k",
    "corpus/small/xml_collection_16k",
    "corpus/small/xml_collection_32k",
    "corpus/small/xml_collection_64k",
    "corpus/small/xml_collection_128k",
    "corpus/small/x-ray_512",
    "corpus/small/x-ray_1k",
    "corpus/small/x-ray_2k",
    "corpus/small/x-ray_4k",
    "corpus/small/x-ray_8k",
    "corpus/small/x-ray_16k",
    "corpus/small/x-ray_32k",
    "corpus/small/x-ray_64k",
    "corpus/small/x-ray_128k",
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
        codec: ZRIP_CODEC.into(),
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

fn train_dict_for_file(data: &[u8], dict_size: usize) -> Vec<u8> {
    let chunk_size = 1024usize;
    let mut concat = Vec::new();
    let mut sizes = Vec::new();
    for chunk in data.chunks(chunk_size) {
        concat.extend_from_slice(chunk);
        sizes.push(chunk.len());
    }
    let mut buf = vec![0u8; dict_size];
    let n = zstd_safe::train_from_buffer(&mut buf, &concat, &sizes).expect("dict training failed");
    buf.truncate(n);
    buf
}

fn bench_zrip_dict(
    data: &[u8],
    name: &str,
    level: i32,
    target_ns: u64,
    dict_bytes: &[u8],
) -> BenchResult {
    let dict = zrip::dict::Dictionary::from_bytes(dict_bytes).unwrap();
    let mut ctx =
        zrip::CompressContext::with_dict_for_size(level, dict.clone(), data.len()).unwrap();
    let compressed = ctx.compress(data).unwrap().to_vec();
    let compress_ns = bench_loop(3, target_ns, 7, || {
        let _ = std::hint::black_box(ctx.compress(std::hint::black_box(data)).unwrap());
    });
    let decompress_ns = bench_loop(3, target_ns, 7, || {
        let _ = std::hint::black_box(
            zrip::decompress_with_dict(std::hint::black_box(&compressed), &dict).unwrap(),
        );
    });
    BenchResult {
        codec: "zrip+dict".into(),
        input_name: name.into(),
        level,
        input_size: data.len(),
        compressed_size: compressed.len(),
        compress_ns,
        decompress_ns,
    }
}

fn bench_c_zstd_dict(
    data: &[u8],
    name: &str,
    level: i32,
    target_ns: u64,
    dict_bytes: &[u8],
) -> BenchResult {
    let mut compressor = zstd::bulk::Compressor::with_dictionary(level, dict_bytes).unwrap();
    let compressed = compressor.compress(data).unwrap();
    let mut decompressor = zstd::bulk::Decompressor::with_dictionary(dict_bytes).unwrap();
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
        codec: "C zstd+dict".into(),
        input_name: name.into(),
        level,
        input_size: data.len(),
        compressed_size: compressed.len(),
        compress_ns,
        decompress_ns,
    }
}

fn bench_decode_only(
    codec: &str,
    compressed: &[u8],
    original_size: usize,
    compressed_size: usize,
    name: &str,
    level: i32,
    target_ns: u64,
) -> BenchResult {
    let decompress_ns = match codec {
        "C zstd" => {
            let mut decompressor = zstd::bulk::Decompressor::new().unwrap();
            let mut buf = Vec::with_capacity(original_size + 1024);
            bench_loop(3, target_ns, 7, || {
                buf.clear();
                let _ = std::hint::black_box(
                    decompressor
                        .decompress_to_buffer(std::hint::black_box(compressed), &mut buf)
                        .unwrap(),
                );
            })
        }
        "zrip" | "zrip paranoid" => {
            let mut ctx = zrip::DecompressContext::new();
            bench_loop(3, target_ns, 7, || {
                let _ =
                    std::hint::black_box(ctx.decompress(std::hint::black_box(compressed)).unwrap());
            })
        }
        "ruzstd" => bench_loop(3, target_ns, 7, || {
            let mut dec = ruzstd::decoding::FrameDecoder::new();
            let mut out = Vec::with_capacity(original_size + 1024);
            dec.decode_all_to_vec(std::hint::black_box(compressed), &mut out)
                .unwrap();
            std::hint::black_box(&out);
        }),
        "structured-zstd" => bench_loop(3, target_ns, 7, || {
            let mut dec = structured_zstd::decoding::FrameDecoder::new();
            let mut out = vec![0u8; original_size + 1024];
            dec.decode_all(std::hint::black_box(compressed), &mut out)
                .unwrap();
            std::hint::black_box(&out);
        }),
        _ => unreachable!("unsupported codec for decode-only: {}", codec),
    };
    BenchResult {
        codec: codec.to_string(),
        input_name: name.into(),
        level,
        input_size: original_size,
        compressed_size,
        compress_ns: 0.0,
        decompress_ns,
    }
}

fn bench_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn corpus_path(relative: &str) -> PathBuf {
    bench_dir().join(relative)
}

fn ensure_corpus() {
    for &(rel, url) in SILESIA_DOWNLOADS {
        let path = corpus_path(rel);
        if path.exists() {
            continue;
        }
        eprintln!("downloading {url} ...");
        let dir = path.parent().unwrap();
        std::fs::create_dir_all(dir).ok();
        let path_str = path.display();
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("curl -fSL '{url}' | bzip2 -d > '{path_str}'"))
            .status();
        match status {
            Ok(s) if s.success() => {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                eprintln!("  saved {path_str} ({size} bytes)");
            }
            _ => {
                eprintln!("  failed to download {path_str}, skipping");
                std::fs::remove_file(&path).ok();
            }
        }
    }
}

fn cache_dir() -> PathBuf {
    let dir = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
        .join(".cache")
        .join("zrip")
        .join(std::env::consts::ARCH);
    std::fs::create_dir_all(&dir).ok();
    dir
}

fn level_cache_dir_inner(level: i32, small: bool, decode_only: bool) -> PathBuf {
    let mut dir = cache_dir();
    if small {
        dir = dir.join("small");
    }
    if decode_only {
        dir = dir.join("decode_cmp");
    }
    dir = dir.join(format!("L{}", level));
    std::fs::create_dir_all(&dir).ok();
    dir
}

fn level_codec_cache_path(level: i32, codec: &str, small: bool) -> PathBuf {
    level_cache_dir_inner(level, small, false).join(format!("{}.jsonl", codec.replace(' ', "_")))
}

fn level_codec_cache_path_decode(level: i32, codec: &str, small: bool) -> PathBuf {
    level_cache_dir_inner(level, small, true).join(format!("{}.jsonl", codec.replace(' ', "_")))
}

fn write_cache(results: &[BenchResult], small: bool) {
    write_cache_inner(results, small, false);
}

fn write_cache_decode(results: &[BenchResult], small: bool) {
    write_cache_inner(results, small, true);
}

fn write_cache_inner(results: &[BenchResult], small: bool, decode_only: bool) {
    let mut keys: Vec<(i32, &str)> = results
        .iter()
        .map(|r| (r.level, r.codec.as_str()))
        .collect();
    keys.sort();
    keys.dedup();

    for (level, codec) in &keys {
        let path = if decode_only {
            level_codec_cache_path_decode(*level, codec, small)
        } else {
            level_codec_cache_path(*level, codec, small)
        };
        let entries: Vec<_> = results
            .iter()
            .filter(|r| r.level == *level && r.codec == *codec)
            .collect();
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
}

fn parse_level_from_json(line: &str) -> Option<i32> {
    let idx = line.find("\"level\":")?;
    let rest = line[idx + 8..].trim_start();
    let end = rest.find(|c: char| c == ',' || c == '}')?;
    rest[..end].trim().parse().ok()
}

fn migrate_flat_cache() {
    let base = cache_dir();
    for codec_name in CODECS {
        let flat_name = format!("{}.jsonl", codec_name.replace(' ', "_"));
        let flat_path = base.join(&flat_name);
        if !flat_path.exists() {
            continue;
        }
        let content = match std::fs::read_to_string(&flat_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let mut by_level: Vec<(i32, Vec<&str>)> = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(level) = parse_level_from_json(line) {
                if let Some(entry) = by_level.iter_mut().find(|(l, _)| *l == level) {
                    entry.1.push(line);
                } else {
                    by_level.push((level, vec![line]));
                }
            }
        }

        if by_level.is_empty() {
            continue;
        }

        let mut total = 0;
        for (level, lines) in &by_level {
            let dest = level_codec_cache_path(*level, codec_name, false);
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&dest)
                .unwrap();
            for line in lines {
                writeln!(f, "{}", line).unwrap();
            }
            total += lines.len();
        }

        let bak = base.join(format!("{}.flat", flat_name));
        std::fs::rename(&flat_path, &bak).ok();
        eprintln!(
            "migrated {} entries from {} -> per-level dirs (backup: {})",
            total,
            flat_path.display(),
            bak.display()
        );
    }
}

#[cfg(feature = "paranoid")]
const ZRIP_CODEC: &str = "zrip paranoid";
#[cfg(not(feature = "paranoid"))]
const ZRIP_CODEC: &str = "zrip";

const CODECS: &[&str] = &["C zstd", ZRIP_CODEC, "ruzstd", "structured-zstd", "lz4rip"];
const DECODE_CODECS: &[&str] = &["C zstd", ZRIP_CODEC, "ruzstd", "structured-zstd"];
const DICT_CODECS: &[&str] = &["C zstd+dict", "zrip+dict"];

fn levels_for_codec<'a>(codec: &str, level_filter: &'a [i32]) -> &'a [i32] {
    match codec {
        "ruzstd" | "lz4rip" => &[1],
        _ if !level_filter.is_empty() => level_filter,
        "zrip" | "zrip paranoid" | "zrip+dict" => ZRIP_LEVELS,
        "C zstd" | "C zstd+dict" | "structured-zstd" => C_ZSTD_LEVELS,
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
        "zrip paranoid" => "paranoid",
        "C zstd+dict" => "Czstd+d",
        "zrip+dict" => "zrip+d",
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

fn load_cached_keys(
    small: bool,
    decode_only: bool,
) -> std::collections::HashSet<(String, i32, String)> {
    let mut keys = std::collections::HashSet::new();
    let mut base = if small {
        cache_dir().join("small")
    } else {
        cache_dir()
    };
    if decode_only {
        base = base.join("decode_cmp");
    }
    if !base.is_dir() {
        return keys;
    }
    for entry in std::fs::read_dir(&base).into_iter().flatten() {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let level_dir = entry.path();
        if !level_dir.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        if !dir_name.starts_with('L') {
            continue;
        }
        for file in std::fs::read_dir(&level_dir).into_iter().flatten() {
            let file = match file {
                Ok(f) => f,
                Err(_) => continue,
            };
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let (Some(codec), Some(level), Some(input)) = (
                    parse_str_field(line, "codec"),
                    parse_level_from_json(line),
                    parse_str_field(line, "input"),
                ) {
                    keys.insert((codec, level, input));
                }
            }
        }
    }
    keys
}

fn parse_str_field(line: &str, field: &str) -> Option<String> {
    let needle = format!("\"{}\": \"", field);
    let idx = line.find(&needle)?;
    let rest = &line[idx + needle.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut only: Vec<String> = Vec::new();
    let mut impl_specified = false;
    let mut file_filter: Vec<String> = Vec::new();
    let mut level_filter: Vec<i32> = Vec::new();
    let mut extra_files: Vec<String> = Vec::new();
    let mut small_only = false;
    let mut dict_mode = false;
    let mut decode_only = false;
    let mut reuse_cached = false;
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
            "--small-only" => small_only = true,
            "--dict" => dict_mode = true,
            "--decode-only" => decode_only = true,
            "--reuse" => reuse_cached = true,
            _ => {}
        }
        i += 1;
    }

    ensure_corpus();
    migrate_flat_cache();

    let cached_keys = if reuse_cached {
        let keys = load_cached_keys(small_only, decode_only);
        if !keys.is_empty() {
            eprintln!(
                "--reuse: {} cached results loaded, will skip those",
                keys.len()
            );
        }
        keys
    } else {
        std::collections::HashSet::new()
    };

    if decode_only {
        if !impl_specified {
            only.clear();
        } else if only.iter().any(|o| o == "all") {
            only.clear();
        }
    } else if !impl_specified {
        only.push("zrip".into());
    } else if only.iter().any(|o| o == "all") {
        only.clear();
    }

    let base_codecs: &[&str] = if decode_only {
        DECODE_CODECS
    } else if dict_mode {
        DICT_CODECS
    } else {
        CODECS
    };

    let active_codecs: Vec<&str> = base_codecs
        .iter()
        .copied()
        .filter(|c| only.is_empty() || only.iter().any(|o| c.contains(o.as_str())))
        .collect();

    let all_levels: Vec<i32> = if decode_only {
        let lvls = if level_filter.is_empty() {
            ZRIP_LEVELS
        } else {
            &level_filter
        };
        let mut v = lvls.to_vec();
        v.sort();
        v.dedup();
        v
    } else {
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
    let mut results_small: Vec<BenchResult> = Vec::new();

    let base_files: &[&str] = if small_only { SMALL_FILES } else { ALL_FILES };
    let all_paths: Vec<&str> = base_files
        .iter()
        .copied()
        .chain(extra_files.iter().map(|s| s.as_str()))
        .collect();

    // Pre-train dicts per source file (keyed by base name before _4k/_16k/etc.)
    let mut dicts: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
    if dict_mode {
        for path in &all_paths {
            let name = path.rsplit('/').next().unwrap();
            if !file_filter.is_empty() && !file_filter.iter().any(|f| f == name) {
                continue;
            }
            let source_name = dict_source_name(name);
            if dicts.contains_key(&source_name) {
                continue;
            }
            let source_path = dict_source_path(&source_name);
            let source_data = match std::fs::read(&source_path) {
                Ok(d) => d,
                Err(_) => {
                    eprintln!(
                        "dict: skipping {source_name} (source {} not found)",
                        source_path.display()
                    );
                    continue;
                }
            };
            eprintln!(
                "training dict for {source_name} from {} bytes...",
                source_data.len()
            );
            let dict_bytes = train_dict_for_file(&source_data, 16384);
            eprintln!("  dict size: {} bytes", dict_bytes.len());
            dicts.insert(source_name, dict_bytes);
        }
    }

    for rel in &all_paths {
        let name = rel.rsplit('/').next().unwrap();
        if !file_filter.is_empty() && !file_filter.iter().any(|f| f == name) {
            continue;
        }

        let path = corpus_path(rel);
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                eprintln!("skipping {}: not found", path.display());
                continue;
            }
        };

        eprintln!("{name} ({} bytes)", data.len());

        let dict_bytes = if dict_mode {
            let source_name = dict_source_name(name);
            dicts.get(&source_name)
        } else {
            None
        };

        for &level in &all_levels {
            let mut level_batch: Vec<BenchResult> = Vec::new();

            if decode_only {
                let mut compressor = zstd::bulk::Compressor::new(level).unwrap();
                let compressed = compressor.compress(&data).unwrap();
                for &codec in &active_codecs {
                    if cached_keys.contains(&(codec.to_string(), level, name.to_string())) {
                        continue;
                    }

                    let r = bench_decode_only(
                        codec,
                        &compressed,
                        data.len(),
                        compressed.len(),
                        name,
                        level,
                        target_ns,
                    );
                    level_batch.push(r);
                }
            } else {
                for &codec in &active_codecs {
                    let codec_levels = levels_for_codec(codec, &level_filter);
                    if !codec_levels.contains(&level) {
                        continue;
                    }

                    if cached_keys.contains(&(codec.to_string(), level, name.to_string())) {
                        continue;
                    }

                    let r = match codec {
                        "C zstd" => bench_c_zstd(&data, name, level, target_ns),
                        "zrip" | "zrip paranoid" => bench_zrip(&data, name, level, target_ns),
                        "ruzstd" => bench_ruzstd(&data, name, level, target_ns),
                        "structured-zstd" => bench_structured_zstd(&data, name, level, target_ns),
                        "lz4rip" => bench_lz4rip(&data, name, level, target_ns),
                        "zrip+dict" => {
                            if let Some(db) = dict_bytes {
                                bench_zrip_dict(&data, name, level, target_ns, db)
                            } else {
                                continue;
                            }
                        }
                        "C zstd+dict" => {
                            if let Some(db) = dict_bytes {
                                bench_c_zstd_dict(&data, name, level, target_ns, db)
                            } else {
                                continue;
                            }
                        }
                        _ => unreachable!(),
                    };
                    level_batch.push(r);
                }
            }

            if !level_batch.is_empty() {
                let refs: Vec<&BenchResult> = level_batch.iter().collect();
                print_live_line(name, level, &refs);
                if rel.starts_with("corpus/small/") {
                    results_small.extend(level_batch);
                } else {
                    results.extend(level_batch);
                }
            }
        }
    }

    if decode_only {
        write_cache_decode(&results, false);
        write_cache_decode(&results_small, true);
    } else {
        write_cache(&results, false);
        write_cache(&results_small, true);
    }
}

fn dict_source_name(file_name: &str) -> String {
    let base = file_name
        .trim_end_matches("_2k")
        .trim_end_matches("_4k")
        .trim_end_matches("_8k")
        .trim_end_matches("_16k")
        .trim_end_matches("_32k")
        .trim_end_matches("_64k")
        .trim_end_matches("_128k");
    let base = base
        .trim_end_matches(".txt")
        .trim_end_matches(".json")
        .trim_end_matches(".xml")
        .trim_end_matches(".pdf");
    base.to_string()
}

fn dict_source_path(source_name: &str) -> PathBuf {
    let rel = match source_name {
        "hdfs" => "corpus/hdfs.json",
        "dickens" => "corpus/dickens.txt",
        "xml_collection" => "corpus/xml_collection.xml",
        "reymont" => "corpus/reymont.pdf",
        "compression_1k" => "corpus/compression_1k.txt",
        "compression_34k" => "corpus/compression_34k.txt",
        "compression_65k" => "corpus/compression_65k.txt",
        "compression_66k_JSON" => "corpus/compression_66k_JSON.txt",
        _ => return corpus_path(&format!("corpus/silesia/{source_name}")),
    };
    corpus_path(rel)
}
