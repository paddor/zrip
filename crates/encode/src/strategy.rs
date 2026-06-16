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
pub fn level_params(level: i32) -> Option<LevelParams> {
    Some(match level {
        0 => return level_params(DEFAULT_LEVEL),
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
            hash_log: 17,
            chain_log: 17,
            search_log: 1,
            min_match: 5,
            target_length: 1,
            search_strength: 4,
            force_raw_literals: false,
        },
        4 => LevelParams {
            strategy: Strategy::DFast,
            window_log: 21,
            hash_log: 18,
            chain_log: 18,
            search_log: 1,
            min_match: 5,
            target_length: 1,
            search_strength: 4,
            force_raw_literals: false,
        },
        _ => return None,
    })
}
