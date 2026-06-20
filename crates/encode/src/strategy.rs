#![forbid(unsafe_code)]

/// Match-finding strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Single hash table (levels -7 through 2).
    Fast,
    /// Short + long hash tables (levels 3-4).
    DFast,
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
    if src_len < usize::MAX && src_len >= 2 {
        let src_log = 32 - ((src_len as u32) - 1).leading_zeros();
        params.hash_log = params.hash_log.min(src_log);
        params.chain_log = params.chain_log.min(src_log);
        params.window_log = params.window_log.min(src_log);
    }
    Some(params)
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
        -7 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 13,
            chain_log: 13,
            search_log: 0,
            min_match: 5,
            target_length: 8,
            search_strength: 7,
            force_raw_literals: true,
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
        },
        _ => return None,
    })
}
