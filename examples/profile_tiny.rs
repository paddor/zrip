//! Profiles tiny frames: 64 random slices of one size from every Silesia
//! file, each compressed as its own frame.
//!
//! `zrip_profile_tiny <mode> <level> <size> [seconds]`
//!
//! Modes:
//! - `zenc`, `zdec`, `cenc`, `cdec`: time zrip or C zstd encode or decode,
//!   in ns per frame.
//! - `hbuild`: time the Huffman table build on each slice's histogram.
//! - `hash`: hash all zrip frames (plus full files at size 8192), to check
//!   that an encoder change keeps the output identical.
//! - `lits`: literal section stats of the first block, zrip vs C.
//!
//! `ONLY=dickens,xml` limits the files. `CORPUS` sets the corpus dir
//! (default `bench/corpus/silesia`).
use std::hint::black_box;
use std::time::{Duration, Instant};

const FILES: &[&str] = &[
    "dickens", "mozilla", "mr", "nci", "ooffice", "osdb", "reymont", "samba", "sao", "webster",
    "xml", "x-ray",
];

fn slices(dir: &str, size: usize) -> Vec<Vec<u8>> {
    let mut state = 0x1234_5678_9abc_def0u64;
    let mut out = Vec::new();
    let only = std::env::var("ONLY").ok();
    for name in FILES {
        if only
            .as_deref()
            .is_some_and(|o| !o.split(',').any(|x| x == *name))
        {
            continue;
        }
        let data = std::fs::read(format!("{dir}/{name}")).unwrap();
        for _ in 0..64 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let at = (state >> 17) as usize % (data.len() - size + 1);
            out.push(data[at..at + size].to_vec());
        }
    }
    out
}

fn run(secs: f64, mut pass: impl FnMut()) -> (u64, Duration) {
    for _ in 0..20 {
        pass();
    }
    let start = Instant::now();
    let mut passes = 0;
    while start.elapsed().as_secs_f64() < secs {
        pass();
        passes += 1;
    }
    (passes, start.elapsed())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args[0].as_str();
    let level: i32 = args[1].parse().unwrap();
    let size: usize = args[2].parse().unwrap();
    let secs: f64 = args.get(3).map_or(5.0, |s| s.parse().unwrap());
    let dir = std::env::var("CORPUS").unwrap_or("bench/corpus/silesia".into());
    let inputs = slices(&dir, size);

    if mode == "hash" {
        use std::hash::{DefaultHasher, Hasher};
        let mut h = DefaultHasher::new();
        let mut bytes = 0;
        for s in &inputs {
            let f = zrip::compress(s, level).unwrap();
            bytes += f.len();
            h.write(&f);
        }
        if size == 8192 {
            for name in FILES {
                let data = std::fs::read(format!("{dir}/{name}")).unwrap();
                let f = zrip::compress(&data, level).unwrap();
                bytes += f.len();
                h.write(&f);
            }
        }
        println!("hash L{level} {size}: {:016x} {bytes}", h.finish());
        return;
    }
    let mut zctx = zrip::CompressContext::new(level).unwrap();
    let mut zdctx = zrip::DecompressContext::new();
    let mut cctx = zstd::bulk::Compressor::new(level).unwrap();
    let mut cdctx = zstd::bulk::Decompressor::new().unwrap();
    let zframes: Vec<Vec<u8>> = inputs
        .iter()
        .map(|s| zrip::compress(s, level).unwrap())
        .collect();
    let cframes: Vec<Vec<u8>> = inputs.iter().map(|s| cctx.compress(s).unwrap()).collect();
    if mode == "lits" {
        for (name, frames) in [("zrip", &zframes), ("C", &cframes)] {
            let (mut lits, mut blocks, mut four, mut huf, mut raw, mut csum) = (0, 0, 0, 0, 0, 0);
            for f in frames.iter() {
                let fhd = f[4];
                let single = fhd & 0x20 != 0;
                csum += usize::from(fhd & 4 != 0);
                let mut p = 5 + usize::from(!single) + [0, 1, 2, 4][usize::from(fhd & 3)];
                p += match fhd >> 6 {
                    0 => usize::from(single),
                    1 => 2,
                    2 => 4,
                    _ => 8,
                };
                let bh = u32::from(f[p]) | u32::from(f[p + 1]) << 8 | u32::from(f[p + 2]) << 16;
                p += 3;
                if (bh >> 1) & 3 != 2 {
                    continue;
                }
                blocks += 1;
                let (b0, b1, b2) = (
                    usize::from(f[p]),
                    usize::from(f[p + 1]),
                    usize::from(f[p + 2]),
                );
                let (ty, sf) = (b0 & 3, (b0 >> 2) & 3);
                let n = if ty < 2 {
                    raw += 1;
                    match sf {
                        0 | 2 => b0 >> 3,
                        1 => (b0 >> 4) + (b1 << 4),
                        _ => (b0 >> 4) + (b1 << 4) + (b2 << 12),
                    }
                } else {
                    huf += 1;
                    four += usize::from(sf != 0);
                    match sf {
                        0 | 1 => (b0 >> 4) | ((b1 & 0x3F) << 4),
                        2 => (b0 >> 4) | (b1 << 4) | ((b2 & 3) << 12),
                        _ => (b0 >> 4) | (b1 << 4) | ((b2 & 63) << 12),
                    }
                };
                lits += n;
            }
            println!(
                "{name}: blocks {blocks}, avg lits {:.0}, huffman {huf}, 4-stream {four}, raw {raw}, checksum {csum}",
                lits as f64 / blocks as f64
            );
        }
        return;
    }
    let mut buf = Vec::with_capacity(size + 1024);

    let (passes, elapsed) = match mode {
        "zenc" => run(secs, || {
            for s in &inputs {
                black_box(zctx.compress(black_box(s)).unwrap());
            }
        }),
        "zdec" => run(secs, || {
            for f in &zframes {
                black_box(zdctx.decompress(black_box(f)).unwrap());
            }
        }),
        "cenc" => run(secs, || {
            for s in &inputs {
                black_box(cctx.compress(black_box(s)).unwrap());
            }
        }),
        "cdec" => run(secs, || {
            for f in &cframes {
                buf.clear();
                black_box(cdctx.decompress_to_buffer(black_box(f), &mut buf).unwrap());
            }
        }),
        "hbuild" => {
            use zrip_core::huffman::encode::{HuffmanEncodeTable, byte_histogram};
            let hists: Vec<_> = inputs
                .iter()
                .map(|s| byte_histogram(&s[..s.len() * 2 / 3]))
                .collect();
            let mut logs = [0u32; 13];
            for h in &hists {
                if let Some(t) = HuffmanEncodeTable::from_histogram(h) {
                    logs[t.table_log() as usize] += 1;
                }
            }
            println!("table logs: {logs:?}");
            run(secs, || {
                for h in &hists {
                    black_box(HuffmanEncodeTable::from_histogram(black_box(h)));
                }
            })
        }
        _ => panic!("mode: zenc, zdec, cenc, cdec, hbuild, hash, or lits"),
    };
    let frames = if mode.starts_with('z') {
        &zframes
    } else {
        &cframes
    };
    let ns = elapsed.as_nanos() as f64 / (passes as f64 * inputs.len() as f64);
    let total: usize = inputs.iter().map(Vec::len).sum();
    let packed: usize = frames.iter().map(Vec::len).sum();
    println!(
        "{mode} L{level} {size} B: {ns:.0} ns/frame, ratio {:.3}",
        total as f64 / packed as f64
    );
}
