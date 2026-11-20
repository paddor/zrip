//! Local performance gate for zrip's encode and decode paths.
//!
//! Floors come from `.perf_hw` in the repository root, which is ignored:
//! each machine keeps its own. Without it, loose smoke floors apply. See
//! "Performance gate" in `DEVELOPMENT.md`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::PathBuf;

use zrip::push::{EndOp, PushEncoder};
use zrip::{CompressContext, DecompressContext, Dictionary, FrameEncoder};

const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;

/// Inputs per case, so small cases cycle through distinct data.
const CASE_BYTES: usize = MIB;
const MAX_INPUTS: usize = 256;

/// One timed batch runs at least this long, so reading the clock costs well
/// under 1% of it.
const MIN_BATCH_NS: u64 = 100_000;

#[derive(Clone, Copy)]
enum Data {
    /// JSON log lines.
    Text,
    /// Little-endian metric records.
    Binary,
    /// Slices spread over every Silesia file, when the corpus is present.
    Silesia,
}

#[derive(Clone, Copy)]
enum Op {
    /// `zrip::compress`, one call per input.
    Compress,
    /// One reused `CompressContext`.
    Context,
    /// One reused `PushEncoder`, a frame per input.
    PushFrames,
    /// One `PushEncoder` frame per input, fed 16 KiB at a time.
    PushStream,
    /// `FrameEncoder` into a `Vec`, fed 64 KiB at a time.
    Stream,
    /// One reused `CompressContext::with_dict`.
    DictContext,
    /// `zrip::decompress` of the level's frames.
    Decompress,
    /// One reused `DecompressContext`.
    DecompressContext,
    /// One reused `DecompressContext::with_dict`.
    DecompressDict,
}

impl Op {
    const fn encodes(self) -> bool {
        !matches!(
            self,
            Op::Decompress | Op::DecompressContext | Op::DecompressDict
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
/// one-shot calls per message, contexts reused across messages, push
/// streams, large streams, and small dictionary frames.
fn cases() -> Vec<Case> {
    use Data::{Binary, Silesia, Text};
    use Op::*;
    vec![
        case("compress_l1", "1k", Compress, 1, Text, KIB),
        case("compress_l1", "16k", Compress, 1, Text, 16 * KIB),
        case("compress_l1", "256k", Compress, 1, Text, 256 * KIB),
        case("compress_l3", "1k", Compress, 3, Text, KIB),
        case("compress_l3", "16k", Compress, 3, Text, 16 * KIB),
        case("compress_l3", "256k", Compress, 3, Text, 256 * KIB),
        case("compress_l-3", "16k", Compress, -3, Text, 16 * KIB),
        case("compress_l-3", "256k", Compress, -3, Text, 256 * KIB),
        case("context_l1", "2k", Context, 1, Text, 2 * KIB),
        case("context_l1", "64k", Context, 1, Text, 64 * KIB),
        case("context_l1", "64k_bin", Context, 1, Binary, 64 * KIB),
        case("context_l1", "1m_bin", Context, 1, Binary, MIB),
        case("context_l3", "2k", Context, 3, Text, 2 * KIB),
        case("context_l3", "16k", Context, 3, Text, 16 * KIB),
        case("context_l3", "256k", Context, 3, Text, 256 * KIB),
        case("context_l3", "16k_bin", Context, 3, Binary, 16 * KIB),
        case("push_l3", "4k_frames", PushFrames, 3, Text, 4 * KIB),
        case("push_l3", "1m", PushStream, 3, Text, MIB),
        case("stream_l1", "4m", Stream, 1, Text, 4 * MIB),
        case("dict_l3", "64b", DictContext, 3, Text, 64),
        case("dict_l3", "1k", DictContext, 3, Text, KIB),
        case("dict_l1", "1k", DictContext, 1, Text, KIB),
        case("decompress", "1k", Decompress, 1, Text, KIB),
        case("decompress", "16k", Decompress, 1, Text, 16 * KIB),
        case("decompress", "256k", Decompress, 1, Text, 256 * KIB),
        case("dcontext", "2k", DecompressContext, 3, Text, 2 * KIB),
        case("dcontext", "64k", DecompressContext, 3, Text, 64 * KIB),
        case("dcontext", "1m_bin", DecompressContext, 1, Binary, MIB),
        case("dict_decode", "1k", DecompressDict, 3, Text, KIB),
        case("silesia_l1", "1k", Compress, 1, Silesia, KIB),
        case("silesia_l1", "16k_ctx", Context, 1, Silesia, 16 * KIB),
        case("silesia_l1", "256k_ctx", Context, 1, Silesia, 256 * KIB),
        case("silesia_l3", "2k_ctx", Context, 3, Silesia, 2 * KIB),
        case("silesia_l3", "16k_ctx", Context, 3, Silesia, 16 * KIB),
        case("silesia_l3", "64k", Compress, 3, Silesia, 64 * KIB),
        case("silesia_l3", "256k_ctx", Context, 3, Silesia, 256 * KIB),
        case("silesia_l-3", "64k_ctx", Context, -3, Silesia, 64 * KIB),
        case(
            "silesia_decode",
            "16k",
            DecompressContext,
            3,
            Silesia,
            16 * KIB,
        ),
        case(
            "silesia_decode",
            "256k",
            DecompressContext,
            1,
            Silesia,
            256 * KIB,
        ),
    ]
}

/// Loose floors for machines without `.perf_hw`. They catch collapses,
/// such as tables cleared per call, not percent-level regressions.
const SMOKE: &[(&str, f64)] = &[
    ("compress_l1.16k_mb_s", 50.0),
    ("compress_l3.16k_mb_s", 30.0),
    ("context_l1.2k_mb_s", 30.0),
    ("context_l3.2k_mb_s", 20.0),
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

fn inputs(case: &Case, corpus: &[Vec<u8>]) -> Vec<Vec<u8>> {
    if let Data::Silesia = case.data {
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
            Data::Silesia => unreachable!(),
        })
        .collect()
}

/// The Silesia files, from `ZRIP_PERF_CORPUS` or `bench/corpus/silesia`
/// (see "Corpus benchmarks" in `DEVELOPMENT.md`), or none if absent.
fn corpus() -> Vec<Vec<u8>> {
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

/// Times one pass over all inputs and returns throughput in MB/s of
/// uncompressed bytes.
fn measure(case: &Case, inputs: &[Vec<u8>], dict: &Dictionary, settings: &Settings) -> Measured {
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
                    std::hint::black_box(zrip::compress(std::hint::black_box(i), level).unwrap());
                }
            })
        }
        Op::Context | Op::DictContext => {
            let mut ctx = match case.op {
                Op::Context => CompressContext::new(level).unwrap(),
                _ => CompressContext::with_dict(level, dict.clone()).unwrap(),
            };
            out_bytes = inputs.iter().map(|i| ctx.compress(i).unwrap().len()).sum();
            time(settings, || {
                for i in inputs {
                    std::hint::black_box(ctx.compress(std::hint::black_box(i)).unwrap());
                }
            })
        }
        Op::PushFrames | Op::PushStream => {
            let mut enc = PushEncoder::new(level).unwrap();
            let mut out = Vec::new();
            let chunk = match case.op {
                Op::PushFrames => usize::MAX,
                _ => 16 * KIB,
            };
            let mut run = |out: &mut Vec<u8>| {
                for i in inputs {
                    let mut parts = i.chunks(chunk.min(i.len())).peekable();
                    while let Some(part) = parts.next() {
                        let op = if parts.peek().is_some() {
                            EndOp::Continue
                        } else {
                            EndOp::End
                        };
                        enc.encode_to_vec(std::hint::black_box(part), out, op)
                            .unwrap();
                    }
                }
            };
            run(&mut out);
            out_bytes = out.len();
            time(settings, || {
                out.clear();
                run(&mut out);
                std::hint::black_box(&out);
            })
        }
        Op::Stream => {
            let encode = |i: &[u8]| {
                let mut enc = FrameEncoder::new(Vec::new(), level).unwrap();
                for part in i.chunks(64 * KIB) {
                    enc.write_all(std::hint::black_box(part)).unwrap();
                }
                enc.finish().unwrap()
            };
            out_bytes = inputs.iter().map(|i| encode(i).len()).sum();
            time(settings, || {
                for i in inputs {
                    std::hint::black_box(encode(i));
                }
            })
        }
        Op::Decompress => {
            let frames: Vec<Vec<u8>> = inputs
                .iter()
                .map(|i| zrip::compress(i, level).unwrap())
                .collect();
            time(settings, || {
                for f in &frames {
                    std::hint::black_box(zrip::decompress(std::hint::black_box(f)).unwrap());
                }
            })
        }
        Op::DecompressContext | Op::DecompressDict => {
            let (mut enc, mut dec) = match case.op {
                Op::DecompressContext => (
                    CompressContext::new(level).unwrap(),
                    DecompressContext::new(),
                ),
                _ => (
                    CompressContext::with_dict(level, dict.clone()).unwrap(),
                    DecompressContext::with_dict(dict.clone()),
                ),
            };
            let frames: Vec<Vec<u8>> = inputs
                .iter()
                .map(|i| enc.compress(i).unwrap().into_owned())
                .collect();
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
                    std::hint::black_box(dec.decompress(std::hint::black_box(f)).unwrap());
                }
            })
        }
    };
    Measured {
        mb_s: bytes as f64 / ns * 1e3,
        ratio: case.op.encodes().then(|| bytes as f64 / out_bytes as f64),
    }
}

fn measure_median(
    case: &Case,
    inputs: &[Vec<u8>],
    dict: &Dictionary,
    settings: &Settings,
) -> Measured {
    let mut runs: Vec<Measured> = (0..settings.repeat)
        .map(|_| measure(case, inputs, dict, settings))
        .collect();
    runs.sort_by(|a, b| a.mb_s.total_cmp(&b.mb_s));
    runs.swap_remove(runs.len() / 2)
}

/// `.perf_hw`: `[section]` headers and `key=value` lines, `#` comments.
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
    measure_only: bool,
    list: bool,
    suggest: Option<f64>,
    repeat: usize,
}

fn parse_args() -> Options {
    let mut options = Options {
        cases: Vec::new(),
        measure_only: false,
        list: false,
        suggest: None,
        repeat: 1,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--case" => options
                .cases
                .push(args.next().expect("--case needs a name")),
            "--repeat" => {
                options.repeat = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .expect("--repeat needs a count");
                assert!(options.repeat > 0, "--repeat must be positive");
            }
            "--measure-only" => options.measure_only = true,
            "--list" => options.list = true,
            "--suggest" => {
                let fraction: f64 = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .expect("--suggest needs a fraction, such as 0.97");
                assert!(
                    fraction > 0.0 && fraction <= 1.0,
                    "--suggest takes a fraction in (0, 1]"
                );
                options.suggest = Some(fraction);
            }
            _ => panic!("unknown option: {arg} (see DEVELOPMENT.md, \"Performance gate\")"),
        }
    }
    options
}

fn main() {
    let options = parse_args();
    let all = cases();
    if options.list {
        for case in &all {
            println!("{}.{}", case.section, case.key);
        }
        return;
    }
    // `--case` takes a section (`context_l3`) or one case (`context_l3.2k`).
    let corpus = corpus();
    let selected: Vec<&Case> = all
        .iter()
        .filter(|c| !matches!(c.data, Data::Silesia) || !corpus.is_empty())
        .filter(|c| {
            options.cases.is_empty()
                || options
                    .cases
                    .iter()
                    .any(|n| *n == c.section || *n == format!("{}.{}", c.section, c.key))
        })
        .collect();
    assert!(!selected.is_empty(), "no case matches {:?}", options.cases);

    let path = hw_path();
    let (floors, source) = match std::fs::read_to_string(&path) {
        Ok(contents) => (parse_floors(&contents), path.display().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
            SMOKE.iter().map(|&(k, v)| (k.to_string(), v)).collect(),
            "smoke floors (no .perf_hw)".to_string(),
        ),
        Err(e) => panic!("cannot read {}: {e}", path.display()),
    };
    let check = !options.measure_only && options.suggest.is_none();
    let settings = Settings {
        rounds: env_number("ZRIP_PERF_ROUNDS", 7) as usize,
        round_ns: env_number("ZRIP_PERF_ROUND_MS", 50) * 1_000_000,
        repeat: options.repeat,
    };
    let placement = pin();
    eprintln!(
        "placement: {placement}; {} rounds of {} ms, best of each, median of {}; floors: {}",
        settings.rounds,
        settings.round_ns / 1_000_000,
        settings.repeat,
        if check {
            source.as_str()
        } else {
            "not checked"
        },
    );
    if corpus.is_empty() {
        eprintln!("no Silesia corpus: skipping silesia_* cases");
    }
    if check {
        let unknown: Vec<&String> = floors
            .keys()
            .filter(|k| {
                !all.iter()
                    .any(|c| k.starts_with(&format!("{}.{}_", c.section, c.key)))
            })
            .collect();
        assert!(unknown.is_empty(), "unknown keys in floors: {unknown:?}");
    }

    let dict = dictionary();
    let mut failed = Vec::new();
    let mut suggested: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for case in selected {
        let inputs = inputs(case, &corpus);
        let name = format!("{}.{}", case.section, case.key);
        let speed_key = format!("{name}_mb_s");
        let ratio_key = format!("{name}_ratio");
        let mut m = measure_median(case, &inputs, &dict, &settings);
        let below = |m: &Measured| {
            check
                && (floors.get(&speed_key).is_some_and(|&f| m.mb_s < f)
                    || floors
                        .get(&ratio_key)
                        .is_some_and(|&f| m.ratio.is_some_and(|r| r < f)))
        };
        // A miss is measured once more, keeping the faster run, so one
        // disturbed run does not fail the gate.
        if below(&m) {
            let again = measure_median(case, &inputs, &dict, &settings);
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
            let keys = suggested.entry(case.section).or_default();
            keys.push(format!("{}_mb_s={:.1}", case.key, m.mb_s * fraction));
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
        println!(
            "\n# zrip perf gate floors for this machine. This file is ignored.\n# {:.0}% of measured throughput, ratios as measured.",
            fraction * 100.0
        );
        for (section, keys) in suggested {
            println!("\n[{section}]");
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
