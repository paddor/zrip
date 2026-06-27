#![forbid(unsafe_code)]

/// Match-finding strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Single hash table (levels -7 through 2).
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
    if (2..usize::MAX).contains(&src_len) {
        let src_log = 32 - ((src_len as u32) - 1).leading_zeros();
        params.hash_log = params.hash_log.min(src_log).max(HASH_LOG_MIN);
        params.chain_log = params.chain_log.min(src_log).max(HASH_LOG_MIN);
        params.window_log = params.window_log.min(src_log);
    }
    Some(params)
}

pub const HASH_LOG_MIN: u32 = 6;
pub const HASH_LOG_MAX: u32 = 30;

/// Returns the maximum hash_log for a given level.
/// Used by CompressContext to pre-allocate hash tables.
pub fn max_hash_log(level: i32) -> Option<u32> {
    let p = level_params_inner(level)?;
    Some(p.hash_log.max(p.chain_log))
}

fn level_params_inner(level: i32) -> Option<LevelParams> {
    Some(match level {
        0 => return level_params_inner(DEFAULT_LEVEL),
        -7 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 13,
            chain_log: 13,
            search_log: 0,
            min_match: 5,
            target_length: 7,
            search_strength: 7,
            force_raw_literals: true,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        -6 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 13,
            chain_log: 13,
            search_log: 0,
            min_match: 5,
            target_length: 7,
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
            min_match: 5,
            target_length: 6,
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
            min_match: 5,
            target_length: 5,
            search_strength: 7,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        -3 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 13,
            chain_log: 13,
            search_log: 0,
            min_match: 5,
            target_length: 4,
            search_strength: 7,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        -2 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 13,
            chain_log: 13,
            search_log: 0,
            min_match: 5,
            target_length: 3,
            search_strength: 7,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        -1 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 13,
            chain_log: 13,
            search_log: 0,
            min_match: 5,
            target_length: 2,
            search_strength: 7,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        1 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 14,
            chain_log: 14,
            search_log: 0,
            min_match: 4,
            target_length: 1,
            search_strength: 8,
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
            min_match: 4,
            target_length: 1,
            search_strength: 8,
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
            window_log: 23,
            hash_log: 19,
            chain_log: 19,
            search_log: 1,
            min_match: 4,
            target_length: 1,
            search_strength: 6,
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
        params.window_log = wl;
    }
    #[cfg(feature = "ldm")]
    if opts.ldm {
        params.ldm_params = Some(LdmParams::default_for_window_log(params.window_log));
    }
}
