//! Sweeps Fast-strategy parameters on the 12 Silesia files and prints, per
//! compressibility group (as in the charts), the geomean encode MB/s, ratio,
//! and decode MB/s. Checksum time is measured once per file and subtracted,
//! as the benchmarks run without checksums.
//!
//! Reads one config per stdin line: `<level> [tl=N] [ss=N] [mm=N] [hl=N]`
//! (target length, search strength, min match, hash log). `CORPUS` sets the
//! corpus dir (default `bench/corpus/silesia`); `SECS` the time per file and
//! direction (default 0.25). `SLICE=<bytes>` instead takes 64 random slices of
//! that size from each file, each its own frame.
use std::fmt::Write;
use std::hint::black_box;
use std::io::BufRead;
use std::time::{Duration, Instant};

const GROUPS: &[(&str, &[&str])] = &[
    ("high", &["nci", "xml", "samba"]),
    (
        "med",
        &[
            "dickens", "mozilla", "mr", "ooffice", "osdb", "reymont", "webster",
        ],
    ),
    ("low", &["sao", "x-ray"]),
];

/// Best time of runs until `secs` have passed (at least 3 runs).
fn best(secs: f64, mut f: impl FnMut()) -> Duration {
    let start = Instant::now();
    let mut best = Duration::MAX;
    let mut runs = 0;
    while runs < 3 || start.elapsed().as_secs_f64() < secs {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed());
        runs += 1;
    }
    best
}

fn gm(v: &[f64]) -> f64 {
    (v.iter().map(|x| x.ln()).sum::<f64>() / v.len() as f64).exp()
}

fn main() {
    let dir = std::env::var("CORPUS").unwrap_or("bench/corpus/silesia".into());
    let secs: f64 = std::env::var("SECS").map_or(0.25, |s| s.parse().unwrap());
    let slice: Option<usize> = std::env::var("SLICE").ok().map(|s| s.parse().unwrap());
    let mut state = 0x1234_5678_9abc_def0u64;
    let files: Vec<(&str, Vec<Vec<u8>>, Duration)> = GROUPS
        .iter()
        .flat_map(|(_, names)| names.iter())
        .map(|&name| {
            let data = std::fs::read(format!("{dir}/{name}")).unwrap();
            let parts = match slice {
                None => vec![data],
                Some(size) => (0..64)
                    .map(|_| {
                        state = state
                            .wrapping_mul(6_364_136_223_846_793_005)
                            .wrapping_add(1);
                        let at = (state >> 17) as usize % (data.len() - size + 1);
                        data[at..at + size].to_vec()
                    })
                    .collect(),
            };
            let xxh = best(0.05, || {
                for p in &parts {
                    black_box(zrip_core::xxhash::xxh64(black_box(p), 0));
                }
            });
            (name, parts, xxh)
        })
        .collect();
    let mut dctx = zrip::DecompressContext::new();

    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let mut words = line.split_whitespace();
        let Some(level) = words.next() else { continue };
        let level: i32 = level.parse().unwrap();
        let mut p = zrip_encode::strategy::level_params(level).unwrap();
        for w in words {
            let (k, v) = w.split_once('=').unwrap();
            let v: u32 = v.parse().unwrap();
            match k {
                "tl" => p.target_length = v,
                "ss" => p.search_strength = v,
                "mm" => p.min_match = v,
                "hl" => {
                    p.hash_log = v;
                    p.chain_log = v;
                }
                _ => panic!("key {k}"),
            }
        }
        let mut out = format!("{line:<28}");
        let mut all = (Vec::new(), Vec::new(), Vec::new());
        for (group, names) in GROUPS {
            let (mut enc, mut ratio, mut dec) = (Vec::new(), Vec::new(), Vec::new());
            for (name, parts, xxh) in files.iter().filter(|f| names.contains(&f.0)) {
                let frames: Vec<Vec<u8>> = parts
                    .iter()
                    .map(|d| {
                        let f = zrip::compress_with_params(d, &p).unwrap();
                        assert_eq!(dctx.decompress(&f).unwrap(), &d[..], "{name}");
                        f
                    })
                    .collect();
                let te = best(secs, || {
                    for d in parts {
                        black_box(zrip::compress_with_params(black_box(d), &p).unwrap());
                    }
                })
                .saturating_sub(*xxh);
                let td = best(secs, || {
                    for f in &frames {
                        black_box(dctx.decompress(black_box(f)).unwrap());
                    }
                })
                .saturating_sub(*xxh);
                let len: usize = parts.iter().map(Vec::len).sum();
                let packed: usize = frames.iter().map(Vec::len).sum();
                enc.push(len as f64 / te.as_secs_f64() / 1e6);
                ratio.push(len as f64 / packed as f64);
                dec.push(len as f64 / td.as_secs_f64() / 1e6);
            }
            write!(
                out,
                " | {group} {:5.0} {:5.3} {:5.0}",
                gm(&enc),
                gm(&ratio),
                gm(&dec)
            )
            .unwrap();
            all.0.extend(enc);
            all.1.extend(ratio);
            all.2.extend(dec);
        }
        write!(
            out,
            " | all {:5.0} {:5.3} {:5.0}",
            gm(&all.0),
            gm(&all.1),
            gm(&all.2)
        )
        .unwrap();
        println!("{out}");
    }
}
