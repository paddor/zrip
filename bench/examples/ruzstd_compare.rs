extern crate libc;

use std::io::Write;
use std::path::PathBuf;

// -7 through 4, skipping 0
const LEVELS: &[i32] = &[-7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4];

const FILES: &[&str] = &[
    "corpus/silesia/dickens",
    "corpus/silesia/mozilla",
    "corpus/silesia/mr",
    "corpus/silesia/nci",
    "corpus/silesia/ooffice",
    "corpus/silesia/osdb",
    "corpus/silesia/reymont",
    "corpus/silesia/samba",
    "corpus/silesia/sao",
    "corpus/silesia/webster",
    "corpus/silesia/x-ray",
    "corpus/silesia/xml",
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

fn mb_per_sec(size: usize, ns: f64) -> f64 {
    size as f64 / ns * 1_000.0
}

struct LevelRow {
    level: i32,
    zrip_ratio: f64,
    zrip_enc_mbs: f64,
    zrip_dec_mbs: f64,
    // ruzstd can only encode via Fastest (≈L1); these are Some only at level 1
    ruzstd_ratio: Option<f64>,
    ruzstd_enc_mbs: Option<f64>,
    // ruzstd decodes any valid zstd frame; measured at every level
    ruzstd_dec_mbs: f64,
}

struct FileResult {
    name: String,
    input_size: usize,
    rows: Vec<LevelRow>,
}

fn bench_file(data: &[u8], name: &str, target_ns: u64) -> FileResult {
    let cap = data.len() + 1024;

    // ruzstd encode — Fastest is the only implemented level
    eprintln!("  ruzstd Fastest...");
    let ruzstd_compressed =
        ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest);
    let ruzstd_enc_ns = bench_loop(3, target_ns, 7, || {
        let _ = std::hint::black_box(ruzstd::encoding::compress_to_vec(
            std::hint::black_box(data),
            ruzstd::encoding::CompressionLevel::Fastest,
        ));
    });
    let ruzstd_enc_mbs = mb_per_sec(data.len(), ruzstd_enc_ns);
    let ruzstd_ratio = ruzstd_compressed.len() as f64 / data.len() as f64;

    let mut rows = Vec::new();
    for &level in LEVELS {
        eprintln!("  zrip L{level}...");
        let mut enc_ctx = zrip::CompressContext::new(level).unwrap();
        let compressed = enc_ctx.compress(data).unwrap().to_vec();
        let zrip_enc_ns = bench_loop(3, target_ns, 7, || {
            let _ = std::hint::black_box(enc_ctx.compress(std::hint::black_box(data)).unwrap());
        });

        let mut dec_ctx = zrip::DecompressContext::new();
        let zrip_dec_ns = bench_loop(3, target_ns, 7, || {
            let _ = std::hint::black_box(
                dec_ctx
                    .decompress(std::hint::black_box(&compressed))
                    .unwrap(),
            );
        });

        // ruzstd decodes the zrip-compressed frame (same bitstream, fair throughput comparison)
        let ruzstd_dec_ns = bench_loop(3, target_ns, 7, || {
            let mut dec = ruzstd::decoding::FrameDecoder::new();
            let mut out = Vec::with_capacity(cap);
            dec.decode_all_to_vec(std::hint::black_box(&compressed), &mut out)
                .unwrap();
            std::hint::black_box(&out);
        });

        rows.push(LevelRow {
            level,
            zrip_ratio: compressed.len() as f64 / data.len() as f64,
            zrip_enc_mbs: mb_per_sec(data.len(), zrip_enc_ns),
            zrip_dec_mbs: mb_per_sec(data.len(), zrip_dec_ns),
            ruzstd_ratio: (level == 1).then_some(ruzstd_ratio),
            ruzstd_enc_mbs: (level == 1).then_some(ruzstd_enc_mbs),
            ruzstd_dec_mbs: mb_per_sec(data.len(), ruzstd_dec_ns),
        });
    }

    FileResult {
        name: name.to_string(),
        input_size: data.len(),
        rows,
    }
}

fn print_results(results: &[FileResult]) {
    let out = std::io::stdout();
    let mut out = out.lock();

    for r in results {
        let mb = r.input_size as f64 / 1_048_576.0;
        writeln!(out, "\n=== {} ({:.2} MB) ===", r.name, mb).unwrap();
        writeln!(
            out,
            "{:>6}  {:>10}  {:>12}  {:>14}  {:>16}  {:>14}  {:>16}",
            "level",
            "zrip ratio",
            "ruzstd ratio",
            "zrip enc MB/s",
            "ruzstd enc MB/s",
            "zrip dec MB/s",
            "ruzstd dec MB/s",
        )
        .unwrap();

        for row in &r.rows {
            let ruzstd_ratio_str = match row.ruzstd_ratio {
                Some(v) => format!("{:12.4}", v),
                None => format!("{:>12}", "N/A"),
            };
            let ruzstd_enc_str = match row.ruzstd_enc_mbs {
                Some(v) => format!("{:16.1}", v),
                None => format!("{:>16}", "N/A"),
            };
            writeln!(
                out,
                "{:>6}  {:10.4}  {}  {:14.1}  {}  {:14.1}  {:16.1}",
                row.level,
                row.zrip_ratio,
                ruzstd_ratio_str,
                row.zrip_enc_mbs,
                ruzstd_enc_str,
                row.zrip_dec_mbs,
                row.ruzstd_dec_mbs,
            )
            .unwrap();
        }
    }
}

fn corpus_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn main() {
    let target_ns = 20_000_000u64;
    let mut results = Vec::new();

    for &rel in FILES {
        let name = rel.rsplit('/').next().unwrap();
        let path = corpus_path(rel);
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                eprintln!("skipping {}: not found", path.display());
                continue;
            }
        };
        eprintln!(
            "\n=== {} ({:.2} MB) ===",
            name,
            data.len() as f64 / 1_048_576.0
        );
        results.push(bench_file(&data, name, target_ns));
    }

    print_results(&results);
}
