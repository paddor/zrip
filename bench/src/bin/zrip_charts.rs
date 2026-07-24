#![allow(clippy::too_many_arguments, clippy::type_complexity)]

use plotters::coord::Shift;
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::{Path, PathBuf};

const BG: RGBColor = RGBColor(0x0d, 0x11, 0x17);
const PANEL: RGBColor = RGBColor(0x16, 0x1b, 0x22);
const GRID: RGBColor = RGBColor(0x21, 0x26, 0x2d);
const AXIS: RGBColor = RGBColor(0x30, 0x36, 0x3d);
const TEXT: RGBColor = RGBColor(0xe6, 0xed, 0xf3);
const MUTED: RGBColor = RGBColor(0x7d, 0x85, 0x90);

const FONT_BUMP: u32 = 1;
const HEADER_SUBTITLE_OFFSET: i32 = 18;
const LEGEND_ROW_H: f64 = 20.0;
const LEGEND_COL_W: f64 = 270.0;

const TRANSFER_RATE: f64 = 100e6;
const LEVEL: i32 = 1;
const DECODE_LEVEL: i32 = 3;
const LEVELS: &[i32] = &[3, 1, -1];
const COLUMN_LEVELS: &[i32] = &[-1, 1, 3];
const BAND_LEVELS: &[i32] = &[-8, -7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4];
const C_ZSTD_LEVELS: &[i32] = &[-7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4];
const INTERIOR_LEVELS: &[i32] = &[-7, -6, -5, -4, -3, -2, -1, 1, 2, 3];
const LABEL_LEVEL: i32 = -1;

const MAIN_CORPUS: &[&str] = &[
    "dickens", "mr", "mozilla", "nci", "ooffice", "osdb", "reymont", "samba", "sao", "webster",
    "x-ray", "xml",
];
const HIGH_COMPRESSIBILITY: &[&str] = &["nci", "xml", "samba"];
const MEDIUM_COMPRESSIBILITY: &[&str] = &[
    "dickens", "mozilla", "mr", "ooffice", "osdb", "reymont", "webster",
];
const LOW_COMPRESSIBILITY: &[&str] = &["sao", "x-ray"];
const GROUPS: &[(&str, &[&str])] = &[
    ("High compressibility", HIGH_COMPRESSIBILITY),
    ("Medium compressibility", MEDIUM_COMPRESSIBILITY),
    ("Low compressibility", LOW_COMPRESSIBILITY),
];

const SMALL_PREFIXES: &[&str] = &["dickens", "nci", "xml", "x-ray"];
const SMALL_SUFFIXES: &[&str] = &[
    "_512", "_1k", "_2k", "_4k", "_8k", "_16k", "_32k", "_64k", "_128k", "_256k", "_512k", "_1m",
];
const SMALL_DECODE_SUFFIXES: &[&str] = &[
    "_512", "_1k", "_2k", "_4k", "_8k", "_16k", "_32k", "_64k", "_128k",
];
const SMALL_SIZES: &[usize] = &[
    512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072, 262144, 524288, 1048576,
];
const SMALL_DECODE_SIZES: &[usize] = &[512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072];
const SIZE_LABELS: &[&str] = &[
    "512", "1K", "2K", "4K", "8K", "16K", "32K", "64K", "128K", "256K", "512K", "1M",
];
const SIZE_DECODE_LABELS: &[&str] = &["512", "1K", "2K", "4K", "8K", "16K", "32K", "64K", "128K"];

const SCATTER_LOG_X_MIN: f64 = 1.477; // 10^1.477 ~= 30 MB/s
const SCATTER_LOG_X_MAX: f64 = 4.0; // 10^4 = 10000 MB/s
const SCATTER_X_TICKS: &[f64] = &[
    50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0,
];

#[derive(Clone)]
struct CodecStyle {
    key: &'static str,
    label: &'static str,
    color: RGBColor,
    dim: RGBColor,
}

#[derive(Clone)]
struct Config {
    target: String,
    hw_label: Option<String>,
    codecs: Vec<CodecStyle>,
    scatter_codecs: Vec<&'static str>,
    summary_codecs: Vec<&'static str>,
    matrix_codecs: Vec<&'static str>,
    pipeline_codecs: Vec<&'static str>,
    small_codecs: Vec<&'static str>,
    small_decode_codecs: Vec<&'static str>,
}

#[derive(Deserialize, Clone)]
struct BenchRow {
    input: String,
    level: i32,
    input_size: usize,
    compressed_size: usize,
    compress_ns: f64,
    decompress_ns: f64,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse()?;
    let cfg = Config::new(args.profile.as_deref())?;
    let out_dir = args.output_dir.unwrap_or_else(|| {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("doc");
        p.push("charts");
        p.push(&cfg.target);
        p
    });
    std::fs::create_dir_all(&out_dir)?;

    let charts: Vec<ChartKind> = if args.chart == ChartKind::All {
        vec![
            ChartKind::Scatter,
            ChartKind::Summary,
            ChartKind::Matrix,
            ChartKind::Pipeline,
            ChartKind::SmallEncode,
            ChartKind::SmallDecode,
        ]
    } else {
        vec![args.chart]
    };

    for chart in charts {
        match chart {
            ChartKind::All => unreachable!(),
            ChartKind::Scatter => draw_scatter(&cfg, &out_dir)?,
            ChartKind::Summary => draw_summary(&cfg, &out_dir)?,
            ChartKind::Matrix => draw_matrix(&cfg, &out_dir)?,
            ChartKind::Pipeline => draw_pipeline(&cfg, &out_dir)?,
            ChartKind::SmallEncode => draw_small_encode(&cfg, &out_dir)?,
            ChartKind::SmallDecode => draw_small_decode(&cfg, &out_dir)?,
        }
    }

    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChartKind {
    All,
    Scatter,
    Summary,
    Matrix,
    Pipeline,
    SmallEncode,
    SmallDecode,
}

struct Args {
    chart: ChartKind,
    output_dir: Option<PathBuf>,
    profile: Option<String>,
}

impl Args {
    fn parse() -> Result<Self, Box<dyn Error>> {
        let mut chart = ChartKind::All;
        let mut output_dir = None;
        let mut profile = None;
        let mut args = std::env::args().skip(1).peekable();

        while let Some(arg) = args.next() {
            if arg == "--profile" {
                profile = Some(args.next().ok_or("--profile requires a value")?);
                continue;
            }
            if arg == "-h" || arg == "--help" {
                print_help();
                std::process::exit(0);
            }
            if let Some(kind) = parse_chart_kind(&arg) {
                chart = kind;
            } else if arg == "small" {
                return Err("unknown chart 'small'; use 'small-encode'".into());
            } else {
                output_dir = Some(PathBuf::from(arg));
            }
        }

        Ok(Self {
            chart,
            output_dir,
            profile,
        })
    }
}

fn print_help() {
    println!(
        "Usage: zrip_charts [all|scatter|summary|matrix|pipeline|small-encode|small-decode] [OUT_DIR] [--profile wasm32]"
    );
}

fn parse_chart_kind(s: &str) -> Option<ChartKind> {
    match s {
        "all" => Some(ChartKind::All),
        "scatter" => Some(ChartKind::Scatter),
        "summary" => Some(ChartKind::Summary),
        "matrix" => Some(ChartKind::Matrix),
        "pipeline" => Some(ChartKind::Pipeline),
        "small-encode" => Some(ChartKind::SmallEncode),
        "small-decode" | "small_decode" => Some(ChartKind::SmallDecode),
        _ => None,
    }
}

impl Config {
    fn new(profile: Option<&str>) -> Result<Self, Box<dyn Error>> {
        let mut cfg = Self::default_native();
        if let Some("wasm32") = profile {
            cfg.target = "wasm32".into();
            cfg.hw_label =
                Some("wasmtime (wasm32-wasip1), Linux VM on Intel Core i7-8700B @ 3.20GHz".into());
            cfg.codecs = vec![
                codec("C zstd", "C zstd 1.5.7 (libzstd wasm)", 0x60a5fa, 0x4680c4),
                codec("zrip", "zrip (SIMD128)", 0xf87171, 0xc45050),
                codec(
                    "structured-zstd",
                    "structured-zstd 0.0.49",
                    0xf59e0b,
                    0xc47d08,
                ),
                codec(
                    "zrip paranoid",
                    "zrip paranoid (safe Rust, no unsafe)",
                    0xf472b6,
                    0xc05a92,
                ),
            ];
            cfg.scatter_codecs = vec!["C zstd", "zrip", "structured-zstd", "zrip paranoid"];
            cfg.summary_codecs = cfg.scatter_codecs.clone();
            cfg.matrix_codecs = cfg.scatter_codecs.clone();
            cfg.pipeline_codecs = cfg.scatter_codecs.clone();
            cfg.small_codecs = vec!["C zstd", "zrip", "structured-zstd"];
            cfg.small_decode_codecs = cfg.scatter_codecs.clone();
        } else if profile.is_some() {
            return Err("unknown profile".into());
        }
        Ok(cfg)
    }

    fn default_native() -> Self {
        Self {
            target: std::env::consts::ARCH.into(),
            hw_label: detect_hardware(),
            codecs: vec![
                codec("C zstd", "libzstd v1.5.7 (C)", 0x60a5fa, 0x4680c4),
                codec(
                    "zrip",
                    "zrip (safe SIMD + encapsulated unsafe)",
                    0xf87171,
                    0xc45050,
                ),
                codec(
                    "zrip paranoid",
                    "zrip paranoid (safe SIMD, no unsafe)",
                    0xf472b6,
                    0xc05a92,
                ),
                codec(
                    "structured-zstd",
                    "structured-zstd v0.0.49 (unsafe)",
                    0xf59e0b,
                    0xc47d08,
                ),
                codec("ruzstd", "ruzstd v0.8.3 (safe)", 0x4ade80, 0x3aaf60),
                codec(
                    "lz4rip",
                    "lz4rip 0.8.5 (encapsulated unsafe, Rust, LZ4)",
                    0xc084fc,
                    0x9966cc,
                ),
            ],
            scatter_codecs: vec![
                "C zstd",
                "zrip",
                "structured-zstd",
                "zrip paranoid",
                "ruzstd",
                "lz4rip",
            ],
            summary_codecs: vec![
                "C zstd",
                "zrip",
                "structured-zstd",
                "zrip paranoid",
                "ruzstd",
            ],
            matrix_codecs: vec!["C zstd", "zrip", "structured-zstd", "zrip paranoid"],
            pipeline_codecs: vec![
                "C zstd",
                "zrip",
                "structured-zstd",
                "zrip paranoid",
                "ruzstd",
            ],
            small_codecs: vec!["C zstd", "zrip", "structured-zstd"],
            small_decode_codecs: vec![
                "C zstd",
                "zrip",
                "zrip paranoid",
                "structured-zstd",
                "ruzstd",
            ],
        }
    }

    fn style(&self, key: &str) -> Option<&CodecStyle> {
        self.codecs.iter().find(|c| c.key == key)
    }
}

fn codec(key: &'static str, label: &'static str, color: u32, dim: u32) -> CodecStyle {
    CodecStyle {
        key,
        label,
        color: hex_color(color),
        dim: hex_color(dim),
    }
}

fn hex_color(v: u32) -> RGBColor {
    RGBColor(
        ((v >> 16) & 0xff) as u8,
        ((v >> 8) & 0xff) as u8,
        (v & 0xff) as u8,
    )
}

fn detect_hardware() -> Option<String> {
    let hw_conf = read_chart_hw();
    let mut cpu = std::env::var("ZRIP_CPU").ok();
    if cpu.is_none() && cfg!(target_os = "macos") {
        cpu = std::process::Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    }
    if cpu.is_none() {
        cpu = std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|content| {
                content
                    .lines()
                    .find(|l| l.starts_with("model name"))
                    .and_then(|l| l.split_once(':'))
                    .map(|(_, v)| {
                        v.trim()
                            .replace("(R)", "")
                            .replace("(TM)", "")
                            .replace("CPU ", "")
                    })
            });
    }

    let mut extras = Vec::new();
    if std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
        .is_ok_and(|s| s.trim() == "performance")
    {
        extras.push("performance governor".to_string());
    }
    for (path, off_val) in [
        ("/sys/devices/system/cpu/intel_pstate/no_turbo", "1"),
        ("/sys/devices/system/cpu/cpufreq/boost", "0"),
    ] {
        if let Ok(s) = std::fs::read_to_string(path) {
            if s.trim() == off_val {
                extras.push("turbo off".to_string());
            }
            break;
        }
    }
    if extras.is_empty()
        && let Ok(hw) = std::env::var("ZRIP_HW_EXTRAS")
    {
        extras.extend(
            hw.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
    }
    let postfix = std::env::var("ZRIP_HW_POSTFIX")
        .ok()
        .or_else(|| hw_conf.get("postfix").cloned());
    if let Some(postfix) = postfix {
        for value in postfix
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            if !extras.iter().any(|existing| existing == &value) {
                extras.push(value);
            }
        }
    }

    let prefix = std::env::var("ZRIP_HW_PREFIX")
        .ok()
        .or_else(|| hw_conf.get("prefix").cloned());
    let cores = std::thread::available_parallelism()
        .ok()
        .map(std::num::NonZero::get);

    if let Some(cpu) = &mut cpu
        && let Some(cores) = cores
    {
        cpu.push_str(&format!(", {cores} cores"));
    }

    let mut parts = Vec::new();
    if let Some(prefix) = prefix.filter(|s| !s.trim().is_empty()) {
        parts.push(prefix);
    }

    match (cpu, extras.is_empty()) {
        (Some(mut cpu), false) => {
            cpu.push_str(", ");
            cpu.push_str(&extras.join(", "));
            parts.push(cpu);
        }
        (Some(cpu), true) => parts.push(cpu),
        (None, false) => parts.push(extras.join(", ")),
        (None, true) => {}
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn read_chart_hw() -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for path in [Path::new(".chart_hw"), Path::new("../.chart_hw")] {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        break;
    }
    map
}

fn cache_dir(cfg: &Config) -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
        .join(".cache")
        .join("zrip")
        .join(&cfg.target)
}

fn codec_file(codec: &str) -> String {
    format!("{}.jsonl", codec.replace(' ', "_"))
}

fn load_all_data(
    cfg: &Config,
    codecs: &[&str],
    min_size: usize,
    filter: Option<&[&str]>,
) -> BTreeMap<String, Vec<BenchRow>> {
    let base = cache_dir(cfg);
    let allowed: Option<BTreeSet<&str>> = filter.map(|f| f.iter().copied().collect());
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(base) else {
        return out;
    };
    for entry in entries.flatten() {
        let level_dir = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !level_dir.is_dir() || !name.starts_with('L') {
            continue;
        }
        for &codec in codecs {
            load_rows_from_path(
                &level_dir.join(codec_file(codec)),
                codec,
                min_size,
                allowed.as_ref(),
                &mut out,
            );
        }
    }
    out.into_iter()
        .map(|(codec, rows)| {
            let mut latest = BTreeMap::new();
            for row in rows {
                latest.insert((row.input.clone(), row.level), row);
            }
            (codec, latest.into_values().collect())
        })
        .collect()
}

fn load_level_data(
    cfg: &Config,
    codecs: &[&str],
    level: i32,
    min_size: usize,
    filter: Option<&[&str]>,
) -> BTreeMap<String, Vec<BenchRow>> {
    let base = cache_dir(cfg).join(format!("L{level}"));
    let allowed: Option<BTreeSet<&str>> = filter.map(|f| f.iter().copied().collect());
    let mut out = BTreeMap::new();
    for &codec in codecs {
        load_rows_from_path(
            &base.join(codec_file(codec)),
            codec,
            min_size,
            allowed.as_ref(),
            &mut out,
        );
    }
    out.into_iter()
        .map(|(codec, rows)| {
            let mut latest = BTreeMap::new();
            for row in rows {
                latest.insert(row.input.clone(), row);
            }
            (codec, latest.into_values().collect())
        })
        .collect()
}

fn load_small_data(cfg: &Config, codecs: &[&str]) -> BTreeMap<String, Vec<BenchRow>> {
    let base = cache_dir(cfg);
    let mut out = BTreeMap::new();
    for cache in [base.join("small"), base] {
        let Ok(entries) = std::fs::read_dir(cache) else {
            continue;
        };
        for entry in entries.flatten() {
            let level_dir = entry.path();
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !level_dir.is_dir() || !name.starts_with('L') {
                continue;
            }
            for &codec in codecs {
                load_rows_from_path(&level_dir.join(codec_file(codec)), codec, 0, None, &mut out);
            }
        }
    }
    out.into_iter()
        .map(|(codec, rows)| {
            let mut latest = BTreeMap::new();
            for row in rows {
                if is_small_name(&row.input, SMALL_SUFFIXES) {
                    latest.insert((row.input.clone(), row.level), row);
                }
            }
            (codec, latest.into_values().collect())
        })
        .collect()
}

fn load_small_decode_data(
    cfg: &Config,
    codecs: &[&str],
) -> (BTreeMap<String, Vec<BenchRow>>, bool) {
    let base = cache_dir(cfg);
    let decode_cmp = base.join("small").join("decode_cmp");
    let search_dirs = if decode_cmp.is_dir() {
        vec![decode_cmp]
    } else {
        vec![base.join("small"), base]
    };
    let common_bitstream =
        search_dirs.len() == 1 && search_dirs[0].ends_with(Path::new("decode_cmp"));

    let mut out = BTreeMap::new();
    for dir in search_dirs {
        let level_dir = dir.join(format!("L{DECODE_LEVEL}"));
        if !level_dir.is_dir() {
            continue;
        }
        for &codec in codecs {
            load_rows_from_path(&level_dir.join(codec_file(codec)), codec, 0, None, &mut out);
        }
    }
    let out = out
        .into_iter()
        .map(|(codec, rows)| {
            let mut latest = BTreeMap::new();
            for row in rows {
                if row.level == DECODE_LEVEL && is_small_name(&row.input, SMALL_DECODE_SUFFIXES) {
                    latest.insert(row.input.clone(), row);
                }
            }
            (codec, latest.into_values().collect())
        })
        .collect();
    (out, common_bitstream)
}

fn load_rows_from_path(
    path: &Path,
    codec: &str,
    min_size: usize,
    allowed: Option<&BTreeSet<&str>>,
    out: &mut BTreeMap<String, Vec<BenchRow>>,
) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    for line in content.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Ok(row) = serde_json::from_str::<BenchRow>(line) else {
            continue;
        };
        if row.input_size < min_size {
            continue;
        }
        if let Some(allowed) = allowed
            && !allowed.contains(row.input.as_str())
        {
            continue;
        }
        out.entry(codec.to_string()).or_default().push(row);
    }
}

fn is_small_name(name: &str, suffixes: &[&str]) -> bool {
    SMALL_PREFIXES.iter().any(|prefix| {
        suffixes
            .iter()
            .any(|suffix| name == format!("{prefix}{suffix}"))
    })
}

fn input_names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn small_names(suffixes: &[&str]) -> Vec<String> {
    SMALL_PREFIXES
        .iter()
        .flat_map(|prefix| {
            suffixes
                .iter()
                .map(move |suffix| format!("{prefix}{suffix}"))
        })
        .collect()
}

fn supported_encode_levels(codec: &str) -> Vec<i32> {
    match codec {
        "zrip" | "zrip paranoid" => BAND_LEVELS.to_vec(),
        "C zstd" | "structured-zstd" => C_ZSTD_LEVELS.to_vec(),
        "ruzstd" | "lz4rip" => vec![LEVEL],
        _ => vec![LEVEL],
    }
}

fn supported_chart_levels(codec: &str, levels: &[i32]) -> Vec<i32> {
    let supported = supported_encode_levels(codec);
    levels
        .iter()
        .copied()
        .filter(|level| supported.contains(level))
        .collect()
}

fn require_named_rows<F>(
    data: &BTreeMap<String, Vec<BenchRow>>,
    codecs: &[&str],
    inputs: &[String],
    levels_for_codec: F,
    chart_name: &str,
) -> Result<(), Box<dyn Error>>
where
    F: Fn(&str) -> Vec<i32>,
{
    let mut missing = Vec::new();
    for &codec in codecs {
        let rows = data.get(codec);
        for level in levels_for_codec(codec) {
            for input in inputs {
                let present = rows.is_some_and(|rows| {
                    rows.iter()
                        .any(|row| row.level == level && row.input == *input)
                });
                if !present {
                    missing.push(format!("{codec} L{level} {input}"));
                }
            }
        }
    }
    if missing.is_empty() {
        return Ok(());
    }

    let sample = missing
        .iter()
        .take(12)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "{chart_name}: missing {} required Silesia cache rows ({sample}). {}",
        missing.len(),
        missing_cache_hint(chart_name, &missing),
    )
    .into())
}

fn missing_cache_hint(chart_name: &str, missing: &[String]) -> String {
    const BENCH: &str = "cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release";
    let has_paranoid = missing.iter().any(|row| row.starts_with("zrip paranoid "));
    let only_paranoid = missing.iter().all(|row| row.starts_with("zrip paranoid "));

    match chart_name {
        "small encode" => {
            format!("Run `{BENCH} -- --small-only --impl all`.")
        }
        "small decode" if has_paranoid => {
            format!(
                "Run `{BENCH} -- --small-only --decode-only --levels 3`, then \
                 `{BENCH} --features paranoid -- --small-only --decode-only --levels 3 --impl zrip`."
            )
        }
        "small decode" => {
            format!("Run `{BENCH} -- --small-only --decode-only --levels 3`.")
        }
        _ if only_paranoid => {
            format!("Run `{BENCH} --features paranoid`.")
        }
        _ if has_paranoid => {
            format!("Run `{BENCH} -- --impl all`, then `{BENCH} --features paranoid`.")
        }
        _ => {
            format!("Run `{BENCH} -- --impl all`.")
        }
    }
}

fn enc_mbs(row: &BenchRow) -> Option<f64> {
    (row.compress_ns > 0.0).then(|| row.input_size as f64 / row.compress_ns * 1000.0)
}

fn dec_mbs(row: &BenchRow) -> Option<f64> {
    (row.decompress_ns > 0.0).then(|| row.input_size as f64 / row.decompress_ns * 1000.0)
}

fn ratio(row: &BenchRow) -> f64 {
    row.input_size as f64 / row.compressed_size as f64
}

fn output_path(out_dir: &Path, name: &str) -> PathBuf {
    out_dir.join(name)
}

type Area<'a> = DrawingArea<SVGBackend<'a>, Shift>;

fn root(path: &Path, width: u32, height: u32) -> Result<Area<'_>, Box<dyn Error>> {
    let area = SVGBackend::new(path, (width, height)).into_drawing_area();
    area.fill(&BG)?;
    Ok(area)
}

fn finish_svg(path: &Path, width: u32, height: u32) -> Result<(), Box<dyn Error>> {
    let mut svg = std::fs::read_to_string(path)?;
    svg = svg.replacen(
        &format!("<svg width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\""),
        &format!("<svg viewBox=\"0 0 {width} {height}\""),
        1,
    );
    svg = svg.replacen(
        "xmlns=\"http://www.w3.org/2000/svg\"",
        "xmlns=\"http://www.w3.org/2000/svg\" font-family=\"system-ui, -apple-system, sans-serif\"",
        1,
    );
    std::fs::write(path, svg)?;
    Ok(())
}

fn text(
    area: &Area<'_>,
    s: impl Into<String>,
    x: i32,
    y: i32,
    size: u32,
    color: RGBColor,
    hpos: HPos,
    bold: bool,
) -> Result<(), Box<dyn Error>> {
    let mut font = ("sans-serif", size + FONT_BUMP).into_font();
    if bold {
        font = font.style(FontStyle::Bold);
    }
    let style = TextStyle::from(font)
        .color(&color)
        .pos(Pos::new(hpos, VPos::Center));
    area.draw(&Text::new(s.into(), (x, y), style))?;
    Ok(())
}

fn vtext(
    area: &Area<'_>,
    s: &str,
    x: i32,
    y: i32,
    size: u32,
    color: RGBColor,
) -> Result<(), Box<dyn Error>> {
    let font = ("sans-serif", size + FONT_BUMP)
        .into_font()
        .style(FontStyle::Bold)
        .transform(FontTransform::Rotate270);
    let style = TextStyle::from(font)
        .color(&color)
        .pos(Pos::new(HPos::Center, VPos::Center));
    area.draw(&Text::new(s.to_string(), (x, y), style))?;
    Ok(())
}

fn rect(
    area: &Area<'_>,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    color: RGBColor,
) -> Result<(), Box<dyn Error>> {
    area.draw(&Rectangle::new(
        [(px(x1), px(y1)), (px(x2), px(y2))],
        ShapeStyle::from(&color).filled(),
    ))?;
    Ok(())
}

fn line(
    area: &Area<'_>,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    color: RGBColor,
    width: u32,
) -> Result<(), Box<dyn Error>> {
    area.draw(&PathElement::new(
        vec![(px(x1), px(y1)), (px(x2), px(y2))],
        color.stroke_width(width),
    ))?;
    Ok(())
}

fn polyline(
    area: &Area<'_>,
    points: &[(f64, f64)],
    color: RGBColor,
    width: u32,
    alpha: f64,
    dashed: bool,
) -> Result<(), Box<dyn Error>> {
    if points.len() < 2 {
        return Ok(());
    }
    if dashed {
        for pair in points.windows(2) {
            dashed_line(area, pair[0], pair[1], color, width)?;
        }
    } else {
        area.draw(&PathElement::new(
            points
                .iter()
                .map(|&(x, y)| (px(x), px(y)))
                .collect::<Vec<_>>(),
            color.mix(alpha).stroke_width(width),
        ))?;
    }
    Ok(())
}

fn dashed_line(
    area: &Area<'_>,
    a: (f64, f64),
    b: (f64, f64),
    color: RGBColor,
    width: u32,
) -> Result<(), Box<dyn Error>> {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len = (dx * dx + dy * dy).sqrt();
    if len == 0.0 {
        return Ok(());
    }
    let dash = 5.0;
    let gap = 4.0;
    let mut pos = 0.0;
    while pos < len {
        let end = (pos + dash).min(len);
        let t1 = pos / len;
        let t2 = end / len;
        line(
            area,
            a.0 + dx * t1,
            a.1 + dy * t1,
            a.0 + dx * t2,
            a.1 + dy * t2,
            color,
            width,
        )?;
        pos += dash + gap;
    }
    Ok(())
}

fn dot(area: &Area<'_>, x: f64, y: f64, r: i32, color: RGBColor) -> Result<(), Box<dyn Error>> {
    area.draw(&Circle::new(
        (px(x), px(y)),
        r,
        ShapeStyle::from(&color).filled(),
    ))?;
    Ok(())
}

fn px(v: f64) -> i32 {
    v.round() as i32
}

fn nice_step(max_val: f64, target_lines: usize) -> f64 {
    if max_val <= 0.0 {
        return 1.0;
    }
    let raw = max_val / target_lines as f64;
    let mag = 10.0_f64.powf(raw.max(1e-9).log10().floor());
    for s in [1.0, 2.0, 5.0, 10.0] {
        let step = s * mag;
        if max_val / step <= target_lines as f64 + 1.0 {
            return step;
        }
    }
    mag * 10.0
}

fn log_ticks(min_val: f64, max_val: f64) -> Vec<f64> {
    let lo = min_val.log10().floor() as i32;
    let hi = max_val.log10().ceil() as i32;
    let mut ticks = Vec::new();
    for exp in lo..=hi {
        for mult in [1.0, 2.0, 5.0] {
            let tick = mult * 10.0_f64.powi(exp);
            if min_val <= tick && tick <= max_val {
                ticks.push(tick);
            }
        }
    }
    ticks
}

fn fmt_level(level: i32) -> String {
    format!("L{level}")
}

fn human_size(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1} MB", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0} KB", n as f64 / 1_000.0)
    } else {
        format!("{n} B")
    }
}

fn chart_header(
    area: &Area<'_>,
    width: u32,
    title: &str,
    hw: Option<&str>,
    y: i32,
) -> Result<(), Box<dyn Error>> {
    let mid = (width / 2) as i32;
    text(area, title, mid, y, 14, TEXT, HPos::Center, true)?;
    if let Some(hw) = hw {
        text(
            area,
            hw,
            mid,
            y + HEADER_SUBTITLE_OFFSET,
            10,
            MUTED,
            HPos::Center,
            false,
        )?;
    }
    Ok(())
}

fn draw_legend(
    area: &Area<'_>,
    cfg: &Config,
    items: &[&str],
    x: f64,
    y: f64,
    columns: usize,
) -> Result<(), Box<dyn Error>> {
    let rows = items.len().div_ceil(columns);
    for (i, key) in items.iter().enumerate() {
        let col = i / rows;
        let row = i % rows;
        let Some(style) = cfg.style(key) else {
            continue;
        };
        let lx = x + col as f64 * LEGEND_COL_W;
        let ly = y + row as f64 * LEGEND_ROW_H;
        rect(area, lx, ly - 6.0, lx + 12.0, ly + 6.0, style.color)?;
        text(
            area,
            style.label,
            px(lx + 18.0),
            px(ly),
            10,
            TEXT,
            HPos::Left,
            false,
        )?;
    }
    Ok(())
}

fn marker_item_width(style: &CodecStyle) -> f64 {
    style.label.chars().count() as f64 * 7.0 + 34.0
}

fn draw_marker_legend(
    area: &Area<'_>,
    cfg: &Config,
    items: &[&str],
    mid_x: f64,
    y: f64,
    max_width: f64,
    preferred_columns: usize,
) -> Result<usize, Box<dyn Error>> {
    let styles = items
        .iter()
        .filter_map(|key| cfg.style(key))
        .collect::<Vec<_>>();
    if styles.is_empty() {
        return Ok(0);
    }

    let gap = 22.0;
    let widths = styles.iter().map(|s| marker_item_width(s)).collect::<Vec<_>>();
    let one_row_total =
        widths.iter().sum::<f64>() + styles.len().saturating_sub(1) as f64 * gap;
    if one_row_total <= max_width || styles.len() == 1 {
        let mut x = mid_x - one_row_total / 2.0;
        for (style, item_w) in styles.iter().zip(widths) {
            draw_marker_legend_item(area, style, x, y)?;
            x += item_w + gap;
        }
        return Ok(1);
    }

    let max_columns = preferred_columns.clamp(1, styles.len());
    let mut columns = 1;
    let mut rows = styles.len();
    let mut col_widths = vec![widths.iter().copied().fold(0.0, f64::max)];

    for candidate_columns in (1..=max_columns).rev() {
        let candidate_rows = styles.len().div_ceil(candidate_columns);
        let mut candidate_widths = vec![0.0_f64; candidate_columns];
        for (i, item_w) in widths.iter().enumerate() {
            let col = i % candidate_columns;
            candidate_widths[col] = candidate_widths[col].max(*item_w);
        }
        let total = candidate_widths.iter().sum::<f64>()
            + candidate_columns.saturating_sub(1) as f64 * gap;
        if total <= max_width || candidate_columns == 1 {
            columns = candidate_columns;
            rows = candidate_rows;
            col_widths = candidate_widths;
            break;
        }
    }

    let total_width = col_widths.iter().sum::<f64>() + columns.saturating_sub(1) as f64 * gap;
    let mut col_x = Vec::with_capacity(columns);
    let mut x = mid_x - total_width / 2.0;
    for col_w in &col_widths {
        col_x.push(x);
        x += col_w + gap;
    }

    for (i, style) in styles.iter().enumerate() {
        let col = i % columns;
        let row = i / columns;
        draw_marker_legend_item(area, style, col_x[col], y + row as f64 * LEGEND_ROW_H)?;
    }
    Ok(rows)
}

fn draw_marker_legend_item(
    area: &Area<'_>,
    style: &CodecStyle,
    x: f64,
    y: f64,
) -> Result<(), Box<dyn Error>> {
    rect(area, x, y - 6.0, x + 12.0, y + 6.0, style.color)?;
    text(
        area,
        style.label,
        px(x + 18.0),
        px(y),
        10,
        TEXT,
        HPos::Left,
        false,
    )?;
    Ok(())
}

fn draw_segment_legend(area: &Area<'_>, mid_x: f64, y: f64) -> Result<(), Box<dyn Error>> {
    text(
        area,
        "bright = compress + decompress",
        px(mid_x - 210.0),
        px(y),
        9,
        TEXT,
        HPos::Left,
        false,
    )?;
    text(
        area,
        "dim = transfer @100 MB/s",
        px(mid_x + 30.0),
        px(y),
        9,
        MUTED,
        HPos::Left,
        false,
    )?;
    Ok(())
}

fn draw_line_style_legend(area: &Area<'_>, mid_x: f64, y: f64) -> Result<(), Box<dyn Error>> {
    line(area, mid_x - 180.0, y, mid_x - 160.0, y, MUTED, 2)?;
    text(
        area,
        "solid = fastest",
        px(mid_x - 152.0),
        px(y),
        10,
        MUTED,
        HPos::Left,
        false,
    )?;
    dashed_line(area, (mid_x + 20.0, y), (mid_x + 40.0, y), MUTED, 2)?;
    text(
        area,
        "dashed = slowest",
        px(mid_x + 48.0),
        px(y),
        10,
        MUTED,
        HPos::Left,
        false,
    )?;
    Ok(())
}

fn draw_stack(
    area: &Area<'_>,
    x: f64,
    width: f64,
    p_top: f64,
    p_bot: f64,
    y_max: f64,
    parts: (f64, f64, f64),
    style: &CodecStyle,
) -> Result<(), Box<dyn Error>> {
    let (comp, transfer, decomp) = parts;
    let map_y = |v: f64| p_bot - (v / y_max) * (p_bot - p_top);
    rect(area, x, map_y(comp), x + width, p_bot, style.color)?;
    rect(
        area,
        x,
        map_y(comp + transfer),
        x + width,
        map_y(comp),
        style.dim,
    )?;
    rect(
        area,
        x,
        map_y(comp + transfer + decomp),
        x + width,
        map_y(comp + transfer),
        style.color,
    )?;
    Ok(())
}

fn compute_pipeline(row: &BenchRow) -> (f64, f64, f64) {
    let per_gb = 1e9 / row.input_size as f64;
    (
        row.compress_ns / 1e9 * per_gb,
        (row.compressed_size as f64 / row.input_size as f64) * (1e9 / TRANSFER_RATE),
        row.decompress_ns / 1e9 * per_gb,
    )
}

fn draw_summary(cfg: &Config, out_dir: &Path) -> Result<(), Box<dyn Error>> {
    let data = load_level_data(cfg, &cfg.summary_codecs, LEVEL, 10_000, Some(MAIN_CORPUS));
    let main_inputs = input_names(MAIN_CORPUS);
    require_named_rows(
        &data,
        &cfg.summary_codecs,
        &main_inputs,
        |_| vec![LEVEL],
        "summary",
    )?;
    let mut stacks: BTreeMap<String, (f64, f64, f64)> = BTreeMap::new();
    for key in &cfg.summary_codecs {
        let rows = data.get(*key).cloned().unwrap_or_default();
        let mut comp = Vec::new();
        let mut xfer = Vec::new();
        let mut decomp = Vec::new();
        for row in rows {
            if row.compress_ns <= 0.0 {
                continue;
            }
            let (c, t, d) = compute_pipeline(&row);
            comp.push(c.ln());
            xfer.push(t.ln());
            decomp.push(d.ln());
        }
        if !comp.is_empty() {
            let n = comp.len() as f64;
            stacks.insert(
                (*key).to_string(),
                (
                    (comp.iter().sum::<f64>() / n).exp(),
                    (xfer.iter().sum::<f64>() / n).exp(),
                    (decomp.iter().sum::<f64>() / n).exp(),
                ),
            );
        }
    }

    let codecs = cfg
        .summary_codecs
        .iter()
        .copied()
        .filter(|c| stacks.contains_key(*c))
        .collect::<Vec<_>>();
    if codecs.is_empty() {
        return Ok(());
    }

    let width = 700;
    let leg_rows = codecs.len().div_ceil(2);
    let height = 63 + 250 + 52 + (leg_rows as f64 * LEGEND_ROW_H) as u32 + 58;
    let path = output_path(out_dir, "summary.svg");
    let area = root(&path, width, height)?;
    chart_header(
        &area,
        width,
        "12-file Silesia: Pipeline @100 MB/s geomean, Level 1 (lower is better)",
        cfg.hw_label.as_deref(),
        22,
    )?;

    let x_left = 70.0;
    let x_right = 680.0;
    let plot_w = x_right - x_left;
    let p_top = if cfg.hw_label.is_some() { 63.0 } else { 48.0 };
    let p_bot = p_top + 250.0;
    let y_max = codecs
        .iter()
        .filter_map(|c| stacks.get(*c))
        .map(|(a, b, c)| a + b + c)
        .fold(0.0, f64::max)
        * 1.15;

    vtext(
        &area,
        "seconds / GB",
        22,
        px((p_top + p_bot) / 2.0),
        11,
        TEXT,
    )?;
    draw_y_grid(&area, x_left, x_right, p_top, p_bot, y_max, true)?;

    let bar_w = (plot_w * 0.7 / codecs.len() as f64).min(80.0);
    let gap = bar_w * 0.4;
    let total = codecs.len() as f64 * bar_w + (codecs.len() - 1) as f64 * gap;
    let start = x_left + (plot_w - total) / 2.0;
    for (i, key) in codecs.iter().enumerate() {
        let Some(style) = cfg.style(key) else {
            continue;
        };
        let x = start + i as f64 * (bar_w + gap);
        draw_stack(&area, x, bar_w, p_top, p_bot, y_max, stacks[*key], style)?;
        text(
            &area,
            *key,
            px(x + bar_w / 2.0),
            px(p_bot + 16.0),
            10,
            TEXT,
            HPos::Center,
            true,
        )?;
    }
    let mid = width as f64 / 2.0;
    let leg_y = p_bot + 52.0;
    draw_legend(&area, cfg, &codecs, mid - 235.0, leg_y, 2)?;
    draw_segment_legend(&area, mid, leg_y + leg_rows as f64 * LEGEND_ROW_H + 18.0)?;
    area.present()?;
    drop(area);
    finish_svg(&path, width, height)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn draw_pipeline(cfg: &Config, out_dir: &Path) -> Result<(), Box<dyn Error>> {
    let data = load_level_data(
        cfg,
        &cfg.pipeline_codecs,
        LEVEL,
        1_000_000,
        Some(MAIN_CORPUS),
    );
    let main_inputs = input_names(MAIN_CORPUS);
    require_named_rows(
        &data,
        &cfg.pipeline_codecs,
        &main_inputs,
        |_| vec![LEVEL],
        "pipeline",
    )?;
    let codecs = cfg
        .pipeline_codecs
        .iter()
        .copied()
        .filter(|c| data.contains_key(*c))
        .collect::<Vec<_>>();
    if codecs.is_empty() {
        return Ok(());
    }

    let mut input_order = BTreeSet::new();
    let mut input_sizes = BTreeMap::new();
    let mut stacks = BTreeMap::new();
    for (codec, rows) in &data {
        for row in rows {
            input_order.insert(row.input.clone());
            input_sizes.insert(row.input.clone(), row.input_size);
            stacks.insert((row.input.clone(), codec.clone()), compute_pipeline(row));
        }
    }
    let inputs = input_order.into_iter().collect::<Vec<_>>();
    let mid = inputs.len().div_ceil(2);
    let panels = [&inputs[..mid], &inputs[mid..]];

    let width = 850;
    let x_left = 55.0;
    let x_right = 830.0;
    let plot_w = x_right - x_left;
    let panel_h = 240.0;
    let panel_gap = 70.0;
    let top = if cfg.hw_label.is_some() { 62.0 } else { 43.0 };
    let panel_tops = [top, top + panel_h + panel_gap];
    let height = (panel_tops[1] + panel_h + 175.0) as u32;
    let path = output_path(out_dir, "pipeline.svg");
    let area = root(&path, width, height)?;
    chart_header(
        &area,
        width,
        "12-file Silesia: Per-file pipeline @100 MB/s, Level 1 (lower is better)",
        cfg.hw_label.as_deref(),
        22,
    )?;

    let y_max = stacks
        .values()
        .map(|(a, b, c)| a + b + c)
        .fold(0.0, f64::max)
        * 1.1;
    vtext(
        &area,
        "seconds / GB",
        22,
        px((panel_tops[0] + panel_tops[1] + panel_h) / 2.0),
        11,
        TEXT,
    )?;

    for (pi, panel_inputs) in panels.iter().enumerate() {
        if panel_inputs.is_empty() {
            continue;
        }
        let p_top = panel_tops[pi];
        let p_bot = p_top + panel_h;
        draw_y_grid(&area, x_left, x_right, p_top, p_bot, y_max, false)?;

        let group_w = plot_w / panel_inputs.len() as f64;
        let bar_w = group_w * 0.75 / codecs.len() as f64;
        let gap = group_w * 0.25;
        for (gi, input) in panel_inputs.iter().enumerate() {
            let group_x = x_left + gi as f64 * group_w + gap / 2.0;
            for (ci, codec) in codecs.iter().enumerate() {
                let Some(parts) = stacks.get(&(input.to_string(), (*codec).to_string())) else {
                    continue;
                };
                let Some(style) = cfg.style(codec) else {
                    continue;
                };
                draw_stack(
                    &area,
                    group_x + ci as f64 * bar_w,
                    bar_w,
                    p_top,
                    p_bot,
                    y_max,
                    *parts,
                    style,
                )?;
            }
            let cx = group_x + codecs.len() as f64 * bar_w / 2.0;
            text(
                &area,
                input,
                px(cx),
                px(p_bot + 17.0),
                10,
                TEXT,
                HPos::Center,
                true,
            )?;
            text(
                &area,
                human_size(*input_sizes.get(input).unwrap_or(&0)),
                px(cx),
                px(p_bot + 32.0),
                9,
                MUTED,
                HPos::Center,
                false,
            )?;
        }
    }

    let leg_y = panel_tops[1] + panel_h + 62.0;
    draw_legend(&area, cfg, &codecs, width as f64 / 2.0 - 235.0, leg_y, 2)?;
    let rows = codecs.len().div_ceil(2);
    draw_segment_legend(
        &area,
        width as f64 / 2.0,
        leg_y + rows as f64 * LEGEND_ROW_H + 16.0,
    )?;
    area.present()?;
    drop(area);
    finish_svg(&path, width, height)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn draw_y_grid(
    area: &Area<'_>,
    x_left: f64,
    x_right: f64,
    p_top: f64,
    p_bot: f64,
    y_max: f64,
    one_decimal: bool,
) -> Result<(), Box<dyn Error>> {
    let map_y = |v: f64| p_bot - (v / y_max) * (p_bot - p_top);
    let step = nice_step(y_max, 5);
    let mut v = step;
    while v <= y_max {
        let yy = map_y(v);
        line(area, x_left, yy, x_right, yy, GRID, 1)?;
        let label = if one_decimal {
            format!("{v:.1}")
        } else {
            format!("{v:.0}")
        };
        text(
            area,
            label,
            px(x_left - 8.0),
            px(yy),
            10,
            MUTED,
            HPos::Right,
            false,
        )?;
        v += step;
    }
    line(area, x_left, p_bot, x_right, p_bot, AXIS, 2)?;
    Ok(())
}

fn draw_matrix(cfg: &Config, out_dir: &Path) -> Result<(), Box<dyn Error>> {
    let data = load_all_data(cfg, &cfg.matrix_codecs, 10_000, Some(MAIN_CORPUS));
    let main_inputs = input_names(MAIN_CORPUS);
    require_named_rows(
        &data,
        &cfg.matrix_codecs,
        &main_inputs,
        |codec| supported_chart_levels(codec, LEVELS),
        "matrix",
    )?;
    let stacks = compute_matrix_stacks(&data, &cfg.matrix_codecs);
    let width = 850;
    let x_left = 70.0;
    let x_right = 830.0;
    let plot_w = x_right - x_left;
    let top = if cfg.hw_label.is_some() { 62.0 } else { 43.0 };
    let panel_h = 200.0;
    let panel_gap = 55.0;
    let label_h = 22.0;
    let mut panel_tops = Vec::new();
    let mut y = top;
    for _ in GROUPS {
        y += label_h;
        panel_tops.push(y);
        y += panel_h + panel_gap;
    }
    let height = (y - panel_gap + 130.0) as u32;
    let path = output_path(out_dir, "matrix.svg");
    let area = root(&path, width, height)?;
    chart_header(
        &area,
        width,
        "12-file Silesia: Pipeline @100 MB/s by compressibility (lower is better)",
        cfg.hw_label.as_deref(),
        22,
    )?;
    vtext(
        &area,
        "seconds / GB",
        22,
        px((panel_tops[0] + panel_tops[2] + panel_h) / 2.0),
        11,
        TEXT,
    )?;

    for (pi, (group, _)) in GROUPS.iter().enumerate() {
        let p_top = panel_tops[pi];
        let p_bot = p_top + panel_h;
        text(
            &area,
            *group,
            px((x_left + x_right) / 2.0),
            px(p_top - 10.0),
            12,
            TEXT,
            HPos::Center,
            true,
        )?;
        let mut y_max: f64 = 0.0;
        for level in LEVELS {
            for codec in &cfg.matrix_codecs {
                if let Some((a, b, c)) =
                    stacks.get(&((*codec).to_string(), *level, (*group).to_string()))
                {
                    y_max = y_max.max(a + b + c);
                }
            }
        }
        let y_max = y_max * 1.15;
        if y_max == 0.0 {
            continue;
        }
        draw_y_grid(&area, x_left, x_right, p_top, p_bot, y_max, true)?;

        let col_w = plot_w / COLUMN_LEVELS.len() as f64;
        let max_codecs = COLUMN_LEVELS
            .iter()
            .map(|level| {
                cfg.matrix_codecs
                    .iter()
                    .copied()
                    .filter(|codec| {
                        stacks.contains_key(&((*codec).to_string(), *level, (*group).to_string()))
                    })
                    .count()
            })
            .max()
            .unwrap_or(1)
            .max(1);
        let bar_w = (col_w * 0.7 / max_codecs as f64).min(50.0);
        let inner_gap = bar_w * 0.15;
        for (li, level) in COLUMN_LEVELS.iter().enumerate() {
            let col_x = x_left + li as f64 * col_w + (col_w * 0.2) / 2.0;
            let codecs = cfg
                .matrix_codecs
                .iter()
                .copied()
                .filter(|codec| {
                    stacks.contains_key(&((*codec).to_string(), *level, (*group).to_string()))
                })
                .collect::<Vec<_>>();
            for (ci, codec) in codecs.iter().enumerate() {
                let Some(parts) = stacks.get(&((*codec).to_string(), *level, (*group).to_string()))
                else {
                    continue;
                };
                let Some(style) = cfg.style(codec) else {
                    continue;
                };
                draw_stack(
                    &area,
                    col_x + ci as f64 * (bar_w + inner_gap / max_codecs as f64),
                    bar_w,
                    p_top,
                    p_bot,
                    y_max,
                    *parts,
                    style,
                )?;
            }
            let label = match *level {
                -1 => "Level -1",
                1 => "Level 1",
                3 => "Level 3",
                _ => "",
            };
            text(
                &area,
                label,
                px(x_left + li as f64 * col_w + col_w / 2.0),
                px(p_bot + 18.0),
                11,
                TEXT,
                HPos::Center,
                true,
            )?;
        }
    }
    let leg_y = panel_tops[2] + panel_h + 48.0;
    draw_legend(
        &area,
        cfg,
        &cfg.matrix_codecs,
        width as f64 / 2.0 - 235.0,
        leg_y,
        2,
    )?;
    let rows = cfg.matrix_codecs.len().div_ceil(2);
    draw_segment_legend(
        &area,
        width as f64 / 2.0,
        leg_y + rows as f64 * LEGEND_ROW_H + 16.0,
    )?;
    area.present()?;
    drop(area);
    finish_svg(&path, width, height)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn compute_matrix_stacks(
    data: &BTreeMap<String, Vec<BenchRow>>,
    codecs: &[&str],
) -> BTreeMap<(String, i32, String), (f64, f64, f64)> {
    let mut out = BTreeMap::new();
    for codec in codecs {
        let rows = data.get(*codec).cloned().unwrap_or_default();
        for level in LEVELS {
            for (group, files) in GROUPS {
                let mut total_input = 0usize;
                let mut total_compressed = 0usize;
                let mut total_comp = 0.0;
                let mut total_decomp = 0.0;
                for row in rows
                    .iter()
                    .filter(|r| r.level == *level && files.contains(&r.input.as_str()))
                {
                    if row.compress_ns <= 0.0 {
                        continue;
                    }
                    total_input += row.input_size;
                    total_compressed += row.compressed_size;
                    total_comp += row.compress_ns;
                    total_decomp += row.decompress_ns;
                }
                if total_input > 0 {
                    let per_gb = 1e9 / total_input as f64;
                    out.insert(
                        ((*codec).to_string(), *level, (*group).to_string()),
                        (
                            total_comp / 1e9 * per_gb,
                            total_compressed as f64 / total_input as f64 * (1e9 / TRANSFER_RATE),
                            total_decomp / 1e9 * per_gb,
                        ),
                    );
                }
            }
        }
    }
    out
}

fn draw_scatter(cfg: &Config, out_dir: &Path) -> Result<(), Box<dyn Error>> {
    let data = load_all_data(cfg, &cfg.scatter_codecs, 10_000, Some(MAIN_CORPUS));
    let main_inputs = input_names(MAIN_CORPUS);
    require_named_rows(
        &data,
        &cfg.scatter_codecs,
        &main_inputs,
        supported_encode_levels,
        "scatter",
    )?;
    let points = compute_scatter_points(&data, &cfg.scatter_codecs);
    if points.is_empty() {
        return Ok(());
    }

    let width = 850;
    let top = if cfg.hw_label.is_some() { 66.0 } else { 48.0 };
    let panel_h = 250.0;
    let gap = 85.0;
    let bottom = 120.0;
    let height = (top + panel_h * 3.0 + gap * 2.0 + bottom) as u32;
    let path = output_path(out_dir, "scatter.svg");
    let area = root(&path, width, height)?;
    chart_header(
        &area,
        width,
        "12-file Silesia: Encode Speed vs Compression Ratio",
        cfg.hw_label.as_deref(),
        18,
    )?;

    let x_left = 70.0;
    let x_right = 830.0;
    for (i, (group, _)) in GROUPS.iter().enumerate() {
        let p_top = top + i as f64 * (panel_h + gap);
        let p_bot = p_top + panel_h;
        let (y_lo, y_hi) = match *group {
            "High compressibility" => (2.5, 8.0),
            "Medium compressibility" => (1.4, 3.2),
            _ => (0.95, 1.55),
        };
        draw_scatter_panel(
            &area, cfg, &points, group, x_left, x_right, p_top, p_bot, y_lo, y_hi,
        )?;
    }
    vtext(
        &area,
        "compression ratio",
        16,
        px((top + top + 2.0 * (panel_h + gap) + panel_h) / 2.0),
        11,
        TEXT,
    )?;
    draw_marker_legend(
        &area,
        cfg,
        &cfg.scatter_codecs
            .iter()
            .copied()
            .filter(|c| data.contains_key(*c))
            .collect::<Vec<_>>(),
        width as f64 / 2.0,
        top + panel_h * 3.0 + gap * 2.0 + 55.0,
        width as f64 - 60.0,
        3,
    )?;
    area.present()?;
    drop(area);
    finish_svg(&path, width, height)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn compute_scatter_points(
    data: &BTreeMap<String, Vec<BenchRow>>,
    codecs: &[&str],
) -> BTreeMap<(String, i32, String), (f64, f64)> {
    let mut points = BTreeMap::new();
    for codec in codecs {
        let Some(rows) = data.get(*codec) else {
            continue;
        };
        let levels = rows.iter().map(|r| r.level).collect::<BTreeSet<_>>();
        for level in levels {
            if *codec == "ruzstd" && level != 1 {
                continue;
            }
            for (group, files) in GROUPS {
                let mut enc_logs = Vec::new();
                let mut ratio_logs = Vec::new();
                for row in rows
                    .iter()
                    .filter(|r| r.level == level && files.contains(&r.input.as_str()))
                {
                    if let Some(mbs) = enc_mbs(row) {
                        enc_logs.push(mbs.ln());
                        ratio_logs.push(ratio(row).ln());
                    }
                }
                if !enc_logs.is_empty() {
                    let n = enc_logs.len() as f64;
                    points.insert(
                        ((*codec).to_string(), level, (*group).to_string()),
                        (
                            (enc_logs.iter().sum::<f64>() / n).exp(),
                            (ratio_logs.iter().sum::<f64>() / n).exp(),
                        ),
                    );
                }
            }
        }
    }
    points
}

fn should_label_scatter_level(codec: &str, level: i32) -> bool {
    matches!(level, -1 | 1 | 3)
        || match codec {
            "zrip" | "zrip paranoid" => level == -8,
            _ => level == -7,
        }
}

#[allow(clippy::too_many_arguments)]
fn draw_scatter_panel(
    area: &Area<'_>,
    cfg: &Config,
    points: &BTreeMap<(String, i32, String), (f64, f64)>,
    group: &str,
    x_left: f64,
    x_right: f64,
    p_top: f64,
    p_bot: f64,
    y_lo: f64,
    y_hi: f64,
) -> Result<(), Box<dyn Error>> {
    rect(area, x_left, p_top, x_right, p_bot, PANEL)?;
    text(
        area,
        group,
        px((x_left + x_right) / 2.0),
        px(p_top - 13.0),
        12,
        TEXT,
        HPos::Center,
        true,
    )?;
    let map_x =
        |mbs: f64| {
            x_left
                + (mbs.log10() - SCATTER_LOG_X_MIN)
                    / (SCATTER_LOG_X_MAX - SCATTER_LOG_X_MIN)
                    * (x_right - x_left)
        };
    let clip_x = |x: f64| x.clamp(x_left, x_right);
    let map_y = |r: f64| p_bot - (r - y_lo) / (y_hi - y_lo) * (p_bot - p_top);

    for &tick in SCATTER_X_TICKS {
        let x = map_x(tick);
        if x_left < x && x <= x_right {
            line(area, x, p_top, x, p_bot, GRID, 1)?;
            text(
                area,
                format!("{tick:.0}"),
                px(x),
                px(p_bot + 16.0),
                9,
                MUTED,
                HPos::Center,
                false,
            )?;
        }
    }
    let step = nice_step(y_hi - y_lo, 5);
    let mut v = (y_lo / step).ceil() * step;
    while v <= y_hi {
        let y = map_y(v);
        line(area, x_left, y, x_right, y, GRID, 1)?;
        text(
            area,
            format!("{v:.2}x"),
            px(x_left - 8.0),
            px(y),
            10,
            MUTED,
            HPos::Right,
            false,
        )?;
        v += step;
    }
    line(area, x_left, p_bot, x_right, p_bot, AXIS, 2)?;

    for codec in &cfg.scatter_codecs {
        let Some(style) = cfg.style(codec) else {
            continue;
        };
        let mut pts = points
            .iter()
            .filter_map(|((c, level, g), &(x, y))| {
                (c == codec && g == group).then_some((*level, x, y))
            })
            .collect::<Vec<_>>();
        pts.sort_by_key(|(level, _, _)| *level);
        let mapped = pts
            .iter()
            .map(|(_, x, y)| (clip_x(map_x(*x)), map_y(*y)))
            .collect::<Vec<_>>();
        polyline(area, &mapped, style.color, 1, 1.0, false)?;
        for (level, x, y) in pts {
            let raw_sx = map_x(x);
            let sx = clip_x(raw_sx);
            let sy = map_y(y);
            dot(area, sx, sy, 3, style.color)?;
            if should_label_scatter_level(codec, level) {
                let (label_x, hpos) = if raw_sx > x_right - 28.0 {
                    (sx - 6.0, HPos::Right)
                } else {
                    (sx + 6.0, HPos::Left)
                };
                text(
                    area,
                    fmt_level(level),
                    px(label_x),
                    px(sy),
                    8,
                    style.color,
                    hpos,
                    true,
                )?;
            }
        }
    }
    text(
        area,
        "encode MB/s (log)",
        px((x_left + x_right) / 2.0),
        px(p_bot + 34.0),
        10,
        MUTED,
        HPos::Center,
        false,
    )?;
    Ok(())
}

fn draw_small_encode(cfg: &Config, out_dir: &Path) -> Result<(), Box<dyn Error>> {
    let data = load_small_data(cfg, &cfg.small_codecs);
    let small_inputs = small_names(SMALL_SUFFIXES);
    require_named_rows(
        &data,
        &cfg.small_codecs,
        &small_inputs,
        supported_encode_levels,
        "small encode",
    )?;
    let width = 830;
    let panel_w = 700.0;
    let panel_h = 220.0;
    let top = if cfg.hw_label.is_some() { 66.0 } else { 48.0 };
    let left = 90.0;
    let gap = 50.0;
    let total_h = SMALL_PREFIXES.len() as f64 * panel_h + (SMALL_PREFIXES.len() - 1) as f64 * gap;
    let height = (top + total_h + 150.0) as u32;
    let path = output_path(out_dir, "small_encode.svg");
    let area = root(&path, width, height)?;
    chart_header(
        &area,
        width,
        "Encode Throughput vs Input Size (Silesia small-input slices)",
        cfg.hw_label.as_deref(),
        18,
    )?;

    for (pi, prefix) in SMALL_PREFIXES.iter().enumerate() {
        let p_top = top + pi as f64 * (panel_h + gap);
        let p_bot = p_top + panel_h;
        let x_left = left;
        let x_right = left + panel_w;
        let mut panel_min = f64::INFINITY;
        let mut panel_max: f64 = 0.0;
        for codec in &cfg.small_codecs {
            let rows = data.get(*codec).cloned().unwrap_or_default();
            for level in BAND_LEVELS {
                for suffix in SMALL_SUFFIXES {
                    if let Some(v) = get_mbs(&rows, &format!("{prefix}{suffix}"), *level) {
                        panel_min = panel_min.min(v);
                        panel_max = panel_max.max(v);
                    }
                }
            }
        }
        if !panel_min.is_finite() || panel_max <= 0.0 {
            continue;
        }
        let y_min = panel_min / 1.15;
        let y_max = panel_max * 1.15;
        draw_small_panel_frame(
            &area, prefix, x_left, x_right, p_top, p_bot, y_min, y_max, true,
        )?;
        let map_x = |size: usize| {
            x_left
                + ((size as f64).log10() - 400.0_f64.log10())
                    / (1_200_000.0_f64.log10() - 400.0_f64.log10())
                    * (x_right - x_left)
        };
        let map_y = |mbs: f64| {
            p_bot
                - (mbs.log10() - y_min.log10()) / (y_max.log10() - y_min.log10()) * (p_bot - p_top)
        };
        draw_small_x_grid(
            &area,
            x_left,
            x_right,
            p_top,
            p_bot,
            map_x,
            SMALL_SIZES,
            SIZE_LABELS,
        )?;
        for tick in log_ticks(y_min, y_max) {
            let y = map_y(tick);
            if p_top + 5.0 < y && y < p_bot - 5.0 {
                line(&area, x_left, y, x_right, y, GRID, 1)?;
                let label = format!("{tick:.0}");
                text(
                    &area,
                    label,
                    px(x_left - 8.0),
                    px(y),
                    9,
                    MUTED,
                    HPos::Right,
                    false,
                )?;
            }
        }
        for codec in &cfg.small_codecs {
            let rows = data.get(*codec).cloned().unwrap_or_default();
            if rows.is_empty() {
                continue;
            }
            let Some(style) = cfg.style(codec) else {
                continue;
            };
            let codec_levels = BAND_LEVELS
                .iter()
                .copied()
                .filter(|level| {
                    SMALL_SUFFIXES.iter().any(|suffix| {
                        get_mbs(&rows, &format!("{prefix}{suffix}"), *level).is_some()
                    })
                })
                .collect::<Vec<_>>();
            if codec_levels.is_empty() {
                continue;
            }
            for level in INTERIOR_LEVELS.iter().filter(|l| codec_levels.contains(l)) {
                let pts = small_points(
                    &rows,
                    prefix,
                    *level,
                    SMALL_SUFFIXES,
                    SMALL_SIZES,
                    map_x,
                    map_y,
                );
                polyline(&area, &pts, style.color, 1, 0.35, false)?;
                if *level == LABEL_LEVEL
                    && let Some(&(x, y)) = pts.last()
                {
                    text(
                        &area,
                        fmt_level(*level),
                        px(x + 6.0),
                        px(y),
                        7,
                        style.color,
                        HPos::Left,
                        true,
                    )?;
                }
            }
            let (lo, hi) = band_envelope(&rows, prefix, &codec_levels, map_x, map_y);
            polyline(&area, &hi, style.color, 2, 1.0, false)?;
            polyline(&area, &lo, style.color, 2, 1.0, true)?;
            for (pts, label) in [
                (&hi, fmt_level(*codec_levels.first().unwrap())),
                (&lo, fmt_level(*codec_levels.last().unwrap())),
            ] {
                for &(x, y) in pts {
                    dot(&area, x, y, 3, style.color)?;
                }
                if let Some(&(x, y)) = pts.last() {
                    text(
                        &area,
                        label,
                        px(x + 6.0),
                        px(y),
                        7,
                        style.color,
                        HPos::Left,
                        true,
                    )?;
                }
            }
        }
    }
    vtext(
        &area,
        "encode MB/s (log scale)",
        20,
        px(top + total_h / 2.0),
        11,
        TEXT,
    )?;
    let y = top + total_h + 55.0;
    let legend_rows = draw_marker_legend(
        &area,
        cfg,
        &cfg.small_codecs,
        width as f64 / 2.0,
        y,
        width as f64 - 50.0,
        3,
    )?;
    draw_line_style_legend(
        &area,
        width as f64 / 2.0,
        y + legend_rows as f64 * LEGEND_ROW_H + 18.0,
    )?;
    area.present()?;
    drop(area);
    finish_svg(&path, width, height)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn draw_small_decode(cfg: &Config, out_dir: &Path) -> Result<(), Box<dyn Error>> {
    let (data, common) = load_small_decode_data(cfg, &cfg.small_decode_codecs);
    let small_inputs = small_names(SMALL_DECODE_SUFFIXES);
    require_named_rows(
        &data,
        &cfg.small_decode_codecs,
        &small_inputs,
        |_| vec![DECODE_LEVEL],
        "small decode",
    )?;
    let width = 830;
    let panel_w = 700.0;
    let panel_h = 220.0;
    let top = if cfg.hw_label.is_some() { 66.0 } else { 48.0 };
    let left = 90.0;
    let gap = 50.0;
    let total_h = SMALL_PREFIXES.len() as f64 * panel_h + (SMALL_PREFIXES.len() - 1) as f64 * gap;
    let height = (top + total_h + 125.0) as u32;
    let path = output_path(out_dir, "small_decode.svg");
    let area = root(&path, width, height)?;
    let subtitle = if common {
        format!("C zstd L{DECODE_LEVEL} bitstream")
    } else {
        format!("L{DECODE_LEVEL}")
    };
    chart_header(
        &area,
        width,
        &format!("Decode Throughput vs Input Size (Silesia slices, {subtitle})"),
        cfg.hw_label.as_deref(),
        18,
    )?;
    for (pi, prefix) in SMALL_PREFIXES.iter().enumerate() {
        let p_top = top + pi as f64 * (panel_h + gap);
        let p_bot = p_top + panel_h;
        let x_left = left;
        let x_right = left + panel_w;
        let mut panel_max: f64 = 0.0;
        for codec in &cfg.small_decode_codecs {
            let rows = data.get(*codec).cloned().unwrap_or_default();
            for suffix in SMALL_DECODE_SUFFIXES {
                if let Some(v) = get_decode_mbs(&rows, &format!("{prefix}{suffix}")) {
                    panel_max = panel_max.max(v);
                }
            }
        }
        if panel_max <= 0.0 {
            continue;
        }
        let y_max = panel_max * 1.15;
        draw_small_panel_frame(
            &area, prefix, x_left, x_right, p_top, p_bot, 0.0, y_max, false,
        )?;
        let map_x = |size: usize| {
            x_left
                + ((size as f64).log10() - 400.0_f64.log10())
                    / (200_000.0_f64.log10() - 400.0_f64.log10())
                    * (x_right - x_left)
        };
        let map_y = |mbs: f64| p_bot - (mbs / y_max) * (p_bot - p_top);
        draw_small_x_grid(
            &area,
            x_left,
            x_right,
            p_top,
            p_bot,
            map_x,
            SMALL_DECODE_SIZES,
            SIZE_DECODE_LABELS,
        )?;
        let step = nice_step(y_max, 5);
        let mut v = step;
        while v < y_max {
            let y = map_y(v);
            line(&area, x_left, y, x_right, y, GRID, 1)?;
            text(
                &area,
                format!("{v:.0}"),
                px(x_left - 8.0),
                px(y),
                9,
                MUTED,
                HPos::Right,
                false,
            )?;
            v += step;
        }
        for codec in &cfg.small_decode_codecs {
            let rows = data.get(*codec).cloned().unwrap_or_default();
            if rows.is_empty() {
                continue;
            }
            let Some(style) = cfg.style(codec) else {
                continue;
            };
            let pts = SMALL_DECODE_SUFFIXES
                .iter()
                .enumerate()
                .filter_map(|(i, suffix)| {
                    get_decode_mbs(&rows, &format!("{prefix}{suffix}"))
                        .map(|mbs| (map_x(SMALL_DECODE_SIZES[i]), map_y(mbs)))
                })
                .collect::<Vec<_>>();
            polyline(&area, &pts, style.color, 2, 1.0, false)?;
            for (x, y) in pts {
                dot(&area, x, y, 3, style.color)?;
            }
        }
    }
    vtext(&area, "decode MB/s", 20, px(top + total_h / 2.0), 11, TEXT)?;
    draw_marker_legend(
        &area,
        cfg,
        &cfg.small_decode_codecs
            .iter()
            .copied()
            .filter(|c| data.contains_key(*c))
            .collect::<Vec<_>>(),
        width as f64 / 2.0,
        top + total_h + 55.0,
        width as f64 - 50.0,
        3,
    )?;
    area.present()?;
    drop(area);
    finish_svg(&path, width, height)?;
    println!("wrote {}", path.display());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_small_panel_frame(
    area: &Area<'_>,
    prefix: &str,
    x_left: f64,
    x_right: f64,
    p_top: f64,
    p_bot: f64,
    _y_min: f64,
    _y_max: f64,
    _log_y: bool,
) -> Result<(), Box<dyn Error>> {
    rect(area, x_left, p_top, x_right, p_bot, PANEL)?;
    text(
        area,
        prefix.replace('_', " "),
        px((x_left + x_right) / 2.0),
        px(p_top - 12.0),
        12,
        TEXT,
        HPos::Center,
        true,
    )?;
    line(area, x_left, p_bot, x_right, p_bot, AXIS, 2)?;
    Ok(())
}

fn draw_small_x_grid(
    area: &Area<'_>,
    x_left: f64,
    x_right: f64,
    p_top: f64,
    p_bot: f64,
    map_x: impl Fn(usize) -> f64,
    sizes: &[usize],
    labels: &[&str],
) -> Result<(), Box<dyn Error>> {
    for (size, label) in sizes.iter().zip(labels) {
        let x = map_x(*size);
        if x_left + 5.0 < x && x < x_right - 5.0 {
            line(area, x, p_top, x, p_bot, GRID, 1)?;
            text(
                area,
                *label,
                px(x),
                px(p_bot + 16.0),
                9,
                MUTED,
                HPos::Center,
                false,
            )?;
        }
    }
    Ok(())
}

fn get_mbs(rows: &[BenchRow], name: &str, level: i32) -> Option<f64> {
    rows.iter()
        .find(|r| r.input == name && r.level == level)
        .and_then(enc_mbs)
}

fn get_decode_mbs(rows: &[BenchRow], name: &str) -> Option<f64> {
    rows.iter().find(|r| r.input == name).and_then(dec_mbs)
}

fn small_points(
    rows: &[BenchRow],
    prefix: &str,
    level: i32,
    suffixes: &[&str],
    sizes: &[usize],
    map_x: impl Fn(usize) -> f64,
    map_y: impl Fn(f64) -> f64,
) -> Vec<(f64, f64)> {
    suffixes
        .iter()
        .enumerate()
        .filter_map(|(i, suffix)| {
            get_mbs(rows, &format!("{prefix}{suffix}"), level)
                .map(|mbs| (map_x(sizes[i]), map_y(mbs)))
        })
        .collect()
}

fn band_envelope(
    rows: &[BenchRow],
    prefix: &str,
    levels: &[i32],
    map_x: impl Fn(usize) -> f64 + Copy,
    map_y: impl Fn(f64) -> f64 + Copy,
) -> (Vec<(f64, f64)>, Vec<(f64, f64)>) {
    let mut lo = Vec::new();
    let mut hi = Vec::new();
    for (i, suffix) in SMALL_SUFFIXES.iter().enumerate() {
        let name = format!("{prefix}{suffix}");
        let vals = levels
            .iter()
            .filter_map(|level| get_mbs(rows, &name, *level))
            .collect::<Vec<_>>();
        if vals.is_empty() {
            continue;
        }
        let min = vals.iter().copied().fold(f64::INFINITY, f64::min);
        let max = vals.iter().copied().fold(0.0, f64::max);
        lo.push((map_x(SMALL_SIZES[i]), map_y(min)));
        hi.push((map_x(SMALL_SIZES[i]), map_y(max)));
    }
    (lo, hi)
}
