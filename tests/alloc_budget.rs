//! Memory each entry point takes for one frame, against what the frame needs:
//! its match tables, buffers in proportion to the input and dictionary, and
//! a fixed allowance. Catches tables and buffers built for inputs the frame
//! does not have, which zeroing audits and benchmarks of reused contexts
//! miss. Reused objects must not allocate once warm.
//!
//! `ZRIP_ALLOC_REPORT=1` prints every measurement.
#![cfg(feature = "std")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::io::{Read, Write};

use zrip::encode::strategy::{self, LevelParams};
use zrip::push::{EndOp, PushDecoder, PushEncoder};
use zrip::{
    CompressContext, DecompressContext, Dictionary, FrameDecoder, FrameEncoder, compress_bound,
};

struct Counting;

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

fn grew(n: usize, new_block: bool) {
    let _ = LIVE.try_with(|live| {
        let v = live.get() + n as isize;
        live.set(v);
        let _ = PEAK.try_with(|p| p.set(p.get().max(v)));
    });
    if new_block {
        let _ = ALLOCS.try_with(|a| a.set(a.get() + 1));
    }
}

fn shrank(n: usize) {
    let _ = LIVE.try_with(|live| live.set(live.get() - n as isize));
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        grew(layout.size(), true);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        grew(layout.size(), true);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        shrank(layout.size());
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size > layout.size() {
            grew(new_size - layout.size(), true);
        } else {
            shrank(layout.size() - new_size);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Peak bytes live above the start while `f` runs, and allocations it made.
fn measure(f: impl FnOnce()) -> (usize, usize) {
    let start = LIVE.with(Cell::get);
    PEAK.with(|p| p.set(start));
    ALLOCS.with(|a| a.set(0));
    f();
    let peak = PEAK.with(Cell::get) - start;
    (peak.max(0) as usize, ALLOCS.with(Cell::get))
}

const LEVELS: [i32; 7] = [-7, -3, 1, 2, 3, 4, -8];
const SIZES: [usize; 5] = [64, 1024, 16 << 10, 128 << 10, 600 << 10];

fn sample(len: usize) -> Vec<u8> {
    const WORDS: [&[u8]; 8] = [
        b"frame ",
        b"block ",
        b"match ",
        b"table ",
        b"zero ",
        b"once ",
        b"literal ",
        b"offset ",
    ];
    let mut x = 0x9e37_79b9u32;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        if x.is_multiple_of(8) {
            out.push((x >> 24) as u8);
        } else {
            out.extend_from_slice(WORDS[(x >> 8) as usize % WORDS.len()]);
        }
    }
    out.truncate(len);
    out
}

fn dict() -> Dictionary {
    let fixture = include_bytes!("fixtures/corpus_dict_roundtrip.bin");
    let len = u32::from_le_bytes(fixture[..4].try_into().unwrap()) as usize;
    Dictionary::from_bytes(&fixture[4..4 + len]).unwrap()
}

fn table_bytes(p: &LevelParams) -> usize {
    match p.strategy {
        strategy::Strategy::Fast => 4 << p.hash_log,
        strategy::Strategy::DFast => (4 << p.chain_log) + (4 << p.hash_log),
    }
}

/// Tables for a frame of `len` bytes that knows its size up front.
fn sized(level: i32, len: usize) -> usize {
    table_bytes(&strategy::level_params_for_size(level, len).unwrap())
}

/// Tables for a frame of `len` bytes after a dictionary. The Fast strategy
/// keeps the prefilled dictionary table beside the one for the input.
fn with_prefix(level: i32, len: usize) -> usize {
    let p = strategy::level_params_for_size(level, len).unwrap();
    match p.strategy {
        strategy::Strategy::Fast => 2 * table_bytes(&p),
        strategy::Strategy::DFast => table_bytes(&p),
    }
}

const BLOCK: usize = 128 << 10;

/// Tables for a stream whose size is unknown when its first block starts,
/// and past one block, the window it reserves toward the most it holds.
fn streaming(level: i32, len: usize) -> usize {
    let p = strategy::level_params(level).unwrap();
    let window = if len > BLOCK {
        (2 << p.window_log) + BLOCK
    } else {
        0
    };
    table_bytes(&p) + window
}

/// A push frame that ends before a block goes out is encoded in one pass.
fn push_end(level: i32, dict_len: usize, len: usize) -> usize {
    if len <= BLOCK && dict_len > 0 {
        with_prefix(level, dict_len + len)
    } else if len <= BLOCK {
        sized(level, len)
    } else {
        streaming(level, len)
    }
}

/// Collects every measurement over budget, and prints them all with
/// `ZRIP_ALLOC_REPORT` set.
struct Report {
    print: bool,
    /// Allowance per byte of input, output and dictionary, for windows,
    /// copies and sequences, and a fixed one for workspaces.
    per_byte: usize,
    fixed: usize,
    failures: RefCell<Vec<String>>,
}

impl Report {
    fn new(per_byte: usize, fixed: usize) -> Self {
        Self {
            print: std::env::var_os("ZRIP_ALLOC_REPORT").is_some(),
            per_byte,
            fixed,
            failures: RefCell::default(),
        }
    }

    /// Checks that `f` takes no more than `tables` plus the allowances for
    /// `bytes` of input, output and dictionary.
    fn check(&self, what: &str, tables: usize, bytes: usize, f: impl FnOnce()) {
        let (peak, allocs) = measure(f);
        let budget = tables + self.per_byte * bytes + self.fixed;
        if self.print {
            println!(
                "{what:<44} peak {peak:>8} tables {tables:>8} budget {budget:>8} allocs {allocs:>3} {}",
                if peak > budget { "OVER" } else { "" }
            );
        }
        if peak > budget {
            self.failures.borrow_mut().push(format!(
                "{what}: {peak} bytes, budget {budget} ({tables} for tables)"
            ));
        }
    }

    /// Checks that a warm, reused object does not allocate for `f`.
    fn warm(&self, what: &str, mut f: impl FnMut()) {
        f();
        f();
        let (_, allocs) = measure(&mut f);
        if self.print {
            println!("{what:<44} warm allocs {allocs}");
        }
        if allocs != 0 {
            self.failures
                .borrow_mut()
                .push(format!("{what}: a warm frame allocates {allocs} times"));
        }
    }

    fn finish(self) {
        let failures = self.failures.into_inner();
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }
}

#[test]
fn encoders_take_what_the_frame_needs() {
    // Fixed allowance well under the 128 KiB block a stray reservation takes.
    let report = Report::new(3, 32 * 1024);
    let dict = dict();
    let dlen = dict.content().len();
    for level in LEVELS {
        for size in SIZES {
            let input = sample(size);
            let at = format!("L{level} {size} B");
            let n = size + compress_bound(size);
            let nd = n + dlen;

            report.check(&format!("compress {at}"), sized(level, size), n, || {
                zrip::compress(&input, level).unwrap();
            });
            let mut out = vec![0; compress_bound(size)];
            report.check(
                &format!("compress_into {at}"),
                sized(level, size),
                n,
                || {
                    zrip::compress_into(&input, &mut out, level).unwrap();
                },
            );
            report.check(
                &format!("compress_with_dict {at}"),
                with_prefix(level, dlen + size),
                nd,
                || {
                    zrip::compress_with_dict(&input, level, &dict).unwrap();
                },
            );
            report.check(&format!("context {at}"), sized(level, size), n, || {
                CompressContext::new(level)
                    .unwrap()
                    .compress(&input)
                    .unwrap();
            });
            let d = dict.clone();
            // A dictionary context keeps a prefilled copy of the tables for
            // inputs of its tier, beside the ones the frame works in.
            report.check(
                &format!("dict context {at}"),
                2 * sized(level, dlen + size),
                nd,
                || {
                    CompressContext::with_dict(level, d)
                        .unwrap()
                        .compress(&input)
                        .unwrap();
                },
            );
            report.check(
                &format!("context compress_with_dict {at}"),
                with_prefix(level, dlen + size),
                nd,
                || {
                    CompressContext::new(level)
                        .unwrap()
                        .compress_with_dict(&input, &dict)
                        .unwrap();
                },
            );
            report.check(&format!("push {at}"), push_end(level, 0, size), n, || {
                let mut out = Vec::new();
                PushEncoder::new(level)
                    .unwrap()
                    .encode_to_vec(&input, &mut out, EndOp::End)
                    .unwrap();
            });
            report.check(
                &format!("push flush {at}"),
                streaming(level, size),
                n,
                || {
                    let mut out = Vec::new();
                    let mut enc = PushEncoder::new(level).unwrap();
                    enc.encode_to_vec(&input, &mut out, EndOp::Continue)
                        .unwrap();
                    enc.encode_to_vec(&[], &mut out, EndOp::Flush).unwrap();
                    enc.encode_to_vec(&[], &mut out, EndOp::End).unwrap();
                },
            );
            let d = dict.clone();
            report.check(
                &format!("push dict {at}"),
                push_end(level, dlen, size),
                nd,
                || {
                    let mut out = Vec::new();
                    PushEncoder::with_dict(level, d)
                        .unwrap()
                        .encode_to_vec(&input, &mut out, EndOp::End)
                        .unwrap();
                },
            );
            report.check(
                &format!("frame encoder {at}"),
                streaming(level, size),
                n,
                || {
                    let mut enc = FrameEncoder::new(Vec::new(), level).unwrap();
                    enc.write_all(&input).unwrap();
                    enc.finish().unwrap();
                },
            );

            let mut ctx = CompressContext::new(level).unwrap();
            report.warm(&format!("context {at}"), || {
                ctx.compress(&input).unwrap();
            });
            let mut ctx = CompressContext::with_dict(level, dict.clone()).unwrap();
            report.warm(&format!("dict context {at}"), || {
                ctx.compress(&input).unwrap();
            });
            let mut enc = PushEncoder::new(level).unwrap();
            let mut out = Vec::new();
            report.warm(&format!("push flush {at}"), || {
                out.clear();
                enc.encode_to_vec(&input, &mut out, EndOp::Continue)
                    .unwrap();
                enc.encode_to_vec(&[], &mut out, EndOp::Flush).unwrap();
                enc.encode_to_vec(&[], &mut out, EndOp::End).unwrap();
            });
            let mut enc = PushEncoder::new(level).unwrap();
            report.warm(&format!("push {at}"), || {
                out.clear();
                enc.encode_to_vec(&input, &mut out, EndOp::End).unwrap();
            });
            let mut enc = PushEncoder::with_dict(level, dict.clone()).unwrap();
            report.warm(&format!("push dict {at}"), || {
                out.clear();
                enc.encode_to_vec(&input, &mut out, EndOp::End).unwrap();
            });
            let mut enc = PushEncoder::with_dict(level, dict.clone()).unwrap();
            report.warm(&format!("push dict flush {at}"), || {
                out.clear();
                enc.encode_to_vec(&input, &mut out, EndOp::Continue)
                    .unwrap();
                enc.encode_to_vec(&[], &mut out, EndOp::Flush).unwrap();
                enc.encode_to_vec(&[], &mut out, EndOp::End).unwrap();
            });
            let mut enc = FrameEncoder::new(std::io::sink(), level).unwrap();
            report.warm(&format!("frame encoder {at}"), || {
                enc.write_all(&input).unwrap();
                enc.reset(std::io::sink()).unwrap();
            });
        }
    }
    report.finish();
}

#[test]
fn decoders_take_what_the_frame_needs() {
    // Output grows by doubling, and frame decoders keep a window beside it.
    let report = Report::new(4, 96 * 1024);
    let dict = dict();
    let dlen = dict.content().len();
    for size in SIZES {
        let input = sample(size);
        let frame = zrip::compress(&input, 1).unwrap();
        let dict_frame = zrip::compress_with_dict(&input, 1, &dict).unwrap();
        let at = format!("{size} B");
        let n = size;
        let nd = size + dlen;

        report.check(&format!("decompress {at}"), 0, n, || {
            zrip::decompress(&frame).unwrap();
        });
        report.check(&format!("decompress_with_dict {at}"), 0, nd, || {
            zrip::decompress_with_dict(&dict_frame, &dict).unwrap();
        });
        report.check(&format!("decompress context {at}"), 0, n, || {
            DecompressContext::new().decompress(&frame).unwrap();
        });
        report.check(&format!("frame decoder {at}"), 0, n, || {
            let mut out = Vec::new();
            FrameDecoder::new(frame.as_slice())
                .read_to_end(&mut out)
                .unwrap();
        });
        report.check(&format!("push decoder {at}"), 0, n, || {
            let mut out = Vec::new();
            PushDecoder::new().decode_to_vec(&frame, &mut out).unwrap();
        });

        let mut ctx = DecompressContext::new();
        report.warm(&format!("decompress context {at}"), || {
            ctx.decompress(&frame).unwrap();
        });
        let mut dec = PushDecoder::new();
        let mut out = Vec::new();
        report.warm(&format!("push decoder {at}"), || {
            out.clear();
            dec.reset();
            dec.decode_to_vec(&frame, &mut out).unwrap();
        });
        let mut dec = FrameDecoder::new(frame.as_slice());
        report.warm(&format!("frame decoder {at}"), || {
            out.clear();
            dec.reset(frame.as_slice());
            dec.read_to_end(&mut out).unwrap();
        });
    }
    report.finish();
}

#[test]
fn stream_decoders_reserve_at_most_8_mib_for_a_declared_content_size() {
    // A 128 MiB window and content size: the header alone, and with a
    // 16-byte raw block.
    let mut header = vec![0x28, 0xb5, 0x2f, 0xfd, 0xc0, 0x88];
    header.extend_from_slice(&(128u64 << 20).to_le_bytes());
    let mut one_block = header.clone();
    one_block.extend_from_slice(&[0x80, 0x00, 0x00]);
    one_block.extend_from_slice(&[7; 16]);
    let report = Report::new(4, 96 * 1024);
    // Without a limit, the header reserves 8 MiB. With a 64 KiB limit, it
    // reserves that, and the block up to 128 KiB, the most it may write.
    let limits = [
        ("no limit", usize::MAX, 8 << 20),
        ("64 KiB limit", 64 << 10, 128 << 10),
    ];
    for (limited, limit, reserve) in limits {
        for (what, frame) in [("header", &header), ("one block", &one_block)] {
            let at = format!("{what}, {limited}");
            report.check(&format!("frame decoder, {at}"), reserve, 0, || {
                let mut out = Vec::new();
                FrameDecoder::with_limit(frame.as_slice(), limit)
                    .read_to_end(&mut out)
                    .unwrap_err();
            });
            report.check(&format!("push decoder, {at}"), reserve, 0, || {
                let mut out = Vec::new();
                let mut dec = PushDecoder::with_limit(limit);
                dec.decode_to_vec(frame, &mut out).unwrap();
                dec.finish().unwrap_err();
            });
        }
    }
    report.finish();
}
