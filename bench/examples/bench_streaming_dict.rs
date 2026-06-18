use std::io::{Read, Write};
use std::time::Instant;

fn main() {
    let samples: Vec<Vec<u8>> = (0..200)
        .map(|i| {
            format!(
                r#"{{"id":{},"name":"user_{}","email":"user{}@example.com","active":true,"score":{}}}"#,
                i, i, i, i * 17 % 100
            )
            .into_bytes()
        })
        .collect();

    let mut concat = Vec::new();
    let mut sizes = Vec::new();
    for s in &samples {
        concat.extend_from_slice(s);
        sizes.push(s.len());
    }

    let mut dict_buf = vec![0u8; 16384];
    let dict_size =
        zstd_safe::train_from_buffer(&mut dict_buf, &concat, &sizes).expect("training failed");
    dict_buf.truncate(dict_size);
    let dict = zrip::dict::Dictionary::from_bytes(&dict_buf).unwrap();

    // Small data: typical dict use case (individual JSON records ~80 bytes)
    let small_data = &samples[150];
    // Medium data: 10 KiB
    let medium_data: Vec<u8> = samples
        .iter()
        .take(120)
        .flat_map(|s| s.iter().copied())
        .collect();
    // Large data: 200 KiB (multi-block)
    let large_data: Vec<u8> = medium_data.iter().copied().cycle().take(200_000).collect();

    println!("=== Streaming dict encode/decode benchmark ===\n");

    for (label, data) in [
        ("small (~80B)", small_data.as_slice()),
        ("medium (~10KiB)", medium_data.as_slice()),
        ("large (~200KiB)", large_data.as_slice()),
    ] {
        println!("--- {label}, {} bytes ---", data.len());

        for level in [1, 3] {
            let iters = match data.len() {
                0..=200 => 50_000,
                201..=20_000 => 10_000,
                _ => 1_000,
            };

            // One-shot with dict
            let t = Instant::now();
            let mut compressed_oneshot = Vec::new();
            for _ in 0..iters {
                compressed_oneshot = zrip::compress_with_dict(data, level, &dict).unwrap();
            }
            let oneshot_enc_us = t.elapsed().as_nanos() as f64 / iters as f64 / 1000.0;
            let oneshot_ratio = compressed_oneshot.len() as f64 / data.len() as f64;

            let t = Instant::now();
            for _ in 0..iters {
                let _ = zrip::decompress_with_dict(&compressed_oneshot, &dict).unwrap();
            }
            let oneshot_dec_us = t.elapsed().as_nanos() as f64 / iters as f64 / 1000.0;

            // Streaming with dict (fresh encoder each time)
            let t = Instant::now();
            let mut compressed_stream = Vec::new();
            for _ in 0..iters {
                let mut enc =
                    zrip::FrameEncoder::with_dict(Vec::new(), level, dict.clone()).unwrap();
                enc.write_all(data).unwrap();
                compressed_stream = enc.finish().unwrap();
            }
            let stream_enc_us = t.elapsed().as_nanos() as f64 / iters as f64 / 1000.0;
            let stream_ratio = compressed_stream.len() as f64 / data.len() as f64;

            let t = Instant::now();
            for _ in 0..iters {
                let mut dec =
                    zrip::FrameDecoder::with_dict(compressed_stream.as_slice(), dict.clone());
                let mut out = Vec::new();
                dec.read_to_end(&mut out).unwrap();
            }
            let stream_dec_us = t.elapsed().as_nanos() as f64 / iters as f64 / 1000.0;

            // Streaming with dict + reset (reuse encoder across frames)
            let t = Instant::now();
            let mut compressed_reset = Vec::new();
            let mut enc = zrip::FrameEncoder::with_dict(Vec::new(), level, dict.clone()).unwrap();
            for _ in 0..iters {
                enc.write_all(data).unwrap();
                compressed_reset = enc.reset(Vec::new()).unwrap();
            }
            drop(enc);
            let reset_enc_us = t.elapsed().as_nanos() as f64 / iters as f64 / 1000.0;
            let reset_ratio = compressed_reset.len() as f64 / data.len() as f64;

            let t = Instant::now();
            let mut dec = zrip::FrameDecoder::with_dict(compressed_reset.as_slice(), dict.clone());
            let mut dec_out = Vec::new();
            for _ in 0..iters {
                dec_out.clear();
                dec.read_to_end(&mut dec_out).unwrap();
                dec.reset(compressed_reset.as_slice());
            }
            let reset_dec_us = t.elapsed().as_nanos() as f64 / iters as f64 / 1000.0;

            // Streaming without dict (baseline)
            let t = Instant::now();
            let mut compressed_nodict = Vec::new();
            for _ in 0..iters {
                let mut enc = zrip::FrameEncoder::new(Vec::new(), level).unwrap();
                enc.write_all(data).unwrap();
                compressed_nodict = enc.finish().unwrap();
            }
            let nodict_enc_us = t.elapsed().as_nanos() as f64 / iters as f64 / 1000.0;
            let nodict_ratio = compressed_nodict.len() as f64 / data.len() as f64;

            let t = Instant::now();
            for _ in 0..iters {
                let mut dec = zrip::FrameDecoder::new(compressed_nodict.as_slice());
                let mut out = Vec::new();
                dec.read_to_end(&mut out).unwrap();
            }
            let nodict_dec_us = t.elapsed().as_nanos() as f64 / iters as f64 / 1000.0;

            let enc_mb_s = |us: f64| data.len() as f64 / us;
            println!(
                "  L{level:2} one-shot+dict:  enc {:7.1} µs ({:6.1} MB/s)  dec {:7.1} µs ({:6.1} MB/s)  ratio {:.3}",
                oneshot_enc_us,
                enc_mb_s(oneshot_enc_us),
                oneshot_dec_us,
                enc_mb_s(oneshot_dec_us),
                oneshot_ratio
            );
            println!(
                "  L{level:2} stream fresh:  enc {:7.1} µs ({:6.1} MB/s)  dec {:7.1} µs ({:6.1} MB/s)  ratio {:.3}",
                stream_enc_us,
                enc_mb_s(stream_enc_us),
                stream_dec_us,
                enc_mb_s(stream_dec_us),
                stream_ratio
            );
            println!(
                "  L{level:2} stream+reset: enc {:7.1} µs ({:6.1} MB/s)  dec {:7.1} µs ({:6.1} MB/s)  ratio {:.3}",
                reset_enc_us,
                enc_mb_s(reset_enc_us),
                reset_dec_us,
                enc_mb_s(reset_dec_us),
                reset_ratio
            );
            println!(
                "  L{level:2} stream nodict: enc {:7.1} µs ({:6.1} MB/s)  dec {:7.1} µs ({:6.1} MB/s)  ratio {:.3}",
                nodict_enc_us,
                enc_mb_s(nodict_enc_us),
                nodict_dec_us,
                enc_mb_s(nodict_dec_us),
                nodict_ratio
            );
            println!();
        }
    }
}
