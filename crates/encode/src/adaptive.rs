//! Compression level adaptation for a sender whose bottleneck moves.
//!
//! The caller compresses in windows, records each frame's work in
//! [`CompressStats`], and names what limited the window, like C zstd's
//! `--adapt`. A blocked output or idle input asks for ratio. Queued input
//! with free output asks for compression speed. The level walks one step at
//! a time toward the goal, crossing small dips, and reverts a step that
//! clearly hurts. Neither ratio nor speed is monotonic in the level for every
//! input, so a finished walk settles on the best recently measured level.
//!
//! Speed is input bytes per second spent compressing. End-to-end throughput
//! varies too much between short windows to rank levels.
//!
//! ```
//! use core::time::Duration;
//! use zrip::adaptive::{AdaptiveLevel, Bottleneck, CompressStats};
//!
//! let mut adaptive = AdaptiveLevel::new(-7..=4, -1);
//! let mut stats = CompressStats::default();
//! // After each frame: input bytes, frame bytes, and time spent compressing.
//! stats.record(64 * 1024, 20 * 1024, Duration::from_micros(300));
//! // After each window of at least `MIN_WINDOW_BYTES` input:
//! let level = adaptive.update(stats, Some(Bottleneck::Output));
//! assert_eq!(level, 1);
//! ```

use core::ops::RangeInclusive;
use core::time::Duration;

use crate::strategy::{DEFAULT_LEVEL, MAX_LEVEL, MIN_LEVEL};

/// Input a window should cover at least. Smaller windows rank levels by
/// measurement noise.
pub const MIN_WINDOW_BYTES: u64 = 256 * 1024;

/// Distinct zrip levels: -7 to 4 without 0.
const LADDER: usize = 11;
/// Windows a limit holds and a measurement stays fresh.
const RETRY_WINDOWS: u64 = 32;
/// A step toward ratio may lose up to 2% of it to cross a dip.
const RATIO_STEP: (u128, u128) = (102, 100);
/// Ratios within 0.5% tie; the lower level wins.
const RATIO_TIE: (u128, u128) = (1005, 1000);
/// A step toward speed may lose up to 10% of it to cross a plateau.
const SPEED_STEP: (u128, u128) = (90, 100);
/// Speeds within 5% tie; the better ratio wins.
const SPEED_TIE: (u128, u128) = (95, 100);

/// Compression work done in one window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompressStats {
    /// Input bytes.
    pub plain: u64,
    /// Compressed bytes.
    pub compressed: u64,
    /// Time spent compressing, summed over threads.
    pub busy: Duration,
}

impl CompressStats {
    /// Adds one frame.
    pub fn record(&mut self, plain: usize, compressed: usize, busy: Duration) {
        self.plain += plain as u64;
        self.compressed += compressed as u64;
        self.busy += busy;
    }

    /// Adds work recorded elsewhere, such as on another thread.
    pub fn merge(&mut self, other: Self) {
        self.plain += other.plain;
        self.compressed += other.compressed;
        self.busy += other.busy;
    }
}

/// What limited the sender during one window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bottleneck {
    /// The output refused bytes. Compress harder.
    Output,
    /// Input waited while the output kept up. Compress faster.
    Cpu,
    /// Nothing waited. Spare time goes to ratio.
    Input,
}

impl Bottleneck {
    /// Blocked output or spare time asks for ratio; a busy CPU for speed.
    fn goal(self) -> Goal {
        match self {
            Self::Output | Self::Input => Goal::Ratio,
            Self::Cpu => Goal::Speed,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Goal {
    Ratio,
    Speed,
}

impl Goal {
    /// A step fails when it clearly loses its goal, or when it moves the
    /// bottleneck to the other side: CPU after a ratio step, output after a
    /// speed step.
    fn kept(self, previous: Sample, current: Sample, bottleneck: Option<Bottleneck>) -> bool {
        match self {
            Self::Ratio => {
                bottleneck != Some(Bottleneck::Cpu) && current.ratio_within(previous, RATIO_STEP)
            }
            Self::Speed => {
                bottleneck != Some(Bottleneck::Output) && current.speed_within(previous, SPEED_STEP)
            }
        }
    }
}

/// Compression at one level, decayed by half per window at that level.
#[derive(Clone, Copy, Debug)]
struct Sample {
    plain: u128,
    compressed: u128,
    busy: u128,
    window: u64,
}

impl Sample {
    fn new(stats: CompressStats, window: u64) -> Option<Self> {
        let busy = stats.busy.as_nanos();
        (stats.plain > 0 && busy > 0).then(|| Self {
            plain: u128::from(stats.plain),
            compressed: u128::from(stats.compressed),
            busy,
            window,
        })
    }

    fn decay_into(self, older: Self) -> Self {
        Self {
            plain: older.plain / 2 + self.plain,
            compressed: older.compressed / 2 + self.compressed,
            busy: older.busy / 2 + self.busy,
            window: self.window,
        }
    }

    /// Compressed bytes per input byte at most `num / den` times `other`'s.
    fn ratio_within(self, other: Self, (num, den): (u128, u128)) -> bool {
        self.compressed * other.plain * den <= other.compressed * self.plain * num
    }

    /// Speed at least `num / den` times `other`'s.
    fn speed_within(self, other: Self, (num, den): (u128, u128)) -> bool {
        self.plain * other.busy * den >= other.plain * self.busy * num
    }

    fn denser(self, other: Self) -> bool {
        self.compressed * other.plain < other.compressed * self.plain
    }

    fn faster(self, other: Self) -> bool {
        self.plain * other.busy > other.plain * self.busy
    }
}

/// A finished walk blocks steps past `index` until window `until`.
#[derive(Clone, Copy, Debug)]
struct Limit {
    index: usize,
    until: u64,
}

/// Hill climb over the distinct zrip levels of one range.
#[derive(Clone, Debug)]
pub struct AdaptiveLevel {
    levels: [i32; LADDER],
    len: usize,
    samples: [Option<Sample>; LADDER],
    index: usize,
    /// The step under test: the index it left, and its goal.
    probe: Option<(usize, Goal)>,
    /// Highest index a step toward ratio may reach while it holds.
    ceiling: Option<Limit>,
    /// Lowest index a step toward speed may reach while it holds.
    floor: Option<Limit>,
    /// Windows seen so far.
    window: u64,
}

impl AdaptiveLevel {
    /// Walks the distinct levels in `range`, starting at the first level not
    /// below `start`, or at the highest. As in [`CompressContext`], level -8
    /// runs as -7 and level 0 as the default level.
    ///
    /// # Panics
    ///
    /// If `range` holds no zrip level.
    ///
    /// [`CompressContext`]: crate::context::CompressContext
    pub fn new(range: RangeInclusive<i32>, start: i32) -> Self {
        let mut levels = [0; LADDER];
        let mut len = 0;
        for level in (MIN_LEVEL..=MAX_LEVEL).filter(|level| *level != 0) {
            if range.contains(&level)
                || (level == MIN_LEVEL && range.contains(&-8))
                || (level == DEFAULT_LEVEL && range.contains(&0))
            {
                levels[len] = level;
                len += 1;
            }
        }
        assert!(len > 0, "{range:?} holds no zrip level");
        let index = levels[..len]
            .iter()
            .position(|level| *level >= start)
            .unwrap_or(len - 1);

        Self {
            levels,
            len,
            samples: [None; LADDER],
            index,
            probe: None,
            ceiling: None,
            floor: None,
            window: 0,
        }
    }

    /// The level to compress at.
    pub fn level(&self) -> i32 {
        self.levels[self.index]
    }

    /// Records one window at the current level and returns the next level.
    /// `bottleneck` is `None` when the window shows no clear one; the level
    /// then holds, though a pending step can still be reverted.
    pub fn update(&mut self, stats: CompressStats, bottleneck: Option<Bottleneck>) -> i32 {
        self.window += 1;
        let Some(mut sample) = Sample::new(stats, self.window) else {
            return self.level();
        };
        if let Some(older) = self.fresh(self.index) {
            sample = sample.decay_into(older);
        }
        self.samples[self.index] = Some(sample);

        if let Some((from, goal)) = self.probe.take()
            && let Some(previous) = self.samples[from]
            && !goal.kept(previous, sample, bottleneck)
        {
            self.set_limit(goal, from);
            self.index = from;
            return self.level();
        }

        let Some(goal) = bottleneck.map(Bottleneck::goal) else {
            return self.level();
        };
        if let Some(next) = self.next(goal) {
            self.probe = Some((self.index, goal));
            self.index = next;
        } else {
            let settled = self.settle(goal);
            if settled != self.index || self.limit(goal).is_none() {
                self.set_limit(goal, settled);
            }
            self.index = settled;
        }
        self.level()
    }

    /// The sample at `index`, if taken within the last `RETRY_WINDOWS`.
    fn fresh(&self, index: usize) -> Option<Sample> {
        self.samples[index].filter(|sample| self.window - sample.window < RETRY_WINDOWS)
    }

    fn limit(&self, goal: Goal) -> Option<Limit> {
        let limit = match goal {
            Goal::Ratio => self.ceiling,
            Goal::Speed => self.floor,
        };
        limit.filter(|limit| self.window < limit.until)
    }

    fn set_limit(&mut self, goal: Goal, index: usize) {
        let limit = Some(Limit {
            index,
            until: self.window + RETRY_WINDOWS,
        });
        match goal {
            Goal::Ratio => self.ceiling = limit,
            Goal::Speed => self.floor = limit,
        }
    }

    /// The next index toward `goal`, unless at the end of the ladder or past
    /// its limit.
    fn next(&self, goal: Goal) -> Option<usize> {
        let limit = self.limit(goal);
        match goal {
            Goal::Ratio => {
                let next = self.index + 1;
                let blocked = limit.is_some_and(|limit| next > limit.index);
                (next < self.len && !blocked).then_some(next)
            }
            Goal::Speed => {
                let next = self.index.checked_sub(1)?;
                let blocked = limit.is_some_and(|limit| next < limit.index);
                (!blocked).then_some(next)
            }
        }
    }

    /// Best fresh level for `goal` within its limit: the lowest of the
    /// densest levels for ratio, the densest of the fastest levels for speed.
    fn settle(&self, goal: Goal) -> usize {
        let limit = self.limit(goal);
        let fresh = || {
            (0..self.len)
                .filter(move |index| match (goal, limit) {
                    (Goal::Ratio, Some(limit)) => *index <= limit.index,
                    (Goal::Speed, Some(limit)) => *index >= limit.index,
                    (_, None) => true,
                })
                .filter_map(|index| Some((index, self.fresh(index)?)))
        };
        let best = |better: fn(Sample, Sample) -> bool| {
            fresh()
                .map(|(_, sample)| sample)
                .reduce(|best, sample| if better(sample, best) { sample } else { best })
        };

        let settled = match goal {
            Goal::Ratio => best(Sample::denser).and_then(|densest| {
                fresh().find(|(_, sample)| sample.ratio_within(densest, RATIO_TIE))
            }),
            Goal::Speed => best(Sample::faster).and_then(|fastest| {
                fresh()
                    .filter(|(_, sample)| sample.speed_within(fastest, SPEED_TIE))
                    .reduce(|best, entry| if entry.1.denser(best.1) { entry } else { best })
            }),
        };
        settled.map_or(self.index, |(index, _)| index)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use core::cell::Cell;
    use std::vec::Vec;

    const LEVELS: [i32; 11] = [-7, -6, -5, -4, -3, -2, -1, 1, 2, 3, 4];
    // zrip on OMQ's benchmark corpus, 1 KiB messages in 32 KiB arenas:
    // compression speed in MB/s and ratio in hundredths.
    const SPEED: [u64; 11] = [286, 298, 234, 226, 221, 225, 220, 197, 197, 154, 154];
    const RATIO: [u64; 11] = [186, 255, 289, 285, 294, 322, 365, 351, 351, 336, 337];

    /// A sender compressing at `speed` MB/s per level, behind an optional
    /// output of `output` MB/s, fed at an optional `input` MB/s.
    struct Sender {
        speed: [u64; 11],
        ratio: [u64; 11],
        output: Option<u64>,
        input: Option<u64>,
        /// Alternating speed error in percent.
        jitter: u64,
        windows: Cell<u64>,
    }

    impl Sender {
        const fn cpu() -> Self {
            Self {
                speed: SPEED,
                ratio: RATIO,
                output: None,
                input: None,
                jitter: 0,
                windows: Cell::new(0),
            }
        }

        /// One 100 ms window at `level`, with the bottleneck a transport
        /// would report: output blocked for at least 1/16 of the window, or
        /// input queued while output blocked under 1/64 of it, or no queue.
        fn window(&self, level: i32) -> (CompressStats, Option<Bottleneck>) {
            let index = LEVELS.iter().position(|l| *l == level).unwrap();
            self.windows.set(self.windows.get() + 1);
            let error = if self.windows.get().is_multiple_of(2) {
                100 + self.jitter
            } else {
                100 - self.jitter
            };
            let speed = self.speed[index] * error / 100;
            let ratio = self.ratio[index];

            // Input bytes per window that the CPU, output, and input allow.
            let cpu = speed * 100_000;
            let output = self.output.map_or(u64::MAX, |output| output * 1000 * ratio);
            let input = self.input.map_or(u64::MAX, |input| input * 100_000);
            let plain = cpu.min(output).min(input);
            let blocked_us = if output < cpu.min(input) {
                100_000 - plain * 100_000 / cpu
            } else {
                0
            };
            let queued = input > plain;
            let bottleneck = if blocked_us > 0 && blocked_us * 16 >= 100_000 {
                Some(Bottleneck::Output)
            } else if queued {
                (blocked_us * 64 < 100_000).then_some(Bottleneck::Cpu)
            } else {
                Some(Bottleneck::Input)
            };
            let stats = CompressStats {
                plain,
                compressed: plain * 100 / ratio,
                busy: Duration::from_micros(plain / speed),
            };
            (stats, bottleneck)
        }

        fn run(&self, adaptive: &mut AdaptiveLevel, windows: usize) -> Vec<i32> {
            (0..windows)
                .map(|_| {
                    let (stats, bottleneck) = self.window(adaptive.level());
                    adaptive.update(stats, bottleneck)
                })
                .collect()
        }
    }

    fn settled(levels: &[i32], level: i32) -> bool {
        levels.iter().all(|l| *l == level)
    }

    #[test]
    fn ladder_skips_zero_and_runs_minus_eight_as_minus_seven() {
        let adaptive = AdaptiveLevel::new(-8..=4, -1);
        assert_eq!(&adaptive.levels[..adaptive.len], LEVELS);
        assert_eq!(adaptive.level(), -1);
        assert_eq!(AdaptiveLevel::new(1..=4, -1).level(), 1);
        assert_eq!(AdaptiveLevel::new(-7..=-3, -1).level(), -3);
        assert_eq!(AdaptiveLevel::new(0..=0, -1).level(), 1);
        assert_eq!(AdaptiveLevel::new(-8..=-8, -1).level(), -7);
    }

    #[test]
    #[should_panic(expected = "holds no zrip level")]
    fn range_without_levels_panics() {
        AdaptiveLevel::new(5..=9, 5);
    }

    #[test]
    fn slow_output_climbs_across_dips_to_the_best_ratio() {
        let sender = Sender {
            output: Some(1),
            ..Sender::cpu()
        };
        let mut adaptive = AdaptiveLevel::new(-7..=4, -7);
        let levels = sender.run(&mut adaptive, 30);
        // -4 compresses 1.4% worse than -5; 1 compresses worse than -1.
        assert_eq!(&levels[..8], &[-6, -5, -4, -3, -2, -1, 1, -1]);
        assert!(settled(&levels[8..], -1), "{levels:?}");
    }

    #[test]
    fn slow_output_climbs_to_the_top_when_it_compresses_best() {
        let sender = Sender {
            ratio: [186, 255, 289, 295, 300, 322, 365, 380, 390, 400, 410],
            output: Some(1),
            ..Sender::cpu()
        };
        let mut adaptive = AdaptiveLevel::new(-7..=4, -1);
        let levels = sender.run(&mut adaptive, 30);
        assert_eq!(&levels[..4], &[1, 2, 3, 4]);
        assert!(settled(&levels[4..], 4), "{levels:?}");
    }

    #[test]
    fn output_climb_stops_where_compression_falls_behind() {
        // Ratio improves with every level, but 3 cannot fill a 45 MB/s output.
        let sender = Sender {
            ratio: [186, 255, 289, 295, 300, 322, 365, 380, 380, 400, 410],
            output: Some(45),
            ..Sender::cpu()
        };
        let mut adaptive = AdaptiveLevel::new(-7..=4, -1);
        let levels = sender.run(&mut adaptive, 30);
        // 1 and 2 compress alike, so the lower level wins.
        assert_eq!(&levels[..5], &[1, 2, 3, 2, 1]);
        assert!(settled(&levels[5..], 1), "{levels:?}");
    }

    #[test]
    fn fast_output_drops_to_the_fastest_level_that_fits() {
        // 1 Gbps: -6 keeps up, -7 overflows the output.
        let sender = Sender {
            output: Some(125),
            ..Sender::cpu()
        };
        let mut adaptive = AdaptiveLevel::new(-7..=4, 4);
        let levels = sender.run(&mut adaptive, 30);
        assert_eq!(&levels[..11], &[3, 2, 1, -1, -2, -3, -4, -5, -6, -7, -6]);
        assert!(settled(&levels[11..], -6), "{levels:?}");
    }

    #[test]
    fn unlimited_output_prefers_the_denser_of_tied_fast_levels() {
        // -7 compresses 4% slower than -6 and much worse.
        let sender = Sender::cpu();
        let mut adaptive = AdaptiveLevel::new(-7..=4, -1);
        let levels = sender.run(&mut adaptive, 30);
        assert_eq!(&levels[..7], &[-2, -3, -4, -5, -6, -7, -6]);
        assert!(settled(&levels[7..], -6), "{levels:?}");
    }

    #[test]
    fn measurement_noise_does_not_move_a_settled_level() {
        for output in [None, Some(1), Some(125)] {
            let sender = Sender {
                output,
                jitter: 4,
                ..Sender::cpu()
            };
            let mut adaptive = AdaptiveLevel::new(-7..=4, -1);
            let levels = sender.run(&mut adaptive, 30);
            let last = levels[15];
            assert!(settled(&levels[15..], last), "{output:?}: {levels:?}");
        }
    }

    #[test]
    fn idle_input_climbs_and_unclear_windows_hold() {
        let sender = Sender {
            input: Some(10),
            ..Sender::cpu()
        };
        let mut adaptive = AdaptiveLevel::new(-3..=4, -3);
        let levels = sender.run(&mut adaptive, 12);
        assert_eq!(&levels[..4], &[-2, -1, 1, -1]);
        assert!(settled(&levels[4..], -1), "{levels:?}");

        let (stats, _) = Sender::cpu().window(-1);
        assert_eq!(adaptive.update(stats, None), -1);
    }

    #[test]
    fn limits_expire_and_the_walk_probes_again() {
        let sender = Sender {
            output: Some(1),
            ..Sender::cpu()
        };
        let mut adaptive = AdaptiveLevel::new(-2..=1, -1);
        let windows = 2 + usize::try_from(RETRY_WINDOWS).unwrap();
        let levels = sender.run(&mut adaptive, windows);
        assert_eq!(&levels[..2], &[1, -1]);
        assert!(settled(&levels[2..windows - 1], -1), "{levels:?}");
        assert_eq!(levels.last(), Some(&1));
    }

    #[test]
    fn empty_windows_keep_the_level() {
        let mut adaptive = AdaptiveLevel::new(-7..=4, -1);
        assert_eq!(
            adaptive.update(CompressStats::default(), Some(Bottleneck::Cpu)),
            -1
        );
        let (stats, _) = Sender::cpu().window(-1);
        assert_eq!(adaptive.update(stats, None), -1);
    }

    #[test]
    fn stats_record_and_merge() {
        let mut stats = CompressStats::default();
        stats.record(100, 40, Duration::from_micros(3));
        let mut other = CompressStats::default();
        other.record(50, 30, Duration::from_micros(2));
        stats.merge(other);
        assert_eq!(
            stats,
            CompressStats {
                plain: 150,
                compressed: 70,
                busy: Duration::from_micros(5),
            }
        );
    }
}
