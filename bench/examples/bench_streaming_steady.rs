//! Steady-state streaming decode throughput with large windows.
//!
//! Input is synthetic Zipf-like word text by default (many short matches,
//! skewed literals). `--corpus FILE` repeats FILE to fill the stream. Use a file
//! at least as large as the largest window, or LDM will trivially match the repeats.
//!
//! Usage: cargo run --release --example zrip_bench_streaming_steady -- \
//!            [--corpus FILE] [--windows 21,23]   # log2 window sizes; default 21,23,25,27

use std::io::{Read, Write};
use std::time::Instant;

const WRITE_CHUNK: usize = 1 << 20;
const READ_SIZE: usize = 64 << 10;
const MIN_STREAM: usize = 256 << 20;
const MIN_WINDOWS: usize = 4;
const RUNS: usize = 7;
const LEVELS: &[i32] = &[1, 3];
const WINDOW_LOGS: &[u32] = &[21, 23, 25, 27];

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn unit_f64(r: u64) -> f64 {
    (r >> 11) as f64 / (1u64 << 53) as f64
}

/// Zipf-like word salad: a skewed pick from a fixed vocabulary gives many
/// short matches and non-uniform literals, unlike repeated noise.
fn synthetic_text(len: usize) -> Vec<u8> {
    const LETTERS: &[u8] = b"etaoinshrdlucmfwypvbgkjqxz";
    let mut rng = 1u64;
    let vocab: Vec<Vec<u8>> = (0..8192)
        .map(|_| {
            let n = 2 + (splitmix(&mut rng) % 9) as usize;
            (0..n)
                .map(|_| {
                    let u = unit_f64(splitmix(&mut rng));
                    LETTERS[(u * u * LETTERS.len() as f64) as usize]
                })
                .collect()
        })
        .collect();

    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        let r = splitmix(&mut rng);
        let u = unit_f64(r);
        out.extend_from_slice(&vocab[(u * u * u * vocab.len() as f64) as usize]);
        out.push(match r & 31 {
            0 => b'\n',
            1 => b',',
            _ => b' ',
        });
    }
    out.truncate(len);
    out
}

fn make_stream(window: usize, corpus: Option<&[u8]>) -> Vec<u8> {
    let len = MIN_STREAM.max(window * MIN_WINDOWS);
    match corpus {
        Some(file) => file.iter().cycle().take(len).copied().collect(),
        None => synthetic_text(len),
    }
}

fn encode(data: &[u8], window_log: u32, level: i32) -> Vec<u8> {
    let opts = zrip::Options::default().window_log(window_log).ldm(true);
    let mut enc = zrip::FrameEncoder::with_options(Vec::new(), level, &opts).unwrap();
    for chunk in data.chunks(WRITE_CHUNK) {
        enc.write_all(chunk).unwrap();
    }
    enc.finish().unwrap()
}

/// One pass over a stream; returns steady-state MB/s, timed from the first
/// read that completes at or past the window boundary to the end of the stream.
fn timed_pass(mut reader: impl Read, window: usize, len: usize) -> f64 {
    let mut buf = vec![0u8; READ_SIZE];
    let mut pos = 0usize;
    let mut steady_start = None;

    loop {
        let n = reader.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        pos += n;
        if steady_start.is_none() && pos >= window {
            steady_start = Some((Instant::now(), pos));
        }
    }

    let end = Instant::now();
    assert_eq!(pos, len);

    let (start, start_pos) = steady_start.expect("stream shorter than window");
    let secs = end.duration_since(start).as_secs_f64().max(1e-9);

    (len - start_pos) as f64 / secs / 1e6
}

fn median(mut run: impl FnMut() -> f64) -> f64 {
    let mut v: Vec<f64> = (0..RUNS).map(|_| run()).collect();
    v.sort_by(f64::total_cmp);
    v[RUNS / 2]
}

fn zrip_pass(compressed: &[u8], window: usize, len: usize) -> f64 {
    timed_pass(zrip::FrameDecoder::new(compressed), window, len)
}

fn parse_list<T: std::str::FromStr>(s: &str) -> Vec<T> {
    s.split(',')
        .map(|v| v.parse().unwrap_or_else(|_| panic!("bad value: {v}")))
        .collect()
}

fn size_label(bytes: usize) -> String {
    if bytes >= 1 << 20 {
        format!("{}MiB", bytes >> 20)
    } else {
        format!("{}KiB", bytes >> 10)
    }
}

fn main() {
    let mut corpus_path = None;
    let mut window_logs = WINDOW_LOGS.to_vec();
    let mut args = std::env::args().skip(1);

    while let Some(flag) = args.next() {
        let value = args
            .next()
            .unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--corpus" => corpus_path = Some(value),
            "--windows" => window_logs = parse_list(&value),
            _ => panic!("unknown flag {flag}"),
        }
    }
    let corpus = corpus_path.as_ref().map(|p| std::fs::read(p).unwrap());

    println!("=== Streaming decode: large window, long stream ===");
    println!(
        "data: {}; stream >= {} MiB and >= {MIN_WINDOWS} windows; median of {RUNS} passes",
        corpus_path.as_deref().unwrap_or("synthetic text"),
        MIN_STREAM >> 20,
    );
    println!(
        "steady state, measured after the first window; {} reads\n",
        size_label(READ_SIZE),
    );
    println!("{:>6} {:>7} {:>3} {:>5}", "window", "stream", "lvl", "MB/s");

    for &window_log in &window_logs {
        let window = 1usize << window_log;
        let data = make_stream(window, corpus.as_deref());

        for &level in LEVELS {
            let compressed = encode(&data, window_log, level);
            let mb_s = median(|| zrip_pass(&compressed, window, data.len()));

            println!(
                "{:>6} {:>7} {:>3} {:>5.0}",
                size_label(window),
                size_label(data.len()),
                level,
                mb_s,
            );
        }
    }
}
