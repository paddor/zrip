//! Match-finder hash tables that know when they hold only zeros.
//!
//! Every frame starts its tables zeroed. A table fresh from the allocator,
//! or cleared and not borrowed mutably since, is zeroed already, so
//! [`MatchTable::clear`] skips it. Callers and the prefills they call can
//! each ask for a clean table, and no path zeroes one twice.
//!
//! Raw `fill` and `vec![x; n]` are banned in this crate (`clippy.toml`), so
//! tables go through this type.
#![forbid(unsafe_code)]

use alloc::vec::Vec;
use core::ops::{Deref, DerefMut};

/// A table of `u32` positions, derefs to `[u32]`. Any mutable borrow marks
/// it dirty.
pub(crate) struct MatchTable {
    entries: Vec<u32>,
    /// Every entry is zero.
    zeroed: bool,
    #[cfg(test)]
    id: usize,
}

impl MatchTable {
    /// An empty table. It allocates on the first [`clear`](Self::clear).
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
            zeroed: true,
            #[cfg(test)]
            id: audit::next_id(),
        }
    }

    /// A table of `len` zero entries.
    pub(crate) fn zeroed(len: usize) -> Self {
        let mut table = Self::new();
        table.clear(len);
        table
    }

    /// Makes the table `len` entries, all zero. Zeroes only entries that may
    /// hold positions; a table that outgrows its allocation gets a fresh
    /// zeroed one.
    pub(crate) fn clear(&mut self, len: usize) {
        let old = self.entries.len();
        let zeroed = if len > self.entries.capacity() {
            // Free first, so the allocator can reuse the memory. A fresh
            // zeroed allocation often maps zero pages without writing them.
            self.entries = Vec::new();
            #[allow(
                clippy::disallowed_methods,
                reason = "the one place tables are allocated zeroed"
            )]
            {
                self.entries = alloc::vec![0; len];
            }
            len
        } else {
            let dirty = if self.zeroed { 0 } else { old.min(len) };
            self.entries.truncate(len);
            #[allow(clippy::disallowed_methods, reason = "the one place tables are zeroed")]
            self.entries[..dirty].fill(0);
            self.entries.resize(len, 0);
            dirty + len.saturating_sub(old)
        };
        self.zeroed = true;
        #[cfg(test)]
        audit::zeroed(self.id, len, zeroed, false);
        #[cfg(not(test))]
        let _ = zeroed;
    }

    /// Zeroes a dirty table through `zero_slots`, which must zero every
    /// entry a position was stored in and return how many entries it wrote.
    /// For tables that took few positions, this beats a whole clear. Tests
    /// check that no entry escaped.
    pub(crate) fn clear_slots(&mut self, zero_slots: impl FnOnce(&mut [u32]) -> usize) {
        if self.zeroed {
            return;
        }
        let written = zero_slots(&mut self.entries);
        #[cfg(test)]
        {
            assert!(
                self.entries.iter().all(|&e| e == 0),
                "a stored position escaped the slot clear"
            );
            audit::zeroed(self.id, self.entries.len(), written, true);
        }
        #[cfg(not(test))]
        let _ = written;
        self.zeroed = true;
    }

    /// [`clear`](Self::clear), then the entries for writing.
    pub(crate) fn cleared(&mut self, len: usize) -> &mut [u32] {
        self.clear(len);
        self
    }

    /// Replaces the entries with a copy of `src`.
    pub(crate) fn copy_from(&mut self, src: &[u32]) {
        self.entries.clear();
        self.entries.extend_from_slice(src);
        self.zeroed = false;
    }

    pub(crate) fn into_vec(self) -> Vec<u32> {
        self.entries
    }
}

impl Deref for MatchTable {
    type Target = [u32];

    #[inline(always)]
    fn deref(&self) -> &[u32] {
        &self.entries
    }
}

impl DerefMut for MatchTable {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut [u32] {
        self.zeroed = false;
        &mut self.entries
    }
}

/// Records what tables zero, per thread, for the zeroing tests.
#[cfg(test)]
pub(crate) mod audit {
    extern crate std;

    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::cell::RefCell;
    use std::vec::Vec;

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

    std::thread_local! {
        static LOG: RefCell<Option<Vec<Zeroing>>> = const { RefCell::new(None) };
    }

    /// One clear that wrote zeros.
    #[derive(Clone, Copy, Debug)]
    pub(crate) struct Zeroing {
        pub table: usize,
        /// The table's length after the clear.
        pub len: usize,
        /// Entries zeroed, by the allocator or by writes.
        pub entries: usize,
        /// A [`clear_slots`](super::MatchTable::clear_slots) instead of a
        /// whole [`clear`](super::MatchTable::clear).
        pub slots: bool,
    }

    pub(super) fn next_id() -> usize {
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    }

    pub(super) fn zeroed(table: usize, len: usize, entries: usize, slots: bool) {
        if entries == 0 {
            return;
        }
        LOG.with_borrow_mut(|log| {
            if let Some(log) = log {
                log.push(Zeroing {
                    table,
                    len,
                    entries,
                    slots,
                });
            }
        });
    }

    /// Runs `f` and returns every zeroing it did on this thread.
    pub(crate) fn record<R>(f: impl FnOnce() -> R) -> (R, Vec<Zeroing>) {
        LOG.with_borrow_mut(|log| *log = Some(Vec::new()));
        let result = f();
        let log = LOG.with_borrow_mut(Option::take).expect("recording");
        (result, log)
    }
}

#[cfg(test)]
mod tests {
    use super::{MatchTable, audit};

    fn zeroed_entries(f: impl FnOnce()) -> usize {
        audit::record(f).1.iter().map(|z| z.entries).sum()
    }

    #[test]
    fn clear_skips_a_zeroed_table() {
        let mut t = MatchTable::new();
        assert_eq!(zeroed_entries(|| t.clear(1024)), 1024);
        assert_eq!(zeroed_entries(|| t.clear(1024)), 0);
        assert_eq!(zeroed_entries(|| t.clear(512)), 0);
        assert_eq!(zeroed_entries(|| t.clear(1024)), 512);
        assert!(t.iter().all(|&e| e == 0));
    }

    #[test]
    fn clear_zeroes_a_written_table_once() {
        let mut t = MatchTable::zeroed(1024);
        t[7] = 9;
        assert_eq!(zeroed_entries(|| t.clear(1024)), 1024);
        assert!(t.iter().all(|&e| e == 0));
        t[7] = 9;
        assert_eq!(zeroed_entries(|| t.clear(256)), 256);
        assert_eq!(t.len(), 256);
        assert!(t.iter().all(|&e| e == 0));
        t.copy_from(&[1, 2, 3]);
        assert_eq!(zeroed_entries(|| t.clear(4096)), 4096);
        assert!(t.iter().all(|&e| e == 0));
    }
}

/// Every encoder entry point zeroes each match table at most once per frame,
/// and a frame without a dictionary zeroes exactly the tables its parameters
/// use, whether its encoder is new or reused.
#[cfg(test)]
mod per_frame {
    use super::audit;
    use crate::context::CompressContext;
    use crate::push::{EndOp, PushEncoder};
    use crate::strategy::{self, LevelParams, Strategy};
    use alloc::collections::BTreeMap;
    use alloc::format;
    use alloc::vec::Vec;
    use zrip_core::dict::Dictionary;

    const LEVELS: [i32; 6] = [-7, -3, 1, 2, 3, 4];
    /// One block, and more than one.
    const SIZES: [usize; 2] = [1024, 200 * 1024];

    /// Words from a small vocabulary with some noise.
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
        let fixture = include_bytes!("../../../tests/fixtures/corpus_dict_roundtrip.bin");
        let len = u32::from_le_bytes([fixture[0], fixture[1], fixture[2], fixture[3]]) as usize;
        Dictionary::from_bytes(&fixture[4..4 + len]).unwrap()
    }

    fn table_bytes(params: &LevelParams) -> usize {
        match params.strategy {
            Strategy::Fast => 4 << params.hash_log,
            Strategy::DFast => (4 << params.chain_log) + (4 << params.hash_log),
        }
    }

    /// Runs `f`, checks that its whole clears zeroed no table for more than
    /// its length, and returns the bytes whole clears zeroed and the entries
    /// slot clears wrote.
    fn frame_slots(what: &str, f: impl FnOnce()) -> (usize, usize) {
        let ((), log) = audit::record(f);
        // Per table: entries zeroed by whole clears, largest length.
        let mut tables = BTreeMap::<usize, (usize, usize)>::new();
        let mut slots = 0;
        for z in &log {
            if z.slots {
                slots += z.entries;
                continue;
            }
            let t = tables.entry(z.table).or_default();
            t.0 += z.entries;
            t.1 = t.1.max(z.len);
        }
        for (entries, len) in tables.values() {
            assert!(
                entries <= len,
                "{what}: zeroed {entries} entries of a {len}-entry table"
            );
        }
        (tables.values().map(|(entries, _)| entries * 4).sum(), slots)
    }

    /// [`frame_slots`] for paths that never clear slots.
    fn frame(what: &str, f: impl FnOnce()) -> usize {
        let (whole, slots) = frame_slots(what, f);
        assert_eq!(slots, 0, "{what}: slot clear");
        whole
    }

    #[test]
    fn one_shot() {
        for level in LEVELS {
            for size in SIZES {
                let input = sample(size);
                let params = strategy::level_params_for_size(level, size).unwrap();
                let what = format!("compress L{level} {size} B");
                let zeroed = frame(&what, || {
                    crate::compress(&input, level).unwrap();
                });
                assert_eq!(zeroed, table_bytes(&params), "{what}");
            }
        }
    }

    #[test]
    fn context() {
        for level in LEVELS {
            for size in SIZES {
                let input = sample(size);
                let params = strategy::level_params_for_size(level, size).unwrap();
                let mut ctx = CompressContext::new(level).unwrap();
                for n in 0..2 {
                    let what = format!("context frame {n} L{level} {size} B");
                    let zeroed = frame(&what, || {
                        ctx.compress(&input).unwrap();
                    });
                    assert_eq!(zeroed, table_bytes(&params), "{what}");
                }
            }
        }
    }

    #[test]
    fn push_encoder() {
        for level in LEVELS {
            for size in SIZES {
                let input = sample(size);
                let mut enc = PushEncoder::new(level).unwrap();
                let mut out = Vec::new();
                // A flush sends a block before the end, so the frame takes the
                // block path with tables sized for the level. The first frame
                // gets fresh tables. A later one clears the slots of a small
                // frame before it, else the whole tables.
                let blocks = strategy::level_params(level).unwrap();
                let tables = match blocks.strategy {
                    Strategy::Fast => 1,
                    Strategy::DFast => 2,
                };
                let entries = table_bytes(&blocks) / 4;
                let slot_clear = crate::frame_state::slot_clear_pays(size, tables, entries);
                for n in 0..3 {
                    let what = format!("push frame {n} with flush L{level} {size} B");
                    let (whole, slots) = frame_slots(&what, || {
                        enc.encode_to_vec(&input, &mut out, EndOp::Continue)
                            .unwrap();
                        enc.encode_to_vec(&[], &mut out, EndOp::Flush).unwrap();
                        enc.encode_to_vec(&[], &mut out, EndOp::End).unwrap();
                    });
                    if n > 0 && slot_clear {
                        assert_eq!(whole, 0, "{what}");
                        assert!(slots > 0, "{what}");
                    } else {
                        assert_eq!(whole, table_bytes(&blocks), "{what}");
                        assert_eq!(slots, 0, "{what}");
                    }
                }
                // A frame that ends before any block went out takes one pass
                // with tables sized for the input.
                let one_pass = strategy::level_params_for_size(level, size).unwrap();
                let what = format!("push frame with end L{level} {size} B");
                let zeroed = frame(&what, || {
                    enc.encode_to_vec(&input, &mut out, EndOp::End).unwrap();
                });
                assert_eq!(zeroed, table_bytes(&one_pass), "{what}");
            }
        }
    }

    #[cfg(feature = "std")]
    #[test]
    fn frame_encoder() {
        use std::io::Write;
        for level in LEVELS {
            for size in SIZES {
                let input = sample(size);
                let params = strategy::level_params(level).unwrap();
                let what = format!("frame encoder L{level} {size} B");
                let zeroed = frame(&what, || {
                    let mut enc = crate::streaming::FrameEncoder::new(Vec::new(), level).unwrap();
                    enc.write_all(&input).unwrap();
                    enc.finish().unwrap();
                });
                assert_eq!(zeroed, table_bytes(&params), "{what}");
            }
        }
    }

    #[test]
    fn dictionary() {
        let dict = dict();
        for level in LEVELS {
            for size in SIZES {
                let input = sample(size);
                let at = format!("L{level} {size} B");
                frame(&format!("compress_with_dict {at}"), || {
                    crate::compress_with_dict(&input, level, &dict).unwrap();
                });

                let mut ctx = None;
                let zeroed = frame(&format!("context with_dict {at}"), || {
                    ctx = Some(CompressContext::with_dict(level, dict.clone()).unwrap());
                });
                // The frames that use the dictionary tables build them.
                assert_eq!(zeroed, 0, "context with_dict {at}");
                let ctx = ctx.as_mut().unwrap();
                for n in 0..2 {
                    frame(&format!("context with dict, frame {n} {at}"), || {
                        ctx.compress(&input).unwrap();
                    });
                }

                let mut ctx = CompressContext::new(level).unwrap();
                for n in 0..2 {
                    frame(&format!("context compress_with_dict {n} {at}"), || {
                        ctx.compress_with_dict(&input, &dict).unwrap();
                    });
                }

                let mut enc = PushEncoder::with_dict(level, dict.clone()).unwrap();
                let mut out = Vec::new();
                for n in 0..3 {
                    frame_slots(&format!("push with dict, frame {n} {at}"), || {
                        enc.encode_to_vec(&input, &mut out, EndOp::Continue)
                            .unwrap();
                        enc.encode_to_vec(&[], &mut out, EndOp::Flush).unwrap();
                        enc.encode_to_vec(&[], &mut out, EndOp::End).unwrap();
                    });
                }
            }
        }
    }

    /// Encodes `input` as one frame with a flush before the end.
    fn push_frame(enc: &mut PushEncoder, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        enc.encode_to_vec(input, &mut out, EndOp::Continue).unwrap();
        enc.encode_to_vec(&[], &mut out, EndOp::Flush).unwrap();
        enc.encode_to_vec(&[], &mut out, EndOp::End).unwrap();
        out
    }

    /// A reused encoder, which clears slots or whole tables between frames,
    /// writes the same frames as a new one.
    #[test]
    fn reused_encoders_match_new_ones() {
        // Small and large frames in turn, so slot and whole clears follow
        // frames of either kind. Frame `i` uses a shifted copy of the sample
        // so the frames hash to different slots.
        const SEQUENCE: [usize; 7] = [1024, 300, 8 << 10, 200 << 10, 2048, 64 << 10, 700];
        let base = sample(256 << 10);
        let inputs: Vec<&[u8]> = SEQUENCE
            .iter()
            .enumerate()
            .map(|(i, &size)| &base[i * 997..i * 997 + size])
            .collect();
        let dict = dict();
        for level in LEVELS {
            // The dictionary tables are built by the first frame that uses
            // each, after other frames left their input in the buffer.
            let mut reused_ctx = CompressContext::with_dict(level, dict.clone()).unwrap();
            for (i, input) in inputs.iter().enumerate() {
                let mut fresh = CompressContext::with_dict(level, dict.clone()).unwrap();
                assert_eq!(
                    reused_ctx.compress(input).unwrap(),
                    fresh.compress(input).unwrap(),
                    "L{level} dict context frame {i}"
                );
            }

            let mut reused = PushEncoder::new(level).unwrap();
            let mut reused_dict = PushEncoder::with_dict(level, dict.clone()).unwrap();
            for (i, input) in inputs.iter().enumerate() {
                let fresh = push_frame(&mut PushEncoder::new(level).unwrap(), input);
                assert_eq!(push_frame(&mut reused, input), fresh, "L{level} frame {i}");
                let fresh = push_frame(
                    &mut PushEncoder::with_dict(level, dict.clone()).unwrap(),
                    input,
                );
                assert_eq!(
                    push_frame(&mut reused_dict, input),
                    fresh,
                    "L{level} dict frame {i}"
                );
            }
            #[cfg(feature = "std")]
            {
                use std::io::Write;
                let mut reused = crate::streaming::FrameEncoder::new(Vec::new(), level).unwrap();
                for (i, input) in inputs.iter().enumerate() {
                    let mut fresh = crate::streaming::FrameEncoder::new(Vec::new(), level).unwrap();
                    fresh.write_all(input).unwrap();
                    let fresh = fresh.finish().unwrap();
                    reused.write_all(input).unwrap();
                    let frame = reused.reset(Vec::new()).unwrap();
                    assert_eq!(frame, fresh, "L{level} frame encoder frame {i}");
                }
            }
        }
    }
}
