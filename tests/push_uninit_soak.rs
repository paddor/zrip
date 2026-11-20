// Soak of the push API's uninitialized outputs. Every scenario drives two
// coders in lockstep with the same random calls: one through `encode` or
// `decode` into initialized buffers, the other through `encode_uninit` or
// `decode_uninit`. Each call must return the same result, write the same
// bytes and nothing past `produced`, and leave the same state.
//
// `cargo test` runs a short pass. The timed soak runs with
// `cargo test --release --test zrip_push_uninit_soak -- --ignored --nocapture`,
// for `ZRIP_SOAK_SECS` seconds (default 300). `ZRIP_SOAK_SEED` replays one
// case, as printed by a failure. Under Miri, a few small cases run without
// C zstd.
#![cfg(feature = "std")]

use std::mem::MaybeUninit;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

use zrip::push::{EndOp, Progress, PushDecoder, PushEncoder, Status};
use zrip::{DecompressError, Dictionary, MAX_LEVEL, MIN_LEVEL, Options};

/// Bytes the initialized buffers start with.
const INIT_POISON: u8 = 0x5A;
/// Bytes the poisoned uninitialized buffers start with.
const UNINIT_POISON: u8 = 0xA5;

/// What the scenarios exercised, for the soak's report.
#[derive(Clone, Copy)]
enum Stat {
    EncodeCalls,
    DecodeCalls,
    SpareCalls,
    MixedCalls,
    Frames,
    Abandoned,
    BytesIn,
    DecodeErrors,
}

static STATS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

fn count(stat: Stat, n: usize) {
    STATS[stat as usize].fetch_add(n as u64, Relaxed);
}

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + self.below((hi - lo + 1) as usize) as i32
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    /// 0 to `max`, about evenly spread over powers of two.
    fn size(&mut self, max: usize) -> usize {
        let bits = self.below((usize::BITS - max.leading_zeros()) as usize + 1);
        (self.next() as usize & ((1usize << bits) - 1)).min(max)
    }
}

/// Text-like words, runs, near and far repeats, skewed and random bytes.
fn data(rng: &mut Rng, len: usize) -> Vec<u8> {
    const WORDS: [&[u8]; 8] = [
        b"alpha ",
        b"beta ",
        b"gamma ",
        b"delta ",
        b"{\"id\":",
        b"\"name\":",
        b"0123",
        b"\n",
    ];
    let mut out = Vec::with_capacity(len + 2048);
    while out.len() < len {
        let n = 1 + rng.size(256);
        match rng.below(6) {
            0 => out.extend((0..n).map(|_| rng.next() as u8)),
            1 => out.extend(std::iter::repeat_n(rng.next() as u8, n)),
            2 => {
                for _ in 0..=n / 4 {
                    out.extend_from_slice(WORDS[rng.below(8)]);
                }
            }
            3 => out.extend((0..n).map(|_| {
                let r = rng.next();
                ((r >> 58) & (r >> 52) & 63) as u8 + b' '
            })),
            _ if !out.is_empty() => {
                let start = rng.below(out.len());
                let n = n.min(out.len() - start);
                out.extend_from_within(start..start + n);
            }
            _ => {}
        }
    }
    out.truncate(len);
    out
}

/// Settings of one test run.
struct Limits {
    max_frame: usize,
    max_out: usize,
    max_in_chunk: usize,
    /// Cross-check with C zstd, and use its dictionary. Off under Miri.
    c_zstd: bool,
}

/// A dictionary trained by C zstd, as its bytes and parsed.
struct Dict {
    bytes: Vec<u8>,
    dict: Dictionary,
}

fn train_dict() -> Dict {
    let mut rng = Rng(7);
    let samples: Vec<Vec<u8>> = (0..300).map(|_| data(&mut rng, 600)).collect();
    let bytes = zstd::dict::from_samples(&samples, 8192).unwrap();
    let dict = Dictionary::from_bytes(&bytes).unwrap();
    Dict { bytes, dict }
}

/// An output buffer on the uninitialized side.
enum UninitBuf {
    /// Filled with `UNINIT_POISON`, so a byte reported but not written, or
    /// written past `produced`, shows.
    Poisoned(Vec<MaybeUninit<u8>>),
    /// A vector's spare capacity, as I/O adapters pass it: truly
    /// uninitialized, which Miri checks.
    Spare(Vec<u8>),
    /// An initialized buffer for the initialized call, as a caller mixing
    /// the two would pass.
    Init(Vec<u8>),
}

/// Output buffers of one call for both sides.
struct Call {
    init: Vec<u8>,
    uninit: UninitBuf,
}

impl Call {
    fn new(rng: &mut Rng, max_out: usize) -> Self {
        // Mostly room to progress; empty buffers sometimes.
        let len = if rng.chance(3) {
            0
        } else {
            1 + rng.size(max_out - 1)
        };
        let uninit = match rng.below(10) {
            0 => {
                count(Stat::MixedCalls, 1);
                UninitBuf::Init(vec![INIT_POISON; len])
            }
            1..=5 => UninitBuf::Poisoned(vec![MaybeUninit::new(UNINIT_POISON); len]),
            _ => {
                count(Stat::SpareCalls, 1);
                UninitBuf::Spare(Vec::with_capacity(len))
            }
        };
        Call {
            init: vec![INIT_POISON; len],
            uninit,
        }
    }

    fn room(&self) -> usize {
        self.init.len()
    }

    /// Checks both calls' results and returns the bytes they produced.
    fn check(&mut self, a: Progress, b: Progress) -> Vec<u8> {
        assert_eq!(a, b, "progress differs (out={})", self.room());
        let untouched = |tail: &[u8], poison| tail.iter().all(|&b| b == poison);
        assert!(
            untouched(&self.init[a.produced..], INIT_POISON),
            "init call wrote past `produced`"
        );
        let out = match &mut self.uninit {
            UninitBuf::Init(v) => {
                assert!(untouched(&v[b.produced..], INIT_POISON));
                v[..b.produced].to_vec()
            }
            UninitBuf::Poisoned(v) => {
                // The poison initialized every byte.
                let v: Vec<u8> = v.iter().map(|b| unsafe { b.assume_init() }).collect();
                assert!(
                    untouched(&v[b.produced..], UNINIT_POISON),
                    "uninit call wrote past `produced`"
                );
                v[..b.produced].to_vec()
            }
            UninitBuf::Spare(v) => {
                assert!(b.produced <= v.capacity());
                // The call initialized the first `produced` bytes.
                unsafe { v.set_len(b.produced) };
                v.clone()
            }
        };
        assert_eq!(out, self.init[..a.produced], "bytes differ");
        out
    }
}

/// One encoded frame and its content.
struct Frame {
    content: Vec<u8>,
    bytes: Vec<u8>,
}

fn encoder_pair(
    rng: &mut Rng,
    lim: &Limits,
    dict: Option<&Dict>,
    level: i32,
) -> (PushEncoder, PushEncoder) {
    match rng.below(4) {
        0 if dict.is_some() => {
            let d = &dict.unwrap().dict;
            (
                PushEncoder::with_dict(level, d.clone()).unwrap(),
                PushEncoder::with_dict(level, d.clone()).unwrap(),
            )
        }
        1 if lim.c_zstd => {
            let log = if rng.chance(30) {
                0
            } else {
                rng.range(10, 22) as u32
            };
            let opts = Options::default().window_log(log).ldm(rng.chance(30));
            (
                PushEncoder::with_options(level, &opts).unwrap(),
                PushEncoder::with_options(level, &opts).unwrap(),
            )
        }
        _ => (
            PushEncoder::new(level).unwrap(),
            PushEncoder::new(level).unwrap(),
        ),
    }
}

/// Encodes random frames in lockstep and returns the finished ones.
fn encode_scenario(rng: &mut Rng, lim: &Limits, dict: Option<&Dict>) -> Vec<Frame> {
    let level = if rng.chance(5) {
        0
    } else {
        rng.range(MIN_LEVEL, MAX_LEVEL)
    };
    let (mut a, mut b) = encoder_pair(rng, lim, dict, level);
    let mut frames = Vec::new();
    for _ in 0..=rng.below(3) {
        let len = rng.size(lim.max_frame);
        let content = data(rng, len);
        let abandon_at = rng.chance(10).then(|| rng.size(content.len()));
        let mut bytes = Vec::new();
        let mut pos = 0;
        let mut finished = false;
        while !finished {
            if abandon_at.is_some_and(|at| pos >= at) {
                count(Stat::Abandoned, 1);
                a.reset();
                b.reset();
                assert_eq!(a.has_pending_output(), b.has_pending_output());
                break;
            }
            let n = rng.size(lim.max_in_chunk).min(content.len() - pos);
            let mut input = &content[pos..pos + n];
            pos += n;
            let op = if pos == content.len() && rng.chance(80) {
                EndOp::End
            } else if rng.chance(15) {
                EndOp::Flush
            } else {
                EndOp::Continue
            };
            let mut stalls = 0;
            loop {
                let mut call = Call::new(rng, lim.max_out);
                count(Stat::EncodeCalls, 1);
                let pa = a.encode(input, &mut call.init, op).unwrap();
                let pb = match &mut call.uninit {
                    UninitBuf::Init(v) => b.encode(input, v, op),
                    UninitBuf::Poisoned(v) => b.encode_uninit(input, v, op),
                    UninitBuf::Spare(v) => b.encode_uninit(input, v.spare_capacity_mut(), op),
                }
                .unwrap();
                bytes.extend(call.check(pa, pb));
                assert_eq!(a.has_pending_output(), b.has_pending_output());
                input = &input[pa.consumed..];
                stalls = if pa.consumed + pa.produced == 0 && call.room() > 0 {
                    stalls + 1
                } else {
                    0
                };
                assert!(stalls < 3, "no progress with {} bytes of room", call.room());
                match (op, pa.status) {
                    (_, Status::NeedsOutput) => {}
                    (EndOp::End, Status::Finished) => {
                        assert_eq!(input, &[] as &[u8]);
                        finished = true;
                        break;
                    }
                    (EndOp::End, s) => panic!("End stopped with {s:?}"),
                    (_, Status::NeedsInput) => {
                        assert_eq!(input, &[] as &[u8]);
                        break;
                    }
                    (_, Status::Finished) => panic!("{op:?} finished a frame"),
                }
            }
        }
        if finished {
            count(Stat::Frames, 1);
            count(Stat::BytesIn, content.len());
            frames.push(Frame { content, bytes });
        }
    }
    frames
}

/// Checks each frame with C zstd.
fn c_decodes(frames: &[Frame], dict: Option<&Dict>) {
    let mut dec = match dict {
        Some(d) => zstd::bulk::Decompressor::with_dictionary(&d.bytes).unwrap(),
        None => zstd::bulk::Decompressor::new().unwrap(),
    };
    for f in frames {
        let out = dec.decompress(&f.bytes, f.content.len()).unwrap();
        assert_eq!(out, f.content, "C zstd decodes the frame");
    }
}

/// Decodes `stream` in lockstep. Returns the output, or the error both
/// sides hit.
fn decode_scenario(
    rng: &mut Rng,
    lim: &Limits,
    stream: &[u8],
    dict: Option<&Dict>,
    limit: Option<usize>,
) -> Result<Vec<u8>, DecompressError> {
    let make = || match (dict, limit) {
        (Some(d), Some(l)) => PushDecoder::with_dict_and_limit(d.dict.clone(), l),
        (Some(d), None) => PushDecoder::with_dict(d.dict.clone()),
        (None, Some(l)) => PushDecoder::with_limit(l),
        (None, None) => PushDecoder::new(),
    };
    let (mut a, mut b) = (make(), make());
    let mut out = Vec::new();
    let mut input = stream;
    let mut stalls = 0;
    loop {
        let n = rng.size(lim.max_in_chunk).min(input.len());
        let mut call = Call::new(rng, lim.max_out);
        count(Stat::DecodeCalls, 1);
        let ra = a.decode(&input[..n], &mut call.init);
        let rb = match &mut call.uninit {
            UninitBuf::Init(v) => b.decode(&input[..n], v),
            UninitBuf::Poisoned(v) => b.decode_uninit(&input[..n], v),
            UninitBuf::Spare(v) => b.decode_uninit(&input[..n], v.spare_capacity_mut()),
        };
        let (pa, pb) = match (ra, rb) {
            (Ok(pa), Ok(pb)) => (pa, pb),
            (Err(ea), Err(eb)) => {
                assert_eq!(ea, eb, "errors differ");
                count(Stat::DecodeErrors, 1);
                // Errors stick on both sides.
                let mut buf = [0u8; 64];
                let mut ubuf = [MaybeUninit::uninit(); 64];
                assert_eq!(a.decode(&[], &mut buf), Err(ea.clone()));
                assert_eq!(b.decode_uninit(&[], &mut ubuf), Err(ea.clone()));
                return Err(ea);
            }
            (ra, rb) => panic!("results differ: {ra:?} vs {rb:?}"),
        };
        out.extend(call.check(pa, pb));
        assert_eq!(a.at_frame_end(), b.at_frame_end());
        assert_eq!(a.has_pending_output(), b.has_pending_output());
        input = &input[pa.consumed..];
        stalls = if pa.consumed + pa.produced == 0 && call.room() > 0 && n > 0 {
            stalls + 1
        } else {
            0
        };
        assert!(stalls < 3, "no progress with input and room");
        if input.is_empty() && pa.status != Status::NeedsOutput && !a.has_pending_output() {
            break;
        }
    }
    let (fa, fb) = (a.finish(), b.finish());
    assert_eq!(fa, fb, "finish differs");
    fa.map(|()| out)
}

/// A skippable frame with a random payload.
fn skippable(rng: &mut Rng) -> Vec<u8> {
    let len = rng.size(64);
    let mut f = vec![0x50 + rng.below(16) as u8, 0x2A, 0x4D, 0x18];
    f.extend_from_slice(&(len as u32).to_le_bytes());
    f.extend((0..len).map(|_| rng.next() as u8));
    f
}

/// One encode scenario, then decode scenarios over its frames and over C
/// zstd's frames of the same content, intact, limited and damaged.
fn run_case(seed: u64, lim: &Limits, dict: Option<&Dict>) {
    let mut rng = Rng(seed);
    let dict = dict.filter(|_| rng.chance(30));
    let frames = encode_scenario(&mut rng, lim, dict);
    if lim.c_zstd {
        c_decodes(&frames, dict);
    }

    // The stream, and its content up to each frame boundary.
    let mut stream = Vec::new();
    let mut bounds = vec![(0, 0)];
    let mut content = Vec::new();
    for f in &frames {
        if rng.chance(20) {
            stream.extend(skippable(&mut rng));
            bounds.push((stream.len(), content.len()));
        }
        stream.extend_from_slice(&f.bytes);
        content.extend_from_slice(&f.content);
        bounds.push((stream.len(), content.len()));
    }
    let out = decode_scenario(&mut rng, lim, &stream, dict, None).unwrap();
    assert_eq!(out, content, "zrip frames decode");

    // C zstd's frames, with and without checksums.
    if lim.c_zstd {
        let mut c_stream = Vec::new();
        for f in &frames {
            let level = rng.range(-5, 12);
            let mut enc = match dict {
                Some(d) => zstd::bulk::Compressor::with_dictionary(level, &d.bytes).unwrap(),
                None => zstd::bulk::Compressor::new(level).unwrap(),
            };
            enc.set_parameter(zstd::zstd_safe::CParameter::ChecksumFlag(rng.chance(50)))
                .unwrap();
            c_stream.extend(enc.compress(&f.content).unwrap());
        }
        let out = decode_scenario(&mut rng, lim, &c_stream, dict, None).unwrap();
        assert_eq!(out, content, "C frames decode");
    }

    // An output limit below the content fails alike on both sides.
    if !content.is_empty() {
        let limit = rng.below(content.len());
        let r = decode_scenario(&mut rng, lim, &stream, dict, Some(limit));
        assert!(r.is_err(), "limit {limit} below {} bytes", content.len());
    }

    if stream.is_empty() {
        return;
    }
    if rng.chance(50) {
        // A cut between frames leaves a valid stream; any other cut fails.
        let cut = rng.below(stream.len());
        let r = decode_scenario(&mut rng, lim, &stream[..cut], dict, None);
        match bounds.iter().find(|&&(end, _)| end == cut) {
            Some(&(_, len)) => assert_eq!(r.unwrap(), content[..len], "cut at {cut}"),
            None => assert!(r.is_err(), "cut at {cut} must not finish"),
        }
    } else {
        // Flipped bits: both sides fail, or decode, alike.
        let mut bad = stream.clone();
        for _ in 0..=rng.below(4) {
            let i = rng.below(bad.len());
            bad[i] ^= 1 << rng.below(8);
        }
        let _ = decode_scenario(&mut rng, lim, &bad, dict, None);
    }
}

/// Runs `run_case` and names the seed that replays a failure.
fn run_case_named(seed: u64, lim: &Limits, dict: Option<&Dict>) {
    let ok = catch_unwind(AssertUnwindSafe(|| run_case(seed, lim, dict))).is_ok();
    assert!(ok, "soak case failed; replay with ZRIP_SOAK_SEED={seed:#x}");
}

fn replay_seed() -> Option<u64> {
    let s = std::env::var("ZRIP_SOAK_SEED").ok()?;
    let s = s.trim_start_matches("0x");
    Some(u64::from_str_radix(s, 16).expect("ZRIP_SOAK_SEED is hex"))
}

#[test]
fn uninit_matches_init_short() {
    let (cases, lim) = if cfg!(miri) {
        (
            4,
            Limits {
                max_frame: 2048,
                max_out: 512,
                max_in_chunk: 1024,
                c_zstd: false,
            },
        )
    } else {
        (
            48,
            Limits {
                max_frame: 64 << 10,
                max_out: 1 << 16,
                max_in_chunk: 16 << 10,
                c_zstd: true,
            },
        )
    };
    let dict = lim.c_zstd.then(train_dict);
    if let Some(seed) = replay_seed() {
        return run_case_named(seed, &lim, dict.as_ref());
    }
    for i in 0..cases {
        run_case_named(0x5EED_0000 + i, &lim, dict.as_ref());
    }
}

#[test]
#[ignore = "timed soak; run with --release -- --ignored"]
fn uninit_matches_init_soak() {
    let lim = Limits {
        max_frame: 4 << 20,
        max_out: 1 << 20,
        max_in_chunk: 1 << 20,
        c_zstd: true,
    };
    let dict = train_dict();
    if let Some(seed) = replay_seed() {
        return run_case_named(seed, &lim, Some(&dict));
    }
    let secs: u64 = std::env::var("ZRIP_SOAK_SECS")
        .map_or(300, |s| s.parse().expect("ZRIP_SOAK_SECS is a number"));
    let base = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let end = Instant::now() + Duration::from_secs(secs);
    let mut cases = 0u64;
    while Instant::now() < end {
        run_case_named(base.wrapping_add(cases), &lim, Some(&dict));
        cases += 1;
    }
    let [enc, dec, spare, mixed, frames, abandoned, bytes, errors] =
        STATS.each_ref().map(|s| s.load(Relaxed));
    eprintln!(
        "{cases} soak cases in {secs} s, base seed {base:#x}: {frames} frames \
     ({} MiB, {abandoned} abandoned), {enc} encode and {dec} decode \
     calls ({spare} into spare capacity, {mixed} mixed), {errors} decode \
     errors",
        bytes >> 20
    );
}
