#![forbid(unsafe_code)]

/// Match-finding strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Single hash table (levels -8 through 2).
    Fast,
    /// Short + long hash tables (levels 3-4).
    DFast,
}

/// Parameters for Long Distance Matching.
#[derive(Debug, Clone, Copy)]
pub struct LdmParams {
    pub hash_log: u32,
    pub bucket_size_log: u32,
    pub min_match_length: u32,
    pub hash_rate_log: u32,
}

impl LdmParams {
    pub fn default_for_window_log(window_log: u32) -> Self {
        let hash_log = 20u32.min(window_log.saturating_sub(1));
        let hash_rate_log = window_log.saturating_sub(hash_log).max(7);
        Self {
            hash_log,
            bucket_size_log: 4,
            min_match_length: 64,
            hash_rate_log,
        }
    }
}

/// Compression parameters for a specific level.
///
/// Obtain via [`level_params`] or construct directly for custom tuning.
/// Pass to [`compress_with_params`](crate::compress_with_params).
#[derive(Debug, Clone, Copy)]
pub struct LevelParams {
    pub strategy: Strategy,
    pub window_log: u32,
    pub hash_log: u32,
    /// DFast short table log. Same as hashLog for Fast strategy.
    pub chain_log: u32,
    pub search_log: u32,
    pub min_match: u32,
    pub target_length: u32,
    pub search_strength: u32,
    pub force_raw_literals: bool,
    #[cfg(feature = "ldm")]
    pub ldm_params: Option<LdmParams>,
}

impl LevelParams {
    #[must_use]
    pub fn with_window_log(mut self, window_log: u32) -> Self {
        self.window_log = window_log;
        self
    }

    #[cfg(feature = "ldm")]
    #[must_use]
    pub fn with_ldm(mut self, params: LdmParams) -> Self {
        self.ldm_params = Some(params);
        self
    }
}

/// Default compression level used when level 0 is requested.
pub const DEFAULT_LEVEL: i32 = 1;

/// Returns the compression parameters for a given level, or `None` if out of range.
///
/// Level 0 is treated as "library default" and maps to level 1.
/// Uses the large-input (>256 KB) parameter tier.
pub fn level_params(level: i32) -> Option<LevelParams> {
    level_params_for_size(level, usize::MAX)
}

/// Returns the compression parameters for a given level, sized for `src_len`.
///
/// Uses fixed parameters per level with log values clamped down for small inputs.
///
/// Level 0 is treated as "library default" and maps to level 1.
pub fn level_params_for_size(level: i32, src_len: usize) -> Option<LevelParams> {
    let mut params = level_params_inner(level)?;
    params.hash_log = params.hash_log.clamp(HASH_LOG_MIN, HASH_LOG_MAX);
    params.chain_log = params.chain_log.clamp(HASH_LOG_MIN, HASH_LOG_MAX);
    params.window_log = params.window_log.clamp(WINDOW_LOG_MIN, WINDOW_LOG_MAX);
    if (2..usize::MAX).contains(&src_len) {
        let src_log = 32 - ((src_len as u32) - 1).leading_zeros();
        params.hash_log = params.hash_log.min(src_log).max(HASH_LOG_MIN);
        params.chain_log = params.chain_log.min(src_log).max(HASH_LOG_MIN);
        params.window_log = params.window_log.min(src_log);
    }
    if raw_literals_limit(level).is_some_and(|max| src_len <= max) {
        params.force_raw_literals = true;
        if let Some((target_length, min_match, search_strength)) = small_input_search(level) {
            params.target_length = target_length;
            params.min_match = min_match;
            params.search_strength = search_strength;
        }
    }
    if level == 3 && (32 * 1024..=128 * 1024).contains(&src_len) {
        params.search_strength = 7;
    }
    Some(params)
}

pub const HASH_LOG_MIN: u32 = 6;
pub const HASH_LOG_MAX: u32 = 30;
pub const WINDOW_LOG_MIN: u32 = 10;
pub const WINDOW_LOG_MAX: u32 = 27;

/// Largest input a negative level encodes with raw literals.
///
/// Building a Huffman table costs a few microseconds regardless of input
/// size. For tiny messages that dominates the encode time, so the negative
/// levels skip it: 2 KiB at L-1, growing 1.5x or 1.33x per level (the size
/// doubles every two levels) to 24 KiB at L-8. `None` for levels that always
/// try Huffman literals.
pub(crate) fn raw_literals_limit(level: i32) -> Option<usize> {
    if !(-8..=-1).contains(&level) {
        return None;
    }
    let rank = (-level - 1) as usize;
    let base = 2048usize << (rank / 2);
    Some(if rank % 2 == 1 { base + base / 2 } else { base })
}

/// Match search for inputs that keep raw literals, as `(target_length,
/// min_match, search_strength)`.
///
/// Without Huffman literals, matches are the only compression, so the
/// negative levels search more densely on these inputs than on large ones.
/// L-8 is a slightly sparser L-7. L-4 shares its step with L-3 and L-2 but
/// grows it faster through regions without matches.
fn small_input_search(level: i32) -> Option<(u32, u32, u32)> {
    Some(match level {
        -8 => (8, 5, 5),
        -7 => (5, 5, 6),
        -6 => (4, 5, 6),
        -5 => (3, 5, 6),
        -4 => (2, 5, 6),
        -3 | -2 => (2, 5, 7),
        -1 => (1, 5, 7),
        _ => return None,
    })
}

/// Whether an input of `input_len` bytes keeps raw literals at these
/// parameters.
pub(crate) fn keeps_raw_literals(params: &LevelParams, _input_len: usize) -> bool {
    params.force_raw_literals
}

/// Per-block encoding choices derived from the level parameters.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BlockPolicy {
    /// Try Huffman literals and custom sequence tables.
    pub custom_tables: bool,
    /// Literals whose sampled order-0 entropy exceeds this many bits per byte
    /// (8.8 fixed point) stay raw. `u32::MAX` disables the check.
    pub max_literal_entropy_fp8: u32,
}

/// Encoding choices for a block of an input of `input_len` bytes.
///
/// The fast negative levels skip Huffman coding for dense literals: on
/// low-compressibility data it costs more time than it saves space. L-8 to
/// L-1 keep literals above 6.25 bits per byte raw. Small inputs keep all
/// literals raw (see `raw_literals_limit`).
pub(crate) fn block_policy(params: &LevelParams, input_len: usize) -> BlockPolicy {
    let max_literal_entropy_fp8 = if params.strategy == Strategy::Fast && params.min_match >= 5 {
        match params.target_length {
            0 | 1 => u32::MAX,
            _ => 6 * 256 + 64,
        }
    } else {
        u32::MAX
    };
    BlockPolicy {
        custom_tables: use_custom_sequence_tables(params, input_len),
        max_literal_entropy_fp8,
    }
}

fn use_custom_sequence_tables(params: &LevelParams, input_len: usize) -> bool {
    // Raw-literal inputs also skip custom sequence tables.
    if keeps_raw_literals(params, input_len) {
        return false;
    }

    if (32768..=zrip_core::frame::MAX_BLOCK_SIZE).contains(&input_len)
        && params.strategy == Strategy::DFast
        && params.min_match == 4
        && params.target_length == 1
        && params.search_strength < 5
    {
        return false;
    }
    true
}

/// Returns the maximum hash_log for a given level.
/// Used by CompressContext to pre-allocate hash tables.
pub fn max_hash_log(level: i32) -> Option<u32> {
    let p = level_params_inner(level)?;
    Some(p.hash_log.max(p.chain_log))
}

fn level_params_inner(level: i32) -> Option<LevelParams> {
    Some(match level {
        0 => return level_params_inner(DEFAULT_LEVEL),
        // zrip's own level: L-7 with a larger step and faster acceleration
        // through regions without matches.
        -8 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 13,
            chain_log: 13,
            search_log: 0,
            min_match: 6,
            target_length: 21,
            search_strength: 5,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        -7 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 13,
            chain_log: 13,
            search_log: 0,
            min_match: 6,
            target_length: 17,
            search_strength: 7,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        -6 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 13,
            chain_log: 13,
            search_log: 0,
            min_match: 6,
            target_length: 13,
            search_strength: 7,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        -5 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 13,
            chain_log: 13,
            search_log: 0,
            min_match: 6,
            target_length: 9,
            search_strength: 7,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        -4 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 13,
            chain_log: 13,
            search_log: 0,
            min_match: 6,
            target_length: 5,
            search_strength: 7,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        -3 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 14,
            chain_log: 14,
            search_log: 0,
            min_match: 6,
            target_length: 4,
            search_strength: 7,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        -2 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 14,
            chain_log: 14,
            search_log: 0,
            min_match: 6,
            target_length: 3,
            search_strength: 7,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        -1 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 14,
            chain_log: 14,
            search_log: 0,
            min_match: 6,
            target_length: 2,
            search_strength: 7,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        1 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 15,
            chain_log: 15,
            search_log: 0,
            min_match: 6,
            target_length: 1,
            search_strength: 7,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        2 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 20,
            hash_log: 16,
            chain_log: 16,
            search_log: 0,
            min_match: 6,
            target_length: 1,
            search_strength: 7,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        3 => LevelParams {
            strategy: Strategy::DFast,
            window_log: 21,
            hash_log: 18,
            chain_log: 18,
            search_log: 1,
            min_match: 4,
            target_length: 1,
            search_strength: 5,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        4 => LevelParams {
            strategy: Strategy::DFast,
            window_log: 24,
            hash_log: 20,
            chain_log: 20,
            search_log: 0,
            min_match: 4,
            target_length: 1,
            search_strength: 8,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        _ => return None,
    })
}

/// Options for large-window and LDM compression, orthogonal to level.
///
/// Pass to [`compress_opts`](crate::compress_opts) or
/// [`FrameEncoder::with_options`](crate::streaming::FrameEncoder::with_options).
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub(crate) window_log: Option<u32>,
    #[cfg_attr(not(feature = "ldm"), allow(dead_code))]
    pub(crate) ldm: bool,
}

impl Options {
    #[must_use]
    pub fn window_log(mut self, log: u32) -> Self {
        self.window_log = Some(log);
        self
    }

    #[cfg(feature = "ldm")]
    #[must_use]
    pub fn ldm(mut self, enable: bool) -> Self {
        self.ldm = enable;
        self
    }
}

pub fn apply_options(params: &mut LevelParams, opts: &Options) {
    if let Some(wl) = opts.window_log {
        params.window_log = wl.clamp(WINDOW_LOG_MIN, WINDOW_LOG_MAX);
    }
    #[cfg(feature = "ldm")]
    if opts.ldm {
        params.ldm_params = Some(LdmParams::default_for_window_log(params.window_log));
    }
}
