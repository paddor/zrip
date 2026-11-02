#![forbid(unsafe_code)]

//! Keeps `u32` table positions in range on inputs past 4 GiB.

use zrip_core::frame::MAX_BLOCK_SIZE;

/// Longest stretch of input the `u32` positions in the match finder tables
/// can address.
///
/// Unit tests use a short one, so they reach a rebase with small inputs.
const MAX_SPAN: usize = if cfg!(test) {
    1 << 20
} else {
    u32::MAX as usize
};

/// Where table positions count from.
///
/// One-shot encoders search the whole input, so its positions can outgrow the
/// tables. A wrapped position looks out of the window, and the finders would
/// stop matching. When a block would pass `MAX_SPAN`, `advance` moves the base
/// up to one window behind the block, and the caller moves the table entries
/// down by the same amount. The finders then search from the base.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PositionBase {
    base: usize,
    window: usize,
}

impl PositionBase {
    pub(crate) fn new(window_log: u32) -> Self {
        Self {
            base: 0,
            window: 1usize << window_log,
        }
    }

    /// Prepares for the block at `block_start..block_end` of the input, and
    /// returns how far to move table entries down, if at all.
    ///
    /// Call it for every block, including those that skip the search, so the
    /// shift always fits a `u32`.
    #[inline]
    pub(crate) fn advance(&mut self, block_start: usize, block_end: usize) -> Option<u32> {
        if block_end - self.base <= MAX_SPAN {
            return None;
        }
        let shift = (block_start - self.base).saturating_sub(self.window);
        if shift == 0 {
            return None;
        }
        self.base += shift;
        Some(shift as u32)
    }

    /// Returns the input position that table positions count from.
    #[inline]
    pub(crate) fn base(&self) -> usize {
        self.base
    }
}

/// Moves table entries down by `shift` and clears those that fall off.
pub(crate) fn reduce_hash_table(table: &mut [u32], shift: u32) {
    for entry in table.iter_mut() {
        if *entry < shift {
            *entry = 0;
        } else {
            *entry -= shift;
        }
    }
}

/// Asserts that a search up to `block_end` stores positions the tables can
/// hold. `FrameEncoder` keeps up to two windows and a block behind its own
/// compaction, so the bound allows that much.
#[inline(always)]
pub(crate) fn debug_assert_fits(block_end: usize, window_log: u32) {
    debug_assert!(
        block_end <= MAX_SPAN.max((2usize << window_log) + MAX_BLOCK_SIZE),
        "block end {block_end} past the table span at window_log {window_log}"
    );
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::context::CompressContext;
    use crate::{compress, compress_with_dict, compress_with_params, strategy};
    use zrip_core::dict::Dictionary;

    const BLOCK: usize = MAX_BLOCK_SIZE;
    // Past two windows at L3, so a block loop that skips the rebase fails
    // `debug_assert_fits`.
    const INPUT_LEN: usize = 6 << 20;
    // L4's window covers these inputs, so `custom_window_rebases_at_l4` covers
    // that level.
    const LEVELS: [i32; 10] = [-7, -6, -5, -4, -3, -2, -1, 1, 2, 3];
    const RANDOM_SEEDS: u64 = 20;
    // Smaller windows rebase on every block, which is slow in debug builds.
    const RANDOM_MIN_WINDOW_LOG: u32 = 13;

    /// Random 4 KiB slices of a small pool. Offsets keep changing, so the
    /// ratio depends on the tables, not on one long repcode run.
    fn pool_slices(len: usize) -> Vec<u8> {
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let pool: Vec<u8> = (0..64 << 10).map(|_| b'a' + (next() >> 60) as u8).collect();
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            let at = (next() as usize) % (pool.len() - 4096);
            let n = 4096.min(len - out.len());
            out.extend_from_slice(&pool[at..at + n]);
        }
        out
    }

    /// Stretches the encoder treats differently: slices of a small pool that
    /// match, flat random bytes that skip the search, and runs of one byte.
    fn mixed(seed: u64, len: usize) -> Vec<u8> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let pool: Vec<u8> = (0..16 << 10).map(|_| b'a' + (next() >> 60) as u8).collect();
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            let n = ((next() as usize) % (256 << 10)).clamp(1, len - out.len());
            match next() % 4 {
                0 => out.extend((0..n).map(|_| next() as u8)),
                1 => out.resize(out.len() + n, next() as u8),
                _ => {
                    let end = out.len() + n;
                    while out.len() < end {
                        let at = (next() as usize) % (pool.len() - 1024);
                        let take = 1024.min(end - out.len());
                        out.extend_from_slice(&pool[at..at + take]);
                    }
                }
            }
        }
        out
    }

    /// The dictionary in the fixture, stored as `[dict_len:u32][dict_bytes]...`.
    fn dict() -> Dictionary {
        let fixture = include_bytes!("../../../tests/fixtures/corpus_dict_roundtrip.bin");
        let len = u32::from_le_bytes([fixture[0], fixture[1], fixture[2], fixture[3]]) as usize;
        Dictionary::from_bytes(&fixture[4..4 + len]).unwrap()
    }

    #[test]
    fn advance_keeps_one_window_behind_the_block() {
        let window = 1usize << 19;
        let mut positions = PositionBase::new(19);
        assert_eq!(positions.advance(0, BLOCK), None);
        assert_eq!(positions.base(), 0);

        let start = MAX_SPAN - BLOCK / 2;
        let end = start + BLOCK;
        let shift = positions.advance(start, end).unwrap() as usize;
        assert_eq!(shift, start - window);
        assert_eq!(start - positions.base(), window);

        assert_eq!(positions.advance(end, end + BLOCK), None);
    }

    #[test]
    fn advance_waits_while_the_window_covers_the_span() {
        let mut positions = PositionBase::new(27);
        assert_eq!(positions.advance(MAX_SPAN, MAX_SPAN + BLOCK), None);
        assert_eq!(positions.base(), 0);
    }

    #[test]
    fn reduce_hash_table_moves_entries_and_clears_the_rest() {
        let mut table = [0, 99, 100, 101, 500];
        reduce_hash_table(&mut table, 100);
        assert_eq!(table, [0, 0, 0, 1, 400]);
    }

    /// Asserts a frame from this build, which rebases past the short test
    /// span, matches one from the `zrip` dev-dependency, which links a normal
    /// `zrip-encode` that does not rebase at these sizes.
    fn assert_rebase_is_transparent(frame: &[u8], plain: &[u8], label: &str) {
        assert!(
            frame == plain,
            "{label}: rebased frame differs, {} vs {} bytes",
            frame.len(),
            plain.len()
        );
    }

    #[test]
    fn one_shot_frames_survive_rebases() {
        let data = pool_slices(INPUT_LEN);
        for level in LEVELS {
            let frame = compress(&data, level).unwrap();
            let plain = zrip::compress(&data, level).unwrap();
            assert_rebase_is_transparent(&frame, &plain, &format!("level {level}"));
            assert_eq!(zrip::decompress(&frame).unwrap(), data, "level {level}");
        }
    }

    #[test]
    fn custom_window_rebases_at_l4() {
        let data = pool_slices(INPUT_LEN);
        let params = strategy::level_params(4).unwrap().with_window_log(19);
        let frame = compress_with_params(&data, &params).unwrap();
        let plain_params = zrip::encode::strategy::level_params(4)
            .unwrap()
            .with_window_log(19);
        let plain = zrip::encode::compress_with_params(&data, &plain_params).unwrap();
        assert_rebase_is_transparent(&frame, &plain, "L4 window_log 19");
        assert_eq!(zrip::decompress(&frame).unwrap(), data);
    }

    #[test]
    fn random_inputs_and_windows_survive_rebases() {
        for seed in 0..RANDOM_SEEDS {
            let level = LEVELS[seed as usize % LEVELS.len()];
            let window_log =
                RANDOM_MIN_WINDOW_LOG + (seed * 7 % (21 - RANDOM_MIN_WINDOW_LOG as u64)) as u32;
            let len = (1 << 20) + (seed as usize * 131_071) % (1 << 20);
            let data = mixed(seed, len);

            let params = strategy::level_params(level)
                .unwrap()
                .with_window_log(window_log);
            let frame = compress_with_params(&data, &params).unwrap();
            let plain_params = zrip::encode::strategy::level_params(level)
                .unwrap()
                .with_window_log(window_log);
            let plain = zrip::encode::compress_with_params(&data, &plain_params).unwrap();

            let label = format!("seed {seed} level {level} window_log {window_log}");
            assert_eq!(zrip::decompress(&frame).unwrap(), data, "{label}");
            // A cleared table entry reads as position 0, which is exactly one
            // window behind the first block after a rebase. Its bytes can
            // match on repetitive data, so frames may differ a little here.
            assert!(
                frame.len() <= plain.len() + plain.len() / 100 + 64,
                "{label}: frame {} plain {}",
                frame.len(),
                plain.len()
            );
        }
    }

    #[cfg(feature = "ldm")]
    #[test]
    fn ldm_frames_survive_rebases() {
        let data = pool_slices(INPUT_LEN);
        let opts = strategy::Options::default().window_log(19).ldm(true);
        let plain_opts = zrip::Options::default().window_log(19).ldm(true);
        for level in LEVELS {
            let frame = crate::compress_opts(&data, level, &opts).unwrap();
            let plain = zrip::compress_opts(&data, level, &plain_opts).unwrap();
            // A single-segment frame lets plain LDM reach the whole input,
            // while a rebase keeps one window. Real rebases only happen past
            // 4 GiB, where the frame's window is that one window.
            assert!(
                frame.len() < plain.len() + plain.len() / 10,
                "LDM level {level}: frame {} plain {}",
                frame.len(),
                plain.len()
            );
            assert_eq!(zrip::decompress(&frame).unwrap(), data, "level {level}");
        }
    }

    /// Past 128 MiB a frame's window is `window_log`, as on a real rebase, so
    /// LDM frames match byte for byte.
    #[cfg(feature = "ldm")]
    #[test]
    #[ignore = "compresses 129 MiB per level, run with --release"]
    fn ldm_frames_past_single_segment_are_transparent() {
        let data = pool_slices(129 << 20);
        let opts = strategy::Options::default().window_log(19).ldm(true);
        let plain_opts = zrip::Options::default().window_log(19).ldm(true);
        for level in LEVELS {
            let frame = crate::compress_opts(&data, level, &opts).unwrap();
            let plain = zrip::compress_opts(&data, level, &plain_opts).unwrap();
            assert_rebase_is_transparent(&frame, &plain, &format!("LDM level {level}"));
        }
    }

    #[test]
    fn dict_frames_survive_rebases() {
        let data = pool_slices(INPUT_LEN);
        let dict = dict();
        for level in LEVELS {
            let frame = compress_with_dict(&data, level, &dict).unwrap();
            let plain = zrip::compress_with_dict(&data, level, &dict).unwrap();
            assert_rebase_is_transparent(&frame, &plain, &format!("dict level {level}"));
            let out = zrip::decompress_with_dict(&frame, &dict).unwrap();
            assert_eq!(out, data, "level {level}");
        }
    }

    #[test]
    fn context_frames_survive_rebases() {
        let data = pool_slices(INPUT_LEN);
        for level in LEVELS {
            let mut ctx = CompressContext::new(level).unwrap();
            let frame = ctx.compress(&data).unwrap();
            let mut plain_ctx = zrip::CompressContext::new(level).unwrap();
            let plain = plain_ctx.compress(&data).unwrap();
            assert_rebase_is_transparent(&frame, &plain, &format!("context level {level}"));
            assert_eq!(zrip::decompress(&frame).unwrap(), data, "level {level}");
        }
    }

    #[test]
    fn context_ad_hoc_dict_frames_survive_rebases() {
        let data = pool_slices(INPUT_LEN);
        let dict = dict();
        for level in LEVELS {
            let mut ctx = CompressContext::new(level).unwrap();
            let frame = ctx.compress_with_dict(&data, &dict).unwrap();
            let mut plain_ctx = zrip::CompressContext::new(level).unwrap();
            let plain = plain_ctx.compress_with_dict(&data, &dict).unwrap();
            assert_rebase_is_transparent(&frame, &plain, &format!("ad hoc dict level {level}"));
            let out = zrip::decompress_with_dict(&frame, &dict).unwrap();
            assert_eq!(out, data, "level {level}");
        }
    }

    #[test]
    fn context_prepared_dict_frames_survive_rebases() {
        let data = pool_slices(INPUT_LEN);
        let dict = dict();
        for level in LEVELS {
            let mut ctx =
                CompressContext::with_dict_for_size(level, dict.clone(), data.len()).unwrap();
            let frame = ctx.compress(&data).unwrap();
            let mut plain_ctx =
                zrip::CompressContext::with_dict_for_size(level, dict.clone(), data.len()).unwrap();
            let plain = plain_ctx.compress(&data).unwrap();
            assert_rebase_is_transparent(&frame, &plain, &format!("prepared dict level {level}"));
            let out = zrip::decompress_with_dict(&frame, &dict).unwrap();
            assert_eq!(out, data, "level {level}");
        }
    }
}
