use std::env;
use std::io::Read;
use std::time::Instant;

const LEVELS: &[i32] = &[-7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4];

fn main() {
    let args: Vec<String> = env::args().collect();

    let mut label = String::from("stdin");
    let mut level_filter: Vec<i32> = Vec::new();
    let mut json_output = false;
    let mut codec_filter: Vec<String> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--label" => {
                i += 1;
                label = args[i].clone();
            }
            "--levels" => {
                i += 1;
                level_filter.extend(
                    args[i]
                        .split(',')
                        .filter_map(|s| s.trim().parse::<i32>().ok()),
                );
            }
            "--json" => json_output = true,
            "--impl" => {
                i += 1;
                codec_filter.extend(args[i].split(',').map(|s| s.to_string()));
            }
            _ => {
                eprintln!("unknown arg: {}", args[i]);
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let mut raw = Vec::new();
    std::io::stdin()
        .read_to_end(&mut raw)
        .expect("failed to read stdin");

    if raw.is_empty() {
        eprintln!("no data on stdin");
        std::process::exit(1);
    }

    let levels: &[i32] = if level_filter.is_empty() {
        LEVELS
    } else {
        &level_filter
    };

    let run_all = codec_filter.is_empty() || codec_filter.iter().any(|s| s == "all");
    let want = |name: &str| run_all || codec_filter.iter().any(|s| s == name);

    eprintln!("{} ({} bytes)", label, raw.len());

    for &level in levels {
        if want("zrip") {
            bench_codec("zrip", &label, &raw, level, json_output, |data, lvl| {
                zrip::compress(data, lvl).expect("zrip compress failed")
            }, |compressed| {
                zrip::decompress(compressed).expect("zrip decompress failed")
            });
        }

        #[cfg(feature = "structured-zstd")]
        if want("structured-zstd") {
            bench_codec("structured-zstd", &label, &raw, level, json_output, |data, lvl| {
                use structured_zstd::encoding::CompressionLevel;
                structured_zstd::encoding::compress_slice_to_vec(data, CompressionLevel::Level(lvl))
            }, |compressed| {
                let mut dec = structured_zstd::decoding::FrameDecoder::new();
                let mut out = vec![0u8; raw.len() + 1024];
                dec.decode_all(compressed, &mut out).expect("structured-zstd decompress failed");
                out
            });
        }

        #[cfg(feature = "ruzstd")]
        if want("ruzstd") && level == 1 {
            bench_codec("ruzstd", &label, &raw, level, json_output, |data, _lvl| {
                ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest)
            }, |compressed| {
                let mut dec = ruzstd::decoding::FrameDecoder::new();
                let mut out = vec![0u8; raw.len() + 1024];
                dec.decode_all(compressed, &mut out).expect("ruzstd decompress failed");
                out
            });
        }

        #[cfg(feature = "czstd")]
        if want("C zstd") {
            bench_codec("C zstd", &label, &raw, level, json_output, |data, lvl| {
                zstd::encode_all(data, lvl).expect("C zstd compress failed")
            }, |compressed| {
                zstd::decode_all(compressed).expect("C zstd decompress failed")
            });
        }
    }
}

fn bench_codec<E, D>(
    codec: &str,
    label: &str,
    raw: &[u8],
    level: i32,
    json_output: bool,
    encode: E,
    decode: D,
)
where
    E: Fn(&[u8], i32) -> Vec<u8>,
    D: Fn(&[u8]) -> Vec<u8>,
{
    let compressed = encode(raw, level);

    let enc_iters = calibrate(|| { let _ = encode(raw, level); });
    let start = Instant::now();
    for _ in 0..enc_iters {
        let _ = encode(raw, level);
    }
    let compress_ns = start.elapsed().as_nanos() as f64 / enc_iters as f64;

    let dec_iters = calibrate(|| { let _ = decode(&compressed); });
    let start = Instant::now();
    for _ in 0..dec_iters {
        let _ = decode(&compressed);
    }
    let decompress_ns = start.elapsed().as_nanos() as f64 / dec_iters as f64;

    if json_output {
        println!(
            r#"{{"codec": "{}", "input": "{}", "level": {}, "input_size": {}, "compressed_size": {}, "compress_ns": {:.1}, "decompress_ns": {:.1}}}"#,
            codec, label, level, raw.len(), compressed.len(), compress_ns, decompress_ns,
        );
    }

    let ratio = raw.len() as f64 / compressed.len() as f64;
    let enc_mbs = raw.len() as f64 / compress_ns * 1000.0;
    let dec_mbs = raw.len() as f64 / decompress_ns * 1000.0;
    eprintln!(
        "  L{level:>3} {label:<16} {codec:>16} {enc_mbs:>5.0} enc {dec_mbs:>5.0} dec {ratio:.2}x",
    );
}

fn calibrate(mut f: impl FnMut()) -> u32 {
    let target_ns: u64 = 500_000_000;
    let start = Instant::now();
    f();
    let single_ns = start.elapsed().as_nanos() as u64;
    ((target_ns / single_ns.max(1)) as u32).clamp(4, 10000)
}
