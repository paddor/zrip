//! Local performance gate for zrip's encode and decode paths.
//!
//! Floors come from `.perf_hw` in the repository root, which is ignored:
//! each machine keeps its own, per build. Without it, loose smoke floors
//! apply. See "Performance gate" in `DEVELOPMENT.md`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use zrip::push::{EndOp, PushDecoder, PushEncoder};
use zrip::{CompressContext, DecompressContext, Dictionary, FrameDecoder, FrameEncoder};

const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;

/// Inputs per case, so small cases cycle through distinct data.
const CASE_BYTES: usize = MIB;
const MAX_INPUTS: usize = 256;

/// The lowest share of the median a suggested floor keeps, however widely
/// a case's runs spread.
const MIN_KEEP: f64 = 0.85;

/// One timed batch runs at least this long, so reading the clock costs well
/// under 1% of it.
const MIN_BATCH_NS: u64 = 100_000;

/// A way to build the gate. Builds differ in code generation, so each keeps
/// its own floors.
struct Build {
    name: &'static str,
    about: &'static str,
    /// `CARGO_ENCODED_RUSTFLAGS`, or `None` for the repository's config.
    rustflags: Option<&'static str>,
    lto: bool,
    features: &'static str,
}

const BUILDS: &[Build] = &[
    Build {
        name: "native",
        about: "the repository's `.cargo/config.toml`: `target-cpu=native`",
        rustflags: None,
        lto: false,
        features: "",
    },
    Build {
        name: "portable",
        about: "no rustflags, as dependent crates build: runtime SIMD dispatch",
        rustflags: Some(""),
        lto: false,
        features: "",
    },
    Build {
        name: "portable-lto",
        about: "portable with fat LTO and one codegen unit",
        rustflags: Some(""),
        lto: true,
        features: "",
    },
    Build {
        name: "paranoid",
        about: "native with the `paranoid` feature: no unsafe primitives",
        rustflags: None,
        lto: false,
        features: "paranoid",
    },
];

/// The build this binary is, as `--builds` names it at compile time. A
/// plain `cargo run` from the repository root builds `native`.
fn build_name() -> &'static str {
    option_env!("ZRIP_PERF_BUILD").unwrap_or(if cfg!(feature = "paranoid") {
        "paranoid"
    } else if cfg!(target_feature = "avx2") {
        "native"
    } else {
        "portable"
    })
}

#[derive(Clone, Copy)]
enum Data {
    /// JSON log lines.
    Text,
    /// Little-endian sensor records: timestamp, id, `f32` value, flags.
    Binary,
    /// Training metric points: key, noisy `f64` value, step, timestamp.
    /// Few long matches and many short ones.
    Points,
    /// Slices spread over every Silesia file, when the corpus is present.
    Silesia,
    /// Slices of Silesia's English text, `dickens` and `webster`: large
    /// literal sections with short codes, which take the two-symbol Huffman
    /// decoder. In `Silesia`, gains on the other files can hide a slowdown
    /// there.
    SilesiaText,
}

#[derive(Clone, Copy)]
enum Op {
    /// `zrip::compress`, one call per input.
    Compress,
    /// A new `CompressContext` per input.
    Fresh,
    /// One reused `CompressContext`.
    Context,
    /// One reused `CompressContext`, `compress_into` a reused buffer.
    Into,
    /// One reused `PushEncoder`, a frame per input.
    PushFrames,
    /// One `PushEncoder` frame per input, fed 16 KiB at a time.
    PushStream,
    /// One `PushEncoder` frame per pass, each input a flushed message.
    PushFlush,
    /// `FrameEncoder` into a `Vec`, fed 64 KiB at a time.
    Stream,
    /// One reused `CompressContext::with_dict`.
    DictContext,
    /// `zrip::decompress` of the level's frames.
    Decompress,
    /// One reused `DecompressContext`.
    DecompressContext,
    /// One reused `DecompressContext`, `decompress_into` a reused `Vec`.
    DecompressInto,
    /// `FrameDecoder` reading each frame to the end.
    DecodeStream,
    /// One reused `PushDecoder`, fed 16 KiB at a time.
    DecodePush,
    /// One reused `DecompressContext::with_dict`.
    DecompressDict,
}

impl Op {
    const fn encodes(self) -> bool {
        !matches!(
            self,
            Op::Decompress
                | Op::DecompressContext
                | Op::DecompressInto
                | Op::DecodeStream
                | Op::DecodePush
                | Op::DecompressDict
        )
    }
}

struct Case {
    section: &'static str,
    key: &'static str,
    op: Op,
    level: i32,
    data: Data,
    len: usize,
}

const fn case(
    section: &'static str,
    key: &'static str,
    op: Op,
    level: i32,
    data: Data,
    len: usize,
) -> Case {
    Case {
        section,
        key,
        op,
        level,
        data,
        len,
    }
}

/// Every case, named `section.key`. The shapes follow how zrip is used:
/// one-shot calls per message, contexts reused across messages or created
/// per message, push streams with and without flushes, large streams, small
/// dictionary frames, and the matching decoders. Sizes run from 64 B
/// messages to 4 MiB streams.
#[rustfmt::skip]
fn cases() -> Vec<Case> {
    use Data::{Binary, Points, Silesia, SilesiaText, Text};
    use Op::*;
    vec![
        case("compress_l1", "64b", Compress, 1, Text, 64),
        case("compress_l1", "256b", Compress, 1, Text, 256),
        case("compress_l1", "1k", Compress, 1, Text, KIB),
        case("compress_l1", "4k", Compress, 1, Text, 4 * KIB),
        case("compress_l1", "16k", Compress, 1, Text, 16 * KIB),
        case("compress_l1", "256k", Compress, 1, Text, 256 * KIB),
        case("compress_l1", "1m", Compress, 1, Text, MIB),
        case("compress_l3", "64b", Compress, 3, Text, 64),
        case("compress_l3", "256b", Compress, 3, Text, 256),
        case("compress_l3", "1k", Compress, 3, Text, KIB),
        case("compress_l3", "4k", Compress, 3, Text, 4 * KIB),
        case("compress_l3", "16k", Compress, 3, Text, 16 * KIB),
        case("compress_l3", "256k", Compress, 3, Text, 256 * KIB),
        case("compress_l3", "1m", Compress, 3, Text, MIB),
        case("levels_64k", "l-7", Compress, -7, Text, 64 * KIB),
        case("levels_64k", "l-5", Compress, -5, Text, 64 * KIB),
        case("levels_64k", "l-3", Compress, -3, Text, 64 * KIB),
        case("levels_64k", "l-1", Compress, -1, Text, 64 * KIB),
        case("levels_64k", "l2", Compress, 2, Text, 64 * KIB),
        case("levels_64k", "l4", Compress, 4, Text, 64 * KIB),
        case("fresh_l1", "1k", Fresh, 1, Text, KIB),
        case("fresh_l1", "64k", Fresh, 1, Text, 64 * KIB),
        case("fresh_l3", "1k", Fresh, 3, Text, KIB),
        case("fresh_l3", "64k", Fresh, 3, Text, 64 * KIB),
        case("context_l1", "2k", Context, 1, Text, 2 * KIB),
        case("context_l1", "64k", Context, 1, Text, 64 * KIB),
        case("context_l1", "64k_bin", Context, 1, Binary, 64 * KIB),
        case("context_l1", "1m_bin", Context, 1, Binary, MIB),
        case("context_l3", "2k", Context, 3, Text, 2 * KIB),
        case("context_l3", "16k", Context, 3, Text, 16 * KIB),
        case("context_l3", "256k", Context, 3, Text, 256 * KIB),
        case("context_l3", "4m", Context, 3, Text, 4 * MIB),
        case("context_l3", "16k_bin", Context, 3, Binary, 16 * KIB),
        case("into_l3", "16k", Into, 3, Text, 16 * KIB),
        case("points_l1", "16k", Context, 1, Points, 16 * KIB),
        case("points_l1", "256k", Context, 1, Points, 256 * KIB),
        case("points_l3", "2k", Context, 3, Points, 2 * KIB),
        case("points_l3", "16k", Context, 3, Points, 16 * KIB),
        case("points_l3", "256k", Context, 3, Points, 256 * KIB),
        case("points_l4", "64k", Context, 4, Points, 64 * KIB),
        case("push_l1", "256b_flush", PushFlush, 1, Text, 256),
        case("push_l3", "256b_flush", PushFlush, 3, Text, 256),
        case("push_l3", "4k_frames", PushFrames, 3, Text, 4 * KIB),
        case("push_l3", "1m", PushStream, 3, Text, MIB),
        case("stream_l1", "4m", Stream, 1, Text, 4 * MIB),
        case("stream_l3", "4m", Stream, 3, Text, 4 * MIB),
        case("dict_l1", "64b", DictContext, 1, Text, 64),
        case("dict_l1", "1k", DictContext, 1, Text, KIB),
        case("dict_l3", "64b", DictContext, 3, Text, 64),
        case("dict_l3", "256b", DictContext, 3, Text, 256),
        case("dict_l3", "1k", DictContext, 3, Text, KIB),
        case("dict_l3", "4k", DictContext, 3, Text, 4 * KIB),
        case("decompress", "1k", Decompress, 1, Text, KIB),
        case("decompress", "16k", Decompress, 1, Text, 16 * KIB),
        case("decompress", "256k", Decompress, 1, Text, 256 * KIB),
        case("decompress", "4m", Decompress, 3, Text, 4 * MIB),
        case("dcontext", "64b", DecompressContext, 1, Text, 64),
        case("dcontext", "2k", DecompressContext, 3, Text, 2 * KIB),
        case("dcontext", "64k", DecompressContext, 3, Text, 64 * KIB),
        case("dcontext", "64k_l-5", DecompressContext, -5, Text, 64 * KIB),
        case("dcontext", "64k_points", DecompressContext, 3, Points, 64 * KIB),
        case("dcontext", "1m_bin", DecompressContext, 1, Binary, MIB),
        case("dinto", "16k", DecompressInto, 3, Text, 16 * KIB),
        case("dstream", "4m", DecodeStream, 3, Text, 4 * MIB),
        case("dpush", "4m", DecodePush, 3, Text, 4 * MIB),
        case("dict_decode", "64b", DecompressDict, 3, Text, 64),
        case("dict_decode", "1k", DecompressDict, 3, Text, KIB),
        case("silesia_l-7", "256k_ctx", Context, -7, Silesia, 256 * KIB),
        case("silesia_l-3", "64k_ctx", Context, -3, Silesia, 64 * KIB),
        case("silesia_l1", "1k", Compress, 1, Silesia, KIB),
        case("silesia_l1", "16k_ctx", Context, 1, Silesia, 16 * KIB),
        case("silesia_l1", "256k_ctx", Context, 1, Silesia, 256 * KIB),
        case("silesia_l3", "2k_ctx", Context, 3, Silesia, 2 * KIB),
        case("silesia_l3", "16k_ctx", Context, 3, Silesia, 16 * KIB),
        case("silesia_l3", "64k", Compress, 3, Silesia, 64 * KIB),
        case("silesia_l3", "256k_ctx", Context, 3, Silesia, 256 * KIB),
        case("silesia_l3", "1m_ctx", Context, 3, Silesia, MIB),
        case("silesia_l4", "256k_ctx", Context, 4, Silesia, 256 * KIB),
        case("silesia_decode", "16k", DecompressContext, 3, Silesia, 16 * KIB),
        case("silesia_decode", "256k", DecompressContext, 1, Silesia, 256 * KIB),
        case("silesia_text_decode", "128k_l-5", DecompressContext, -5, SilesiaText, 128 * KIB),
        case("silesia_text_decode", "128k_l1", DecompressContext, 1, SilesiaText, 128 * KIB),
    ]
}

/// Loose floors for machines without `.perf_hw`. They catch collapses,
/// such as tables cleared per call, not percent-level regressions.
const SMOKE: &[(&str, f64)] = &[
    ("compress_l1.16k_mb_s", 50.0),
    ("compress_l3.16k_mb_s", 30.0),
    ("context_l1.2k_mb_s", 30.0),
    ("context_l3.2k_mb_s", 20.0),
    ("points_l3.16k_mb_s", 20.0),
    ("dict_l3.64b_mb_s", 5.0),
    ("decompress.16k_mb_s", 100.0),
];

/// SplitMix64: small, seeded, and the same on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn text(len: usize, seed: u64) -> Vec<u8> {
    const LEVELS: &[&str] = &["info", "info", "info", "debug", "warn", "error"];
    const PATHS: &[&str] = &["items", "users", "orders", "sessions", "metrics"];
    const WORDS: &[&str] = &[
        "request", "served", "from", "cache", "miss", "hit", "user", "session", "timeout", "retry",
        "upstream", "payload", "accepted", "rejected", "queue", "flush", "commit", "batch", "of",
        "records", "after", "the", "slow", "path",
    ];
    let mut rng = Rng(seed);
    let mut ts = 1_731_900_000_000 + seed * 7_919;
    let mut out = String::with_capacity(len + 256);
    while out.len() < len {
        ts += rng.below(40);
        let level = LEVELS[rng.below(LEVELS.len() as u64) as usize];
        let path = PATHS[rng.below(PATHS.len() as u64) as usize];
        let status = [200, 200, 200, 201, 204, 304, 404, 500][rng.below(8) as usize];
        let _ = write!(
            out,
            "{{\"ts\":{ts},\"host\":\"node-{}\",\"level\":\"{level}\",\"path\":\"/api/v1/{path}/{}\",\"status\":{status},\"bytes\":{},\"ms\":{}.{},\"msg\":\"",
            rng.below(24),
            rng.below(100_000),
            rng.below(65_536),
            rng.below(300),
            rng.below(10),
        );
        for i in 0..2 + rng.below(7) {
            if i > 0 {
                out.push(' ');
            }
            out.push_str(WORDS[rng.below(WORDS.len() as u64) as usize]);
        }
        out.push_str("\"}\n");
    }
    let mut out = out.into_bytes();
    out.truncate(len);
    out
}

fn binary(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng(seed);
    let mut ts = 1_731_900_000_000u64 + seed * 7_919;
    let mut value = 20.0f32;
    let mut out = Vec::with_capacity(len + 32);
    while out.len() < len {
        ts += 1_000 + rng.below(16);
        value += (rng.below(201) as f32 - 100.0) / 400.0;
        out.extend_from_slice(&ts.to_le_bytes());
        out.extend_from_slice(&(rng.below(64) as u32).to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
        out.extend_from_slice(&(rng.below(4) as u16).to_le_bytes());
    }
    out.truncate(len);
    out
}

/// Six metrics per training step: a decaying loss, rising accuracy, a
/// learning rate, a gradient norm, throughput and a memory gauge, each with
/// noise in its low bits, as a metrics client batches them.
fn points(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng(seed ^ 0x7068_6f74_6f6e);
    let mut step = 12_000 + seed * 1_000;
    let mut ts = 1_731_000_000_000u64 + seed * 7_919;
    let mut out = Vec::with_capacity(len + 32);
    let noise = |rng: &mut Rng| rng.unit() + rng.unit() + rng.unit() - 1.5;
    while out.len() < len {
        let s = step as f64;
        let loss = 0.35 + 2.5 / (1.0 + s / 2_000.0);
        let values = [
            loss + 0.04 * noise(&mut rng),
            0.95 - 0.6 / (1.0 + s / 1_500.0) + 0.01 * noise(&mut rng),
            3e-4 * (s / 2_000.0).min(1.0),
            0.8 + 0.25 * noise(&mut rng),
            (48_000.0 + 900.0 * noise(&mut rng)).round(),
            37.25,
        ];
        for (key, value) in values.into_iter().enumerate() {
            out.extend_from_slice(&(key as u32).to_le_bytes());
            out.extend_from_slice(&value.to_le_bytes());
            out.extend_from_slice(&step.to_le_bytes());
            out.extend_from_slice(&(ts + key as u64 / 3).to_le_bytes());
        }
        step += 1;
        ts += 330 + rng.below(40);
    }
    out.truncate(len);
    out
}

fn inputs(case: &Case, corpus: &[Vec<u8>]) -> Vec<Vec<u8>> {
    if let Data::Silesia | Data::SilesiaText = case.data {
        // The same share of every file, at evenly spaced offsets.
        let per_file = (CASE_BYTES / corpus.len() / case.len).max(1);
        return corpus
            .iter()
            .filter(|file| file.len() >= case.len)
            .flat_map(|file| {
                let room = file.len() - case.len;
                (0..per_file).map(move |j| file[room * j / per_file..][..case.len].to_vec())
            })
            .collect();
    }
    let count = (CASE_BYTES / case.len).clamp(1, MAX_INPUTS);
    (0..count as u64)
        .map(|seed| match case.data {
            Data::Text => text(case.len, seed),
            Data::Binary => binary(case.len, seed),
            Data::Points => points(case.len, seed),
            Data::Silesia | Data::SilesiaText => unreachable!(),
        })
        .collect()
}

/// The Silesia files, from `ZRIP_PERF_CORPUS` or `bench/corpus/silesia`
/// (see "Corpus benchmarks" in `DEVELOPMENT.md`), or none if absent.
fn corpus_paths() -> Vec<PathBuf> {
    let dir = std::env::var_os("ZRIP_PERF_CORPUS").map_or_else(
        || PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/corpus/silesia")),
        PathBuf::from,
    );
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .collect();
    paths.sort();
    paths
}

fn corpus() -> Vec<Vec<u8>> {
    read_all(&corpus_paths())
}

/// The English text files of the corpus.
fn corpus_text() -> Vec<Vec<u8>> {
    let mut paths = corpus_paths();
    paths.retain(|p| p.file_name().is_some_and(|n| n == "dickens" || n == "webster"));
    read_all(&paths)
}

fn read_all(paths: &[PathBuf]) -> Vec<Vec<u8>> {
    paths
        .iter()
        .map(|p| std::fs::read(p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display())))
        .collect()
}

/// A dictionary trained on records like the inputs, but none of them.
fn dictionary() -> Dictionary {
    let samples: Vec<Vec<u8>> = (10_000..10_400).map(|seed| text(KIB, seed)).collect();
    let refs: Vec<&[u8]> = samples.iter().map(Vec::as_slice).collect();
    zrip::dict::train_dict_fastcover(&refs, 16 * KIB, Default::default())
}

fn cpu_nanos() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

struct Settings {
    rounds: usize,
    round_ns: u64,
    repeat: usize,
}

/// The fastest of `rounds` rounds, in nanoseconds per call of `f`. Process
/// CPU time leaves out time the VM spent elsewhere.
fn time<F: FnMut()>(settings: &Settings, mut f: F) -> f64 {
    for _ in 0..2 {
        f();
    }
    let mut batch = 1u64;
    loop {
        let start = cpu_nanos();
        for _ in 0..batch {
            f();
        }
        if cpu_nanos() - start >= MIN_BATCH_NS || batch >= 1 << 20 {
            break;
        }
        batch *= 2;
    }
    let mut best = f64::MAX;
    for _ in 0..settings.rounds {
        let mut calls = 0u64;
        let start = cpu_nanos();
        while cpu_nanos() - start < settings.round_ns {
            for _ in 0..batch {
                f();
            }
            calls += batch;
        }
        best = best.min((cpu_nanos() - start) as f64 / calls as f64);
    }
    best
}

struct Measured {
    mb_s: f64,
    /// Input over output bytes, for encode cases.
    ratio: Option<f64>,
}

/// Each input's frame, from one reused context at the case's level.
fn frames(case: &Case, inputs: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut enc = CompressContext::new(case.level).unwrap();
    inputs
        .iter()
        .map(|i| enc.compress(i).unwrap().into_owned())
        .collect()
}

/// Times one pass over all inputs and returns throughput in MB/s of
/// uncompressed bytes.
fn measure(
    case: &Case,
    inputs: &[Vec<u8>],
    dict: Option<&Dictionary>,
    settings: &Settings,
) -> Measured {
    use std::hint::black_box;
    let dict = || dict.expect("a dictionary case").clone();
    let bytes: usize = inputs.iter().map(Vec::len).sum();
    let level = case.level;
    let mut out_bytes = 0;
    let ns = match case.op {
        Op::Compress => {
            out_bytes = inputs
                .iter()
                .map(|i| zrip::compress(i, level).unwrap().len())
                .sum();
            time(settings, || {
                for i in inputs {
                    black_box(zrip::compress(black_box(i), level).unwrap());
                }
            })
        }
        Op::Fresh => {
            out_bytes = inputs
                .iter()
                .map(|i| zrip::compress(i, level).unwrap().len())
                .sum();
            time(settings, || {
                for i in inputs {
                    let mut ctx = CompressContext::new(level).unwrap();
                    black_box(ctx.compress(black_box(i)).unwrap().len());
                }
            })
        }
        Op::Context | Op::DictContext => {
            let mut ctx = match case.op {
                Op::Context => CompressContext::new(level).unwrap(),
                _ => CompressContext::with_dict(level, dict()).unwrap(),
            };
            out_bytes = inputs.iter().map(|i| ctx.compress(i).unwrap().len()).sum();
            time(settings, || {
                for i in inputs {
                    black_box(ctx.compress(black_box(i)).unwrap());
                }
            })
        }
        Op::Into => {
            let mut ctx = CompressContext::new(level).unwrap();
            let mut buf = vec![0u8; zrip::compress_bound(case.len)];
            out_bytes = inputs
                .iter()
                .map(|i| ctx.compress_into(i, &mut buf).unwrap())
                .sum();
            time(settings, || {
                for i in inputs {
                    black_box(ctx.compress_into(black_box(i), &mut buf).unwrap());
                }
            })
        }
        Op::PushFrames | Op::PushStream | Op::PushFlush => {
            let mut enc = PushEncoder::new(level).unwrap();
            let mut out = Vec::new();
            let chunk = match case.op {
                Op::PushStream => 16 * KIB,
                _ => usize::MAX,
            };
            let mut run = |out: &mut Vec<u8>| {
                for i in inputs {
                    if let Op::PushFlush = case.op {
                        enc.encode_to_vec(black_box(i), out, EndOp::Flush).unwrap();
                        continue;
                    }
                    let mut parts = i.chunks(chunk.min(i.len())).peekable();
                    while let Some(part) = parts.next() {
                        let op = if parts.peek().is_some() {
                            EndOp::Continue
                        } else {
                            EndOp::End
                        };
                        enc.encode_to_vec(black_box(part), out, op).unwrap();
                    }
                }
                if let Op::PushFlush = case.op {
                    enc.encode_to_vec(&[], out, EndOp::End).unwrap();
                }
            };
            run(&mut out);
            out_bytes = out.len();
            time(settings, || {
                out.clear();
                run(&mut out);
                black_box(&out);
            })
        }
        Op::Stream => {
            let encode = |i: &[u8]| {
                let mut enc = FrameEncoder::new(Vec::new(), level).unwrap();
                for part in i.chunks(64 * KIB) {
                    enc.write_all(black_box(part)).unwrap();
                }
                enc.finish().unwrap()
            };
            out_bytes = inputs.iter().map(|i| encode(i).len()).sum();
            time(settings, || {
                for i in inputs {
                    black_box(encode(i));
                }
            })
        }
        Op::Decompress => {
            let frames = frames(case, inputs);
            time(settings, || {
                for f in &frames {
                    black_box(zrip::decompress(black_box(f)).unwrap());
                }
            })
        }
        Op::DecompressContext | Op::DecompressDict => {
            let (frames, mut dec) = match case.op {
                Op::DecompressContext => (frames(case, inputs), DecompressContext::new()),
                _ => {
                    let mut enc = CompressContext::with_dict(level, dict()).unwrap();
                    let frames = inputs
                        .iter()
                        .map(|i| enc.compress(i).unwrap().into_owned())
                        .collect();
                    (frames, DecompressContext::with_dict(dict()))
                }
            };
            for (f, i) in frames.iter().zip(inputs) {
                assert_eq!(
                    &*dec.decompress(f).unwrap(),
                    i.as_slice(),
                    "{}",
                    case.section
                );
            }
            time(settings, || {
                for f in &frames {
                    black_box(dec.decompress(black_box(f)).unwrap());
                }
            })
        }
        Op::DecompressInto => {
            let frames = frames(case, inputs);
            let mut dec = DecompressContext::new();
            let mut out = Vec::new();
            time(settings, || {
                for f in &frames {
                    out.clear();
                    black_box(dec.decompress_into(black_box(f), &mut out).unwrap());
                }
            })
        }
        Op::DecodeStream => {
            let frames = frames(case, inputs);
            let mut out = Vec::new();
            time(settings, || {
                for f in &frames {
                    out.clear();
                    let mut dec = FrameDecoder::new(black_box(f.as_slice()));
                    dec.read_to_end(&mut out).unwrap();
                    black_box(&out);
                }
            })
        }
        Op::DecodePush => {
            let frames = frames(case, inputs);
            let mut dec = PushDecoder::new();
            let mut out = Vec::new();
            time(settings, || {
                for f in &frames {
                    out.clear();
                    for part in f.chunks(16 * KIB) {
                        dec.decode_to_vec(black_box(part), &mut out).unwrap();
                    }
                    dec.finish().unwrap();
                    black_box(&out);
                }
            })
        }
    };
    Measured {
        mb_s: bytes as f64 / ns * 1e3,
        ratio: case.op.encodes().then(|| bytes as f64 / out_bytes as f64),
    }
}

/// `.perf_hw`: `[build.section]` headers and `key=value` lines, `#`
/// comments. Keys come back as `build.section.key`.
fn parse_floors(contents: &str) -> BTreeMap<String, f64> {
    let mut section = String::new();
    let mut floors = BTreeMap::new();
    for line in contents.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = name.trim().to_string();
        } else if let Some((key, value)) = line.split_once('=') {
            let value = value
                .trim()
                .parse()
                .unwrap_or_else(|_| panic!("bad value in .perf_hw: {line}"));
            floors.insert(format!("{section}.{}", key.trim()), value);
        }
    }
    floors
}

fn hw_path() -> PathBuf {
    std::env::var_os("ZRIP_PERF_HW").map_or_else(
        || PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.perf_hw")),
        PathBuf::from,
    )
}

/// Pins the process to `ZRIP_PERF_CPU`, by default the last CPU it may run
/// on. `ZRIP_PERF_CPU=none` leaves it unpinned.
fn pin() -> String {
    let wanted = std::env::var("ZRIP_PERF_CPU").ok();
    if wanted.as_deref() == Some("none") {
        return "unpinned".into();
    }
    // SAFETY: `set` is a plain bitmask that the calls below only read and
    // write within its size.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        let size = std::mem::size_of::<libc::cpu_set_t>();
        if libc::sched_getaffinity(0, size, &mut set) != 0 {
            return "unpinned (sched_getaffinity failed)".into();
        }
        let cpu = match wanted {
            Some(cpu) => cpu
                .parse()
                .expect("ZRIP_PERF_CPU must be a CPU number or none"),
            None => match (0..libc::CPU_SETSIZE as usize)
                .rev()
                .find(|&c| libc::CPU_ISSET(c, &set))
            {
                Some(cpu) => cpu,
                None => return "unpinned".into(),
            },
        };
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(cpu, &mut set);
        assert_eq!(
            libc::sched_setaffinity(0, size, &set),
            0,
            "cannot pin to CPU {cpu}"
        );
        format!("cpu {cpu}")
    }
}

fn env_number(name: &str, default: u64) -> u64 {
    std::env::var(name).map_or(default, |v| {
        v.parse()
            .unwrap_or_else(|_| panic!("{name} must be a number"))
    })
}

struct Options {
    cases: Vec<String>,
    builds: Vec<&'static Build>,
    measure_only: bool,
    list: bool,
    /// Measure one case and print the result for the parent.
    worker: Option<String>,
    suggest: Option<f64>,
    repeat: usize,
    /// The arguments to hand each build's gate.
    passthrough: Vec<String>,
}

fn parse_args() -> Options {
    let mut options = Options {
        cases: Vec::new(),
        builds: Vec::new(),
        measure_only: false,
        list: false,
        worker: None,
        suggest: None,
        repeat: 1,
        passthrough: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |what: &str| args.next().unwrap_or_else(|| panic!("{arg} needs {what}"));
        match arg.as_str() {
            "--builds" => {
                let names = value("build names, such as native,portable");
                for name in names.split(',') {
                    if name == "all" {
                        options.builds.extend(BUILDS.iter());
                        continue;
                    }
                    let build = BUILDS.iter().find(|b| b.name == name).unwrap_or_else(|| {
                        panic!(
                            "unknown build {name}; builds: all, {}",
                            BUILDS.iter().map(|b| b.name).collect::<Vec<_>>().join(", ")
                        )
                    });
                    options.builds.push(build);
                }
                continue;
            }
            "--case" => {
                let name = value("a name");
                options.passthrough.extend([arg.clone(), name.clone()]);
                options.cases.push(name);
                continue;
            }
            "--repeat" => {
                let n = value("a count");
                options.repeat = n.parse().expect("--repeat needs a count");
                assert!(options.repeat > 0, "--repeat must be positive");
                options.passthrough.extend([arg.clone(), n]);
                continue;
            }
            "--suggest" => {
                let f = value("a fraction, such as 0.97");
                let fraction: f64 = f.parse().expect("--suggest needs a fraction");
                assert!(
                    fraction > 0.0 && fraction <= 1.0,
                    "--suggest takes a fraction in (0, 1]"
                );
                options.suggest = Some(fraction);
                options.passthrough.extend([arg.clone(), f]);
                continue;
            }
            "--worker" => {
                options.worker = Some(value("a case name"));
                continue;
            }
            "--measure-only" => options.measure_only = true,
            "--list" => options.list = true,
            _ => panic!("unknown option: {arg} (see DEVELOPMENT.md, \"Performance gate\")"),
        }
        options.passthrough.push(arg);
    }
    options
}

/// Builds the gate as `build` next to this binary's target directory and
/// returns the binary.
fn build_gate(build: &Build) -> PathBuf {
    let exe = std::env::current_exe().expect("current_exe");
    // `<target>/release/zrip_perf_verify`
    let target = exe
        .parent()
        .and_then(Path::parent)
        .expect("binary under <target>/release");
    let dir = target.join(format!("perf-{}", build.name));
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    // From the repository root, so its `.cargo/config.toml` applies.
    cmd.current_dir(&root)
        .args(["build", "--release", "--quiet", "--bin", "zrip_perf_verify"])
        .arg("--manifest-path")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .env("CARGO_TARGET_DIR", &dir)
        .env("ZRIP_PERF_BUILD", build.name)
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS");
    if !build.features.is_empty() {
        cmd.args(["--features", build.features]);
    }
    if let Some(flags) = build.rustflags {
        cmd.env("CARGO_ENCODED_RUSTFLAGS", flags);
    }
    if build.lto {
        cmd.env("CARGO_PROFILE_RELEASE_LTO", "fat")
            .env("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "1");
    }
    eprintln!("building {}: {}", build.name, build.about);
    let status = cmd.status().expect("cannot run cargo");
    assert!(status.success(), "building {} failed", build.name);
    dir.join("release").join("zrip_perf_verify")
}

/// Builds and runs the gate once per build, serially. Returns whether every
/// build passed.
fn run_builds(options: &Options) -> bool {
    let bins: Vec<(&Build, PathBuf)> = options.builds.iter().map(|&b| (b, build_gate(b))).collect();
    let mut floors = String::new();
    let mut failed = Vec::new();
    for (build, bin) in bins {
        eprintln!("\n== {}", build.name);
        let mut cmd = Command::new(&bin);
        cmd.args(&options.passthrough)
            .env("ZRIP_PERF_CHILD", "1")
            .stderr(Stdio::inherit())
            .stdout(if options.suggest.is_some() {
                Stdio::piped()
            } else {
                Stdio::inherit()
            });
        let output = cmd.output().expect("cannot run the gate");
        floors.push_str(&String::from_utf8(output.stdout).unwrap());
        if !output.status.success() {
            failed.push(build.name);
        }
    }
    if let Some(fraction) = options.suggest {
        print!("{}{floors}", floors_header(fraction));
    }
    if !failed.is_empty() {
        eprintln!("\nFAIL in builds: {}", failed.join(", "));
    }
    failed.is_empty()
}

fn floors_header(fraction: f64) -> String {
    format!(
        "# zrip perf gate floors for this machine. This file is ignored.\n\
         # {:.0}% of the median throughput, less where runs spread more\n\
         # (down to {:.0}%); ratios as measured.\n",
        fraction * 100.0,
        MIN_KEEP * 100.0
    )
}

/// Worker mode: measures one case once and prints `mb_s ratio` (`-` for
/// decode cases) on stdout.
fn worker(name: &str, settings: &Settings) {
    let all = cases();
    let case = all
        .iter()
        .find(|c| format!("{}.{}", c.section, c.key) == name)
        .unwrap_or_else(|| panic!("no case {name}"));
    pin();
    let corpus = match case.data {
        Data::Silesia => corpus(),
        Data::SilesiaText => corpus_text(),
        _ => Vec::new(),
    };
    let dict = matches!(case.op, Op::DictContext | Op::DecompressDict).then(dictionary);
    let inputs = inputs(case, &corpus);
    let m = measure(case, &inputs, dict.as_ref(), settings);
    match m.ratio {
        Some(r) => println!("{} {r}", m.mb_s),
        None => println!("{} -", m.mb_s),
    }
}

/// Measures `case` in `--repeat` fresh worker processes and returns the
/// median and the spread of their throughputs, `(max - min) / median`.
/// A new process per run leaves no heap from earlier cases (some cases
/// move by 10% with it) and puts the spread across processes, as the
/// checks see it.
fn run_case(case: &Case, settings: &Settings) -> (Measured, f64) {
    let exe = std::env::current_exe().expect("current_exe");
    let name = format!("{}.{}", case.section, case.key);
    let mut runs: Vec<Measured> = (0..settings.repeat)
        .map(|_| {
            let output = Command::new(&exe)
                .args(["--worker", &name])
                .stderr(Stdio::inherit())
                .output()
                .expect("cannot run a worker");
            assert!(output.status.success(), "worker failed: {name}");
            let text = String::from_utf8(output.stdout).unwrap();
            let mut fields = text.split_whitespace();
            let mut next = || fields.next().expect("worker output");
            let mb_s = next().parse().unwrap();
            let ratio = match next() {
                "-" => None,
                r => Some(r.parse().unwrap()),
            };
            Measured { mb_s, ratio }
        })
        .collect();
    runs.sort_by(|a, b| a.mb_s.total_cmp(&b.mb_s));
    let spread = (runs[runs.len() - 1].mb_s - runs[0].mb_s) / runs[runs.len() / 2].mb_s;
    (runs.swap_remove(runs.len() / 2), spread)
}

fn main() {
    let options = parse_args();
    let settings = Settings {
        rounds: env_number("ZRIP_PERF_ROUNDS", 7) as usize,
        round_ns: env_number("ZRIP_PERF_ROUND_MS", 50) * 1_000_000,
        repeat: options.repeat,
    };
    if let Some(name) = &options.worker {
        worker(name, &settings);
        return;
    }
    if !options.builds.is_empty() && !options.list {
        let passed = run_builds(&options);
        std::process::exit(i32::from(!passed));
    }
    let all = cases();
    if options.list {
        for case in &all {
            println!("{}.{}", case.section, case.key);
        }
        return;
    }
    // `--case` takes a section (`context_l3`) or one case (`context_l3.2k`).
    let have_corpus = !corpus_paths().is_empty();
    let selected: Vec<&Case> = all
        .iter()
        .filter(|c| !matches!(c.data, Data::Silesia | Data::SilesiaText) || have_corpus)
        .filter(|c| {
            options.cases.is_empty()
                || options
                    .cases
                    .iter()
                    .any(|n| *n == c.section || *n == format!("{}.{}", c.section, c.key))
        })
        .collect();
    assert!(!selected.is_empty(), "no case matches {:?}", options.cases);

    let build = build_name();
    let path = hw_path();
    let (floors, source) = match std::fs::read_to_string(&path) {
        Ok(contents) => (parse_floors(&contents), path.display().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
            SMOKE
                .iter()
                .map(|&(k, v)| (format!("{build}.{k}"), v))
                .collect(),
            "smoke floors (no .perf_hw)".to_string(),
        ),
        Err(e) => panic!("cannot read {}: {e}", path.display()),
    };
    let check = !options.measure_only && options.suggest.is_none();
    let placement = pin();
    eprintln!(
        "build {build}; {placement}; {} rounds of {} ms, best of each, median of {} processes; floors: {}",
        settings.rounds,
        settings.round_ns / 1_000_000,
        settings.repeat,
        if check {
            source.as_str()
        } else {
            "not checked"
        },
    );
    if !have_corpus {
        eprintln!("no Silesia corpus: skipping silesia_* cases");
    }
    if check {
        let unknown: Vec<&String> = floors
            .keys()
            .filter(|k| {
                let known_build = BUILDS
                    .iter()
                    .any(|b| k.starts_with(&format!("{}.", b.name)));
                let known_case = all.iter().any(|c| {
                    BUILDS
                        .iter()
                        .any(|b| k.starts_with(&format!("{}.{}.{}_", b.name, c.section, c.key)))
                });
                !known_build || !known_case
            })
            .collect();
        assert!(unknown.is_empty(), "unknown keys in floors: {unknown:?}");
        if !floors.keys().any(|k| k.starts_with(&format!("{build}."))) {
            eprintln!("no floors for build {build} in {source}");
        }
    }

    let mut failed = Vec::new();
    let mut suggested: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for case in selected {
        let name = format!("{}.{}", case.section, case.key);
        let speed_key = format!("{build}.{name}_mb_s");
        let ratio_key = format!("{build}.{name}_ratio");
        let (mut m, spread) = run_case(case, &settings);
        let below = |m: &Measured| {
            check
                && (floors.get(&speed_key).is_some_and(|&f| m.mb_s < f)
                    || floors
                        .get(&ratio_key)
                        .is_some_and(|&f| m.ratio.is_some_and(|r| r < f)))
        };
        // A miss is measured once more in a new process, keeping the faster
        // run, so one disturbed run does not fail the gate.
        if below(&m) {
            let (again, _) = run_case(case, &settings);
            if again.mb_s > m.mb_s {
                m = again;
            }
        }
        let mut line = format!("{name:<24} {:>10.1} MB/s", m.mb_s);
        if let Some(floor) = floors.get(&speed_key).filter(|_| check) {
            let _ = write!(
                line,
                " (floor {floor:.1}, {:+.1}%)",
                (m.mb_s / floor - 1.0) * 100.0
            );
        }
        if settings.repeat > 1 {
            let _ = write!(line, " spread {:.1}%", spread * 100.0);
        }
        if let Some(ratio) = m.ratio {
            let _ = write!(line, "  ratio {ratio:.3}");
            if let Some(floor) = floors.get(&ratio_key).filter(|_| check) {
                let _ = write!(line, " (floor {floor:.3})");
            }
        }
        if below(&m) {
            line.push_str("  FAIL");
            failed.push(name.clone());
        }
        // With `--suggest`, stdout carries only the floors file.
        if options.suggest.is_some() {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
        if let Some(fraction) = options.suggest {
            // A case whose runs spread widely gets three spreads of room,
            // but keeps at least `MIN_KEEP`.
            let keep = fraction.min(1.0 - 3.0 * spread).max(MIN_KEEP);
            let keys = suggested.entry(case.section).or_default();
            keys.push(format!("{}_mb_s={:.1}", case.key, m.mb_s * keep));
            if let Some(ratio) = m.ratio {
                // Output sizes are deterministic, so ratios need no margin.
                keys.push(format!(
                    "{}_ratio={:.3}",
                    case.key,
                    (ratio * 1000.0).floor() / 1000.0
                ));
            }
        }
    }

    if let Some(fraction) = options.suggest {
        if std::env::var_os("ZRIP_PERF_CHILD").is_none() {
            print!("{}", floors_header(fraction));
        }
        for (section, keys) in suggested {
            println!("\n[{build}.{section}]");
            for key in keys {
                println!("{key}");
            }
        }
    }
    if !failed.is_empty() {
        eprintln!("FAIL: {} below floor: {}", failed.len(), failed.join(", "));
        std::process::exit(1);
    }
}
