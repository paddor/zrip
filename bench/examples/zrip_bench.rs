extern crate libc;

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const ZRIP_LEVELS: &[i32] = &[-8, -7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4];
const C_ZSTD_LEVELS: &[i32] = &[-7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4];

#[derive(Clone, Copy)]
struct CorpusEntry {
    rel: &'static str,
    label: &'static str,
    url: &'static str,
    size: usize,
    sha256: &'static str,
}

#[derive(Clone, Copy)]
struct SmallSource {
    prefix: &'static str,
    base_label: &'static str,
}

#[derive(Clone, Copy)]
struct SmallSize {
    label: &'static str,
    bytes: usize,
}

struct BenchInput {
    name: String,
    data: Vec<u8>,
    sha256: String,
    is_small: bool,
    dict_source: String,
    dict_entry: Option<&'static CorpusEntry>,
}

const BASE_CORPUS: &[CorpusEntry] = &[
    CorpusEntry {
        rel: "corpus/silesia/dickens",
        label: "dickens",
        url: "https://sun.aei.polsl.pl/~sdeor/corpus/dickens.bz2",
        size: 10_192_446,
        sha256: "b24c37886142e11d0ee687db6ab06f936207aa7f2ea1fd1d9a36763c7a507e6a",
    },
    CorpusEntry {
        rel: "corpus/silesia/reymont",
        label: "reymont",
        url: "https://sun.aei.polsl.pl/~sdeor/corpus/reymont.bz2",
        size: 6_627_202,
        sha256: "0eac0114a3dfe6e2ee1f345a0f79d653cb26c3bc9f0ed79238af4933422b7578",
    },
    CorpusEntry {
        rel: "corpus/silesia/xml",
        label: "xml",
        url: "https://sun.aei.polsl.pl/~sdeor/corpus/xml.bz2",
        size: 5_345_280,
        sha256: "0e82e54e695c1938e4193448022543845b33020c8be6bf3bf3ead2224903e08c",
    },
    CorpusEntry {
        rel: "corpus/silesia/mr",
        label: "mr",
        url: "https://sun.aei.polsl.pl/~sdeor/corpus/mr.bz2",
        size: 9_970_564,
        sha256: "68637ed52e3e4860174ed2dc0840ac77d5f1a60abbcb13770d5754e3774d53e6",
    },
    CorpusEntry {
        rel: "corpus/silesia/mozilla",
        label: "mozilla",
        url: "https://sun.aei.polsl.pl/~sdeor/corpus/mozilla.bz2",
        size: 51_220_480,
        sha256: "657fc3764b0c75ac9de9623125705831ebbfbe08fed248df73bc2dc66e2a963b",
    },
    CorpusEntry {
        rel: "corpus/silesia/nci",
        label: "nci",
        url: "https://sun.aei.polsl.pl/~sdeor/corpus/nci.bz2",
        size: 33_553_445,
        sha256: "fc63a31770947b8c2062d3b19ca94c00485a232bb91b502021948fee983e1635",
    },
    CorpusEntry {
        rel: "corpus/silesia/ooffice",
        label: "ooffice",
        url: "https://sun.aei.polsl.pl/~sdeor/corpus/ooffice.bz2",
        size: 6_152_192,
        sha256: "e7ee013880d34dd5208283d0d3d91b07f442e067454276095ded14f322a656eb",
    },
    CorpusEntry {
        rel: "corpus/silesia/osdb",
        label: "osdb",
        url: "https://sun.aei.polsl.pl/~sdeor/corpus/osdb.bz2",
        size: 10_085_684,
        sha256: "60f027179302ca3ad87c58ac90b6be72ec23588aaa7a3b7fe8ecc0f11def3fa3",
    },
    CorpusEntry {
        rel: "corpus/silesia/samba",
        label: "samba",
        url: "https://sun.aei.polsl.pl/~sdeor/corpus/samba.bz2",
        size: 21_606_400,
        sha256: "93ba07bc44d8267789c1d911992f40b089ffa2140b4a160fac11ccae9a40e7b2",
    },
    CorpusEntry {
        rel: "corpus/silesia/sao",
        label: "sao",
        url: "https://sun.aei.polsl.pl/~sdeor/corpus/sao.bz2",
        size: 7_251_944,
        sha256: "c2d0ea2cc59d4c21b7fe43a71499342a00cbe530a1d5548770e91ecd6214adcc",
    },
    CorpusEntry {
        rel: "corpus/silesia/webster",
        label: "webster",
        url: "https://sun.aei.polsl.pl/~sdeor/corpus/webster.bz2",
        size: 41_458_703,
        sha256: "6a68f69b26daf09f9dd84f7470368553194a0b294fcfa80f1604efb11143a383",
    },
    CorpusEntry {
        rel: "corpus/silesia/x-ray",
        label: "x-ray",
        url: "https://sun.aei.polsl.pl/~sdeor/corpus/x-ray.bz2",
        size: 8_474_240,
        sha256: "7de9fce1405dc44ae5e6813ed21cd5751e761bd4265655a005d39b9685d1c9ad",
    },
];

const SMALL_SOURCES: &[SmallSource] = &[
    SmallSource {
        prefix: "dickens",
        base_label: "dickens",
    },
    SmallSource {
        prefix: "nci",
        base_label: "nci",
    },
    SmallSource {
        prefix: "xml",
        base_label: "xml",
    },
    SmallSource {
        prefix: "x-ray",
        base_label: "x-ray",
    },
];

const SMALL_SIZES: &[SmallSize] = &[
    SmallSize {
        label: "512",
        bytes: 512,
    },
    SmallSize {
        label: "1k",
        bytes: 1024,
    },
    SmallSize {
        label: "2k",
        bytes: 2048,
    },
    SmallSize {
        label: "4k",
        bytes: 4096,
    },
    SmallSize {
        label: "8k",
        bytes: 8192,
    },
    SmallSize {
        label: "16k",
        bytes: 16_384,
    },
    SmallSize {
        label: "32k",
        bytes: 32_768,
    },
    SmallSize {
        label: "64k",
        bytes: 65_536,
    },
    SmallSize {
        label: "128k",
        bytes: 131_072,
    },
    SmallSize {
        label: "256k",
        bytes: 262_144,
    },
    SmallSize {
        label: "512k",
        bytes: 524_288,
    },
    SmallSize {
        label: "1m",
        bytes: 1_048_576,
    },
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
    input_sha256: String,
}

impl BenchResult {
    fn to_json(&self) -> String {
        format!(
            concat!(
                r#"{{"codec": "{}", "input": "{}", "level": {}, "#,
                r#""input_size": {}, "compressed_size": {}, "#,
                r#""compress_ns": {:.1}, "decompress_ns": {:.1}, "#,
                r#""input_sha256": "{}"}}"#,
            ),
            self.codec,
            self.input_name,
            self.level,
            self.input_size,
            self.compressed_size,
            self.compress_ns,
            self.decompress_ns,
            self.input_sha256,
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
        input_sha256: String::new(),
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
        input_sha256: String::new(),
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
        input_sha256: String::new(),
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
        input_sha256: String::new(),
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
        input_sha256: String::new(),
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
        input_sha256: String::new(),
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
        input_sha256: String::new(),
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
        input_sha256: String::new(),
    }
}

fn bench_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn corpus_path(relative: &str) -> PathBuf {
    bench_dir().join(relative)
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(out, "{byte:02x}").unwrap();
    }
    out
}

fn verify_corpus_bytes(entry: &CorpusEntry, data: &[u8]) -> Result<String, String> {
    if data.len() != entry.size {
        return Err(format!(
            "{} has {} bytes, expected {}",
            entry.rel,
            data.len(),
            entry.size
        ));
    }
    let actual = sha256_hex(data);
    if actual != entry.sha256 {
        return Err(format!(
            "{} sha256 mismatch: got {actual}, expected {}",
            entry.rel, entry.sha256
        ));
    }
    Ok(actual)
}

fn download_bzip2(url: &str, tmp: &Path) -> Result<(), String> {
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "curl -fSL '{url}' | bzip2 -d > '{}'",
            tmp.display()
        ))
        .status()
        .map_err(|e| format!("failed to start curl/bzip2 for {url}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("curl/bzip2 failed for {url}: {status}"))
    }
}

fn ensure_corpus_entry(entry: &CorpusEntry) -> Result<(), String> {
    let path = corpus_path(entry.rel);
    if path.exists() {
        let data = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        match verify_corpus_bytes(entry, &data) {
            Ok(_) => return Ok(()),
            Err(e) => eprintln!("corpus mismatch, refreshing {}: {e}", path.display()),
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid corpus path {}", path.display()))?;
    let tmp = path.with_file_name(format!("{file_name}.part.{}", std::process::id()));
    std::fs::remove_file(&tmp).ok();
    eprintln!("downloading {} ...", entry.url);
    download_bzip2(entry.url, &tmp)?;

    let data = std::fs::read(&tmp).map_err(|e| format!("read {}: {e}", tmp.display()))?;
    let hash = match verify_corpus_bytes(entry, &data) {
        Ok(hash) => hash,
        Err(e) => {
            std::fs::remove_file(&tmp).ok();
            return Err(e);
        }
    };
    std::fs::rename(&tmp, &path)
        .map_err(|e| format!("rename {} -> {}: {e}", tmp.display(), path.display()))?;
    eprintln!("  saved {} ({} bytes, {hash})", path.display(), data.len());
    Ok(())
}

fn load_corpus_entry(entry: &'static CorpusEntry) -> Result<(Vec<u8>, String), String> {
    ensure_corpus_entry(entry)?;
    let path = corpus_path(entry.rel);
    let data = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let hash = verify_corpus_bytes(entry, &data)?;
    Ok((data, hash))
}

fn corpus_entry_by_label(label: &str) -> Option<&'static CorpusEntry> {
    BASE_CORPUS.iter().find(|entry| entry.label == label)
}

fn name_matches_filter(name: &str, file_filter: &[String]) -> bool {
    file_filter.is_empty() || file_filter.iter().any(|f| f == name)
}

fn small_source_matches_filter(prefix: &str, file_filter: &[String]) -> bool {
    file_filter.is_empty()
        || file_filter
            .iter()
            .any(|f| f == prefix || f.starts_with(&format!("{prefix}_")))
}

fn small_input_matches_filter(prefix: &str, size_label: &str, file_filter: &[String]) -> bool {
    file_filter.is_empty()
        || file_filter
            .iter()
            .any(|f| f == prefix || f == &format!("{prefix}_{size_label}"))
}

fn load_benchmark_inputs(
    small_only: bool,
    file_filter: &[String],
    extra_files: &[String],
) -> Result<Vec<BenchInput>, String> {
    let mut inputs = Vec::new();

    if small_only {
        for source in SMALL_SOURCES {
            if !small_source_matches_filter(source.prefix, file_filter) {
                continue;
            }
            let entry = corpus_entry_by_label(source.base_label)
                .unwrap_or_else(|| panic!("missing corpus entry {}", source.base_label));
            let (base, _) = load_corpus_entry(entry)?;
            for size in SMALL_SIZES {
                if !small_input_matches_filter(source.prefix, size.label, file_filter) {
                    continue;
                }
                if base.len() < size.bytes {
                    return Err(format!(
                        "{} has {} bytes, cannot create {}_{}",
                        entry.label,
                        base.len(),
                        source.prefix,
                        size.label
                    ));
                }
                let name = format!("{}_{}", source.prefix, size.label);
                let data = base[..size.bytes].to_vec();
                let sha256 = sha256_hex(&data);
                inputs.push(BenchInput {
                    name,
                    data,
                    sha256,
                    is_small: true,
                    dict_source: source.prefix.to_string(),
                    dict_entry: Some(entry),
                });
            }
        }
    } else {
        for entry in BASE_CORPUS {
            if !name_matches_filter(entry.label, file_filter) {
                continue;
            }
            let (data, sha256) = load_corpus_entry(entry)?;
            inputs.push(BenchInput {
                name: entry.label.to_string(),
                data,
                sha256,
                is_small: false,
                dict_source: dict_source_name(entry.label),
                dict_entry: Some(entry),
            });
        }
    }

    for extra in extra_files {
        let path = PathBuf::from(extra);
        let data = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(extra)
            .to_string();
        let sha256 = sha256_hex(&data);
        inputs.push(BenchInput {
            dict_source: dict_source_name(&name),
            name,
            data,
            sha256,
            is_small: false,
            dict_entry: None,
        });
    }

    if inputs.is_empty() {
        return Err("no corpus inputs selected".into());
    }

    Ok(inputs)
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
    let end = rest.find([',', '}'])?;
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
    if mbs >= 100.0 {
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
) -> HashSet<(String, i32, String, Option<String>)> {
    let mut keys = HashSet::new();
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
                    let input_sha256 = parse_str_field(line, "input_sha256");
                    keys.insert((codec, level, input, input_sha256));
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

fn print_help(program: &str) {
    println!(
        "\
Usage: {program} [OPTIONS]

Options:
  --impl <name>       Codec filter: zrip, C zstd, all, etc. Default: zrip
  --files <list>      Comma-separated corpus file basenames to run
  --levels <list>     Comma-separated levels, e.g. -7,1,3
  --extra <path>      Add an extra input file
  --small-only        Run the small-file corpus
  --dict              Run dictionary benchmarks
  --decode-only       Run decode-only benchmark set
  --reuse             Reuse cached results when available
  -h, --help          Print this help

Missing base corpus files are downloaded or generated under bench/corpus.
Small inputs are sliced in memory from base corpus files."
    );
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
            "-h" | "--help" => {
                print_help(&args[0]);
                return;
            }
            "--impl" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--impl requires a value");
                    std::process::exit(2);
                }
                impl_specified = true;
                only.push(args[i].clone());
            }
            "--files" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--files requires a value");
                    std::process::exit(2);
                }
                file_filter.extend(args[i].split(',').map(|s| s.to_string()));
            }
            "--levels" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--levels requires a value");
                    std::process::exit(2);
                }
                level_filter.extend(
                    args[i]
                        .split(',')
                        .filter_map(|s| s.trim().parse::<i32>().ok()),
                );
            }
            "--extra" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--extra requires a value");
                    std::process::exit(2);
                }
                extra_files.push(args[i].clone());
            }
            "--small-only" => small_only = true,
            "--dict" => dict_mode = true,
            "--decode-only" => decode_only = true,
            "--reuse" => reuse_cached = true,
            flag if flag.starts_with('-') => {
                eprintln!("unknown option: {flag}");
                eprintln!("try --help");
                std::process::exit(2);
            }
            _ => {}
        }
        i += 1;
    }

    let all_inputs = match load_benchmark_inputs(small_only, &file_filter, &extra_files) {
        Ok(inputs) => inputs,
        Err(e) => {
            eprintln!("corpus setup failed: {e}");
            std::process::exit(1);
        }
    };

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
        if !impl_specified || only.iter().any(|o| o == "all") {
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

    // Pre-train dicts per source file (keyed by base name before _4k/_16k/etc.)
    let mut dicts: HashMap<String, Vec<u8>> = HashMap::new();
    if dict_mode {
        for input in &all_inputs {
            if dicts.contains_key(&input.dict_source) {
                continue;
            }
            let source_data = if let Some(entry) = input.dict_entry {
                match load_corpus_entry(entry) {
                    Ok((data, _)) => data,
                    Err(e) => {
                        eprintln!("dict: skipping {} ({e})", input.dict_source);
                        continue;
                    }
                }
            } else {
                input.data.clone()
            };
            eprintln!(
                "training dict for {} from {} bytes...",
                input.dict_source,
                source_data.len()
            );
            let dict_bytes = train_dict_for_file(&source_data, 16384);
            eprintln!("  dict size: {} bytes", dict_bytes.len());
            dicts.insert(input.dict_source.clone(), dict_bytes);
        }
    }

    for input in &all_inputs {
        let name = input.name.as_str();
        let data = input.data.as_slice();
        eprintln!("{name} ({} bytes)", data.len());

        let dict_bytes = if dict_mode {
            dicts.get(&input.dict_source)
        } else {
            None
        };

        for &level in &all_levels {
            let mut level_batch: Vec<BenchResult> = Vec::new();

            if decode_only {
                let mut compressor = zstd::bulk::Compressor::new(level).unwrap();
                let compressed = compressor.compress(data).unwrap();
                for &codec in &active_codecs {
                    let cache_key = (
                        codec.to_string(),
                        level,
                        name.to_string(),
                        Some(input.sha256.clone()),
                    );
                    if cached_keys.contains(&cache_key) {
                        continue;
                    }

                    let mut r = bench_decode_only(
                        codec,
                        &compressed,
                        data.len(),
                        compressed.len(),
                        name,
                        level,
                        target_ns,
                    );
                    r.input_sha256.clone_from(&input.sha256);
                    level_batch.push(r);
                }
            } else {
                for &codec in &active_codecs {
                    let codec_levels = levels_for_codec(codec, &level_filter);
                    if !codec_levels.contains(&level) {
                        continue;
                    }

                    let cache_key = (
                        codec.to_string(),
                        level,
                        name.to_string(),
                        Some(input.sha256.clone()),
                    );
                    if cached_keys.contains(&cache_key) {
                        continue;
                    }

                    let mut r = match codec {
                        "C zstd" => bench_c_zstd(data, name, level, target_ns),
                        "zrip" | "zrip paranoid" => bench_zrip(data, name, level, target_ns),
                        "ruzstd" => bench_ruzstd(data, name, level, target_ns),
                        "structured-zstd" => bench_structured_zstd(data, name, level, target_ns),
                        "lz4rip" => bench_lz4rip(data, name, level, target_ns),
                        "zrip+dict" => {
                            if let Some(db) = dict_bytes {
                                bench_zrip_dict(data, name, level, target_ns, db)
                            } else {
                                continue;
                            }
                        }
                        "C zstd+dict" => {
                            if let Some(db) = dict_bytes {
                                bench_c_zstd_dict(data, name, level, target_ns, db)
                            } else {
                                continue;
                            }
                        }
                        _ => unreachable!(),
                    };
                    r.input_sha256.clone_from(&input.sha256);
                    level_batch.push(r);
                }
            }

            if !level_batch.is_empty() {
                let refs: Vec<&BenchResult> = level_batch.iter().collect();
                print_live_line(name, level, &refs);
                if input.is_small {
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
    let mut base = file_name;
    for size in SMALL_SIZES {
        let suffix = format!("_{}", size.label);
        if let Some(stripped) = base.strip_suffix(&suffix) {
            base = stripped;
            break;
        }
    }
    let base = base
        .trim_end_matches(".txt")
        .trim_end_matches(".json")
        .trim_end_matches(".xml")
        .trim_end_matches(".pdf");
    base.to_string()
}
