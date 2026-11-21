// zrip's decoders against C zstd's, on valid frames, mutated frames, and
// hand-built edge cases: both must accept the same inputs, with the same
// output. `ZRIP_DIFF_MUTATIONS` sets the mutations per seed frame.
#![cfg(feature = "std")]

#[path = "differential/check.rs"]
mod check;

use zrip_core::bitstream::writer::BitWriter;

const MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Disagreements of one test, reported together at its end.
#[derive(Default)]
struct Failures(Vec<String>);

impl Failures {
    fn check(&mut self, input: &[u8], dict: Option<&[u8]>, what: &str) {
        if let Err(e) = check::check(input, dict) {
            let head = &input[..input.len().min(48)];
            if std::env::var_os("ZRIP_DIFF_LOG").is_some() {
                eprintln!("DIFF {what}: {e} | {head:02x?}");
                if let Some(dir) = std::env::var_os("ZRIP_DIFF_DUMP") {
                    let name = what.replace(|c: char| !c.is_ascii_alphanumeric(), "_");
                    let _ = std::fs::write(std::path::Path::new(&dir).join(name), input);
                }
            }
            self.0.push(format!(
                "{what}: {e}\n  input ({} bytes): {head:02x?}",
                input.len()
            ));
        }
    }
}

impl Drop for Failures {
    fn drop(&mut self) {
        if !self.0.is_empty() && !std::thread::panicking() {
            let shown: Vec<&str> = self.0.iter().take(40).map(String::as_str).collect();
            panic!(
                "{} disagreements with C zstd:\n{}",
                self.0.len(),
                shown.join("\n")
            );
        }
    }
}

fn text(len: usize, seed: u64) -> Vec<u8> {
    const WORDS: [&str; 12] = [
        "the ", "quick ", "brown ", "fox ", "jumps ", "over ", "lazy ", "dog ", "zstd ", "frame ",
        "block ", "literal ",
    ];
    let mut rng = Rng(seed | 1);
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        out.extend_from_slice(WORDS[rng.below(WORDS.len())].as_bytes());
    }
    out.truncate(len);
    out
}

fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng(seed | 1);
    (0..len).map(|_| rng.next() as u8).collect()
}

/// Mostly text with random runs: compressed, raw and RLE blocks, Huffman and
/// raw literals.
fn mixed(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng(seed | 1);
    let mut out = Vec::with_capacity(len + 512);
    while out.len() < len {
        match rng.below(4) {
            0 => out.extend(noise(1 + rng.below(300), rng.next())),
            1 => out.extend(std::iter::repeat_n(rng.next() as u8, 1 + rng.below(400))),
            _ => out.extend(text(1 + rng.below(600), rng.next())),
        }
    }
    out.truncate(len);
    out
}

fn inputs() -> Vec<Vec<u8>> {
    let mut v = vec![Vec::new(), vec![7], b"abc".to_vec()];
    for (i, len) in [17usize, 100, 1000, 5000, 20_000].into_iter().enumerate() {
        v.push(text(len, i as u64));
        v.push(mixed(len, 100 + i as u64));
    }
    v.push(noise(3000, 9));
    v.push(vec![0; 3000]);
    v
}

fn c_frame(input: &[u8], level: i32, checksum: bool, size: bool, dict: Option<&[u8]>) -> Vec<u8> {
    use zstd::zstd_safe::CParameter;
    let mut c = match dict {
        Some(d) => zstd::bulk::Compressor::with_dictionary(level, d).unwrap(),
        None => zstd::bulk::Compressor::new(level).unwrap(),
    };
    c.set_parameter(CParameter::ChecksumFlag(checksum)).unwrap();
    c.set_parameter(CParameter::ContentSizeFlag(size)).unwrap();
    c.compress(input).unwrap()
}

fn train_dict() -> Vec<u8> {
    let samples: Vec<Vec<u8>> = (0..300).map(|i| text(400, 1000 + i)).collect();
    zstd::dict::from_samples(&samples, 8192).unwrap()
}

/// Valid frames from C zstd and zrip, with and without a dictionary.
fn seed_frames(dict: &[u8]) -> Vec<(Vec<u8>, bool)> {
    let zdict = zrip::Dictionary::from_bytes(dict).unwrap();
    let mut frames = Vec::new();
    for (i, input) in inputs().iter().enumerate() {
        for level in [-5, 1, 3, 9, 19] {
            frames.push((c_frame(input, level, i % 2 == 0, i % 3 != 0, None), false));
        }
        frames.push((c_frame(input, 3, true, true, Some(dict)), true));
        for level in [-7, -3, 1, 2, 4] {
            frames.push((zrip::compress(input, level).unwrap(), false));
        }
        frames.push((zrip::compress_with_dict(input, 1, &zdict).unwrap(), true));
    }
    // Several frames and a skippable one in a row.
    let mut multi = c_frame(b"first", 1, true, true, None);
    multi.extend_from_slice(&[0x50, 0x2A, 0x4D, 0x18, 3, 0, 0, 0, 1, 2, 3]);
    multi.extend(zrip::compress(&text(700, 5), 1).unwrap());
    frames.push((multi, false));
    frames
}

fn mutate(frame: &[u8], other: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut m = frame.to_vec();
    if m.is_empty() {
        return m;
    }
    for _ in 0..=rng.below(3) {
        let len = m.len();
        // Headers hold most of the structure: favor the first bytes.
        let at = if rng.below(2) == 0 {
            rng.below(len.min(24))
        } else {
            rng.below(len)
        };
        match rng.below(8) {
            0 => m[at] ^= 1 << rng.below(8),
            1 => m[at] = rng.next() as u8,
            2 => m[at] = [0, 0xFF, 0x80, 0x7F, 1][rng.below(5)],
            3 => m.truncate(at),
            4 => m.insert(at, rng.next() as u8),
            5 => {
                m.remove(at);
            }
            6 if !other.is_empty() => {
                let from = rng.below(other.len());
                let n = rng.below(64).min(other.len() - from).min(len - at);
                m[at..at + n].copy_from_slice(&other[from..from + n]);
            }
            _ => {
                let n = rng.below(16).min(len - at);
                let v = rng.next() as u8;
                m[at..at + n].fill(v);
            }
        }
        if m.is_empty() {
            break;
        }
    }
    m
}

#[test]
fn valid_frames_agree() {
    let mut failures = Failures::default();
    let dict = train_dict();
    for (i, (frame, with_dict)) in seed_frames(&dict).iter().enumerate() {
        failures.check(frame, with_dict.then_some(&dict[..]), &format!("seed {i}"));
        if !with_dict {
            // A frame naming no dictionary decodes with one, too.
            failures.check(frame, Some(&dict), &format!("seed {i} with dict"));
        }
    }
}

#[test]
fn mutated_frames_agree() {
    let mut failures = Failures::default();
    let per_seed: usize = std::env::var("ZRIP_DIFF_MUTATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let dict = train_dict();
    let seeds = seed_frames(&dict);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for (i, (frame, with_dict)) in seeds.iter().enumerate() {
        let other = &seeds[(i * 7 + 3) % seeds.len()].0;
        for j in 0..per_seed {
            let m = mutate(frame, other, &mut rng);
            let d = with_dict.then_some(&dict[..]);
            failures.check(&m, d, &format!("seed {i} mutation {j}"));
        }
    }
}

// ===== Hand-built frames =====

/// A frame of `blocks`: magic, a frame header descriptor `fhd` and the
/// header fields after it, then the blocks.
fn frame(fhd: u8, fields: &[u8], blocks: &[u8]) -> Vec<u8> {
    let mut f = MAGIC.to_vec();
    f.push(fhd);
    f.extend_from_slice(fields);
    f.extend_from_slice(blocks);
    f
}

fn block(last: bool, kind: u32, size: u32, body: &[u8]) -> Vec<u8> {
    let h = (size << 3) | (kind << 1) | u32::from(last);
    let mut b = vec![h as u8, (h >> 8) as u8, (h >> 16) as u8];
    b.extend_from_slice(body);
    b
}

/// A frame of one compressed block, content size `size`, with a 128 KiB
/// window. A single-segment frame's window, and so its block limit, would
/// be the content size, smaller than many of the blocks.
fn compressed_frame(size: usize, body: &[u8]) -> Vec<u8> {
    let b = block(true, 2, body.len() as u32, body);
    let mut fields = vec![7 << 3];
    fields.extend_from_slice(&(size as u32).to_le_bytes());
    frame(0x80, &fields, &b)
}

/// A Huffman literals section for `lits` with code lengths `bits` (symbol
/// `s` has `bits[s]` bits), in one or four streams, followed by an empty
/// sequences section. `tree` writes the tree description from the explicit
/// weights.
fn huffman_block_body(
    lits: &[u8],
    bits: &[u8],
    four: bool,
    tree: impl Fn(&[u8]) -> Vec<u8>,
) -> Vec<u8> {
    let table_log = *bits.iter().max().unwrap();
    let weights: Vec<u8> = bits.iter().map(|&b| table_log + 1 - b).collect();
    // Code of each symbol: weights from 1 up take the decode table from its
    // start, symbols in order within a weight, as C zstd fills it.
    let mut codes = vec![0u32; bits.len()];
    let mut next = 0u32;
    for w in 1..=table_log {
        for s in 0..bits.len() {
            if weights[s] == w {
                codes[s] = next >> (w - 1);
                next += 1 << (w - 1);
            }
        }
    }
    assert_eq!(next, 1 << table_log, "code lengths must fill the table");

    let tree = tree(&weights[..weights.len() - 1]);

    let encode = |part: &[u8]| {
        let mut w = BitWriter::new();
        for &s in part.iter().rev() {
            w.write_bits(codes[s as usize], bits[s as usize]);
        }
        w.close_reverse_stream();
        w.into_bytes()
    };
    let mut streams = Vec::new();
    if four {
        let seg = lits.len().div_ceil(4);
        let parts: Vec<&[u8]> = (0..4)
            .map(|k| &lits[(seg * k).min(lits.len())..(seg * (k + 1)).min(lits.len())])
            .collect();
        let enc: Vec<Vec<u8>> = parts.iter().map(|p| encode(p)).collect();
        for e in &enc[..3] {
            streams.extend_from_slice(&(e.len() as u16).to_le_bytes());
        }
        for e in &enc {
            streams.extend_from_slice(e);
        }
    } else {
        streams = encode(lits);
    }

    let regen = lits.len() as u32;
    let comp = (tree.len() + streams.len()) as u32;
    assert!(regen < 1024 && comp < 1024);
    // Compressed literals, size format 0 (one stream) or 1 (four), 10-bit
    // sizes.
    let h = 2 | (u32::from(four) << 2) | (regen << 4) | (comp << 14);
    let mut body = vec![h as u8, (h >> 8) as u8, (h >> 16) as u8];
    body.extend_from_slice(&tree);
    body.extend_from_slice(&streams);
    body.push(0); // no sequences
    body
}

/// The direct tree description of the explicit weights `weights`.
fn direct_tree(weights: &[u8]) -> Vec<u8> {
    let mut tree = vec![127 + weights.len() as u8];
    for pair in weights.chunks(2) {
        tree.push(pair[0] << 4 | pair.get(1).copied().unwrap_or(0));
    }
    tree
}

/// The FSE-compressed tree description of the explicit weights `weights`,
/// at least two, with the table `dist` at accuracy log `log`. The weights
/// are encoded as zrip and C zstd encode them, with two alternating states.
fn fse_tree(weights: &[u8], dist: &[i16], log: u8) -> Vec<u8> {
    use zrip_core::fse::encode::{FseEncodeState, FseEncodeTable};
    use zrip_core::fse::table_builder::serialize_fse_table_description;

    let table = FseEncodeTable::from_distribution(dist, log);
    let mut w = BitWriter::new();
    let n = weights.len();
    let mut i = n - 2;
    let mut s1 = FseEncodeState::init(&table, weights[n - 2 + n % 2]);
    let mut s2 = FseEncodeState::init(&table, weights[n - 1 - n % 2]);
    if n % 2 == 1 {
        i -= 1;
        s1.encode_symbol(&mut w, weights[i]);
    }
    while i > 0 {
        s2.encode_symbol(&mut w, weights[i - 1]);
        s1.encode_symbol(&mut w, weights[i - 2]);
        i -= 2;
    }
    s2.flush(&mut w, log);
    s1.flush(&mut w, log);
    w.close_reverse_stream();

    let mut body = serialize_fse_table_description(dist, log);
    body.extend(w.into_bytes());
    let mut tree = vec![body.len() as u8];
    tree.extend(body);
    tree
}

#[test]
fn huffman_literal_sizes_agree() {
    let mut failures = Failures::default();
    // Three symbols of 1, 2 and 2 bits, and a 12-bit-deep code.
    let deep: Vec<u8> = (1..=12).chain([12]).collect();
    for bits in [vec![1u8, 2, 2], deep] {
        for n in 1..=40usize {
            let lits: Vec<u8> = (0..n).map(|i| ((i * 7) % bits.len()) as u8).collect();
            for four in [false, true] {
                let f = compressed_frame(n, &huffman_block_body(&lits, &bits, four, direct_tree));
                let what = format!("{} symbols, {n} literals, four streams {four}", bits.len());
                if std::env::var_os("ZRIP_DIFF_LOG").is_some() {
                    eprintln!("CRAFT {what}: C accepts {}", check::c_accepts(&f));
                }
                failures.check(&f, None, &what);
            }
        }
    }
}

#[test]
fn huffman_weight_tables_agree() {
    let mut failures = Failures::default();
    // Weights 3, 2, 1 and 1. The weight table also gives symbol `last`, which
    // the weights never use. C zstd takes up to 92 symbols at accuracy log 5
    // and 12 at log 6.
    let bits = [1u8, 2, 3, 3];
    let lits: Vec<u8> = (0..64).map(|i| [0u8, 0, 1, 2, 0, 3][i % 6]).collect();
    for (log, counts, most, lasts) in [
        (
            5u8,
            [10i16, 10, 11],
            92,
            &[4usize, 12, 13, 45, 91, 92, 255][..],
        ),
        (6, [21, 21, 21], 12, &[4, 11, 12, 13, 40][..]),
    ] {
        for &last in lasts {
            let mut dist = vec![0i16; last + 1];
            dist[1..4].copy_from_slice(&counts);
            dist[last] = 1;
            // Past 12, also with `last` decoded as the second weight, which
            // fails.
            for decoded in [false, true] {
                if decoded && last <= 12 {
                    continue;
                }
                let accepts = last < most && !decoded;
                let tree = |w: &[u8]| {
                    if decoded {
                        fse_tree(&[w[0], last as u8, w[1], w[2]], &dist, log)
                    } else {
                        fse_tree(w, &dist, log)
                    }
                };
                for four in [false, true] {
                    let body = huffman_block_body(&lits, &bits, four, tree);
                    let f = compressed_frame(lits.len(), &body);
                    let what = format!("log {log}, last {last}, decoded {decoded}, four {four}");
                    assert_eq!(check::c_accepts(&f), accepts, "C on {what}");
                    if accepts {
                        let out = zstd::bulk::decompress(&f, lits.len()).unwrap();
                        assert_eq!(out, lits, "C on {what}");
                    }
                    failures.check(&f, None, &what);
                }
            }
        }
    }
}

#[test]
fn frame_headers_agree() {
    let mut failures = Failures::default();
    let raw1 = block(true, 0, 1, b"x");
    // Window descriptors, without and with a content size.
    for exponent in 0..=31u8 {
        for mantissa in [0u8, 7] {
            let wd = exponent << 3 | mantissa;
            let what = format!("window {exponent}/{mantissa}");
            failures.check(&frame(0x00, &[wd], &raw1), None, &what);
            failures.check(&frame(0x80, &[wd, 1, 0, 0, 0], &raw1), None, &what);
        }
    }
    // Every descriptor bit pattern over a few field layouts.
    for fhd in 0..=255u8 {
        let fields: Vec<u8> = match fhd >> 6 {
            0 => vec![],
            1 => vec![0, 0],
            2 => vec![1, 0, 0, 0],
            _ => vec![1, 0, 0, 0, 0, 0, 0, 0],
        };
        let mut all = Vec::new();
        if fhd & 0x20 == 0 {
            all.push(0x00); // window descriptor
        }
        all.extend_from_slice(&[0, 0, 0, 0][..[0, 1, 2, 4][usize::from(fhd & 3)]]);
        if fhd >> 6 == 0 && fhd & 0x20 != 0 {
            all.push(1); // 1-byte content size
        } else {
            all.extend_from_slice(&fields);
        }
        let mut f = frame(fhd, &all, &raw1);
        if fhd & 4 != 0 {
            let h = zrip_core::xxhash::xxh64(b"x", 0) as u32;
            f.extend_from_slice(&h.to_le_bytes());
        }
        failures.check(&f, None, &format!("descriptor {fhd:#04x}"));
    }
}

#[test]
fn blocks_agree() {
    let mut failures = Failures::default();
    let wd = 0x00; // 1 KiB window
    let big = 7 << 3; // 128 KiB window
    let cases: Vec<(String, Vec<u8>)> = vec![
        ("empty raw".into(), frame(0, &[wd], &block(true, 0, 0, &[]))),
        (
            "empty rle".into(),
            frame(0, &[wd], &block(true, 1, 0, &[9])),
        ),
        (
            "reserved type".into(),
            frame(0, &[wd], &block(true, 3, 1, b"x")),
        ),
        (
            "raw over window".into(),
            frame(0, &[wd], &block(true, 0, 1025, &[1; 1025])),
        ),
        (
            "rle over window".into(),
            frame(0, &[wd], &block(true, 1, 1025, &[1])),
        ),
        (
            "raw 128 KiB".into(),
            frame(0, &[big], &block(true, 0, 1 << 17, &vec![2; 1 << 17])),
        ),
        (
            "rle 128 KiB".into(),
            frame(0, &[big], &block(true, 1, 1 << 17, &[2])),
        ),
        (
            "rle over 128 KiB".into(),
            frame(0, &[big + 8], &block(true, 1, (1 << 17) + 1, &[2])),
        ),
        (
            "not last".into(),
            frame(0, &[wd], &block(false, 0, 1, b"x")),
        ),
        (
            "truncated block".into(),
            frame(0, &[wd], &block(true, 0, 5, b"xy")),
        ),
        (
            "size mismatch short".into(),
            frame(0x20, &[2], &block(true, 0, 1, b"x")),
        ),
        (
            "size mismatch long".into(),
            frame(0x20, &[0], &block(true, 0, 1, b"x")),
        ),
        ("bad checksum".into(), {
            let mut f = frame(0x24, &[1], &block(true, 0, 1, b"x"));
            f.extend_from_slice(&[0; 4]);
            f
        }),
        ("trailing byte".into(), {
            let mut f = frame(0, &[wd], &block(true, 0, 1, b"x"));
            f.push(0);
            f
        }),
        ("truncated magic".into(), MAGIC[..3].to_vec()),
        (
            "skippable, short".into(),
            vec![0x5F, 0x2A, 0x4D, 0x18, 9, 0, 0, 0, 1],
        ),
        (
            "skippable, huge".into(),
            vec![0x5A, 0x2A, 0x4D, 0x18, 0xFF, 0xFF, 0xFF, 0xFF],
        ),
        (
            "skippable, empty".into(),
            vec![0x50, 0x2A, 0x4D, 0x18, 0, 0, 0, 0],
        ),
        (
            "legacy magic".into(),
            vec![0x27, 0xB5, 0x2F, 0xFD, 0, 0, 0, 0],
        ),
    ];
    for (what, f) in cases {
        failures.check(&f, None, &what);
    }
}

#[test]
fn literals_headers_agree() {
    let mut failures = Failures::default();
    // Raw and RLE literals in every size format, then no sequences.
    let mut cases = Vec::new();
    for n in [0usize, 1, 31, 32, 4095, 4096, 70_000] {
        for rle in [false, true] {
            let kind = u32::from(rle);
            let mut body = if n < 32 {
                vec![(kind | (n as u32) << 3) as u8]
            } else if n < 4096 {
                let h = kind | 1 << 2 | (n as u32) << 4;
                vec![h as u8, (h >> 8) as u8]
            } else {
                let h = kind | 3 << 2 | (n as u32) << 4;
                vec![h as u8, (h >> 8) as u8, (h >> 16) as u8]
            };
            if rle {
                body.push(b'r');
            } else {
                body.extend(std::iter::repeat_n(b'w', n));
            }
            body.push(0);
            cases.push((
                format!("{n} literals rle {rle}"),
                compressed_frame(n, &body),
            ));
        }
    }
    // Treeless literals with no table to repeat.
    let h = 3u32 | 6 << 4 | 4 << 14;
    let body = vec![h as u8, (h >> 8) as u8, (h >> 16) as u8, 1, 2, 3, 0x80, 0];
    cases.push(("treeless first".into(), compressed_frame(6, &body)));
    for (what, f) in cases {
        failures.check(&f, None, &what);
    }
}
