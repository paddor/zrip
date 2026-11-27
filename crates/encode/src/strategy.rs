//! Level parameters: the settings each level runs with, and how the input
//! size and [`Options`] adjust them.
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
    /// Log2 of the LDM table's entries.
    pub hash_log: u32,
    /// Log2 of the entries per bucket, at most `hash_log` and 8.
    pub bucket_size_log: u32,
    /// Shortest LDM match in bytes. Each split point indexes this many
    /// bytes before it.
    pub min_match_length: u32,
    /// Split points, which LDM indexes and looks up, fall about once every
    /// 2^`hash_rate_log` bytes.
    pub hash_rate_log: u32,
}

impl LdmParams {
    /// Defaults for a 2^`window_log`-byte window: a table of up to 2^20
    /// entries in buckets of 16, and 64-byte minimum matches.
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
    /// Match finder.
    pub strategy: Strategy,
    /// Log2 of the window size in bytes, [`WINDOW_LOG_MIN`] to
    /// [`WINDOW_LOG_MAX`].
    pub window_log: u32,
    /// Log2 of the entries of the Fast table, or of the DFast long table.
    pub hash_log: u32,
    /// DFast short table log. Same as hashLog for Fast strategy.
    pub chain_log: u32,
    /// C zstd's `searchLog`. The Fast and DFast finders ignore it.
    pub search_log: u32,
    /// Bytes the Fast table hashes, 4 to 8, or the DFast short table, 4 or
    /// 5. A Fast match needs 8 equal bytes at 8, else 4.
    pub min_match: u32,
    /// Fast only: the search step starts at `target_length + 1` bytes, at
    /// least 2, as in C zstd. Above 1, with `min_match` 5 or more, it also
    /// keeps dense literals raw: over 6.25 to 6.37 bits per byte, and over 5
    /// to 6 bits in blocks with matches, as L-6 to L-1 do on large inputs.
    pub target_length: u32,
    /// The search step grows by one every 2^`search_strength` bytes
    /// without a match. C zstd fixes it at 8. A DFast block after one that
    /// compressed to more than 60% of its size searches as at 0.
    pub search_strength: u32,
    /// Keep literals raw and use the predefined sequence tables. Blocks over
    /// 32 KiB that are almost all literals may still take Huffman literals.
    pub force_raw_literals: bool,
    /// Long distance matching; `None` turns it off.
    #[cfg(feature = "ldm")]
    pub ldm_params: Option<LdmParams>,
}

impl LevelParams {
    /// Sets the window to 2^`window_log` bytes.
    /// [`compress_with_params`](crate::compress_with_params) clamps it to
    /// [`WINDOW_LOG_MIN`]..=[`WINDOW_LOG_MAX`] and to the input.
    #[must_use]
    pub fn with_window_log(mut self, window_log: u32) -> Self {
        self.window_log = window_log;
        self
    }

    /// Enables long distance matching with `params`. The window stays as
    /// set; see [`LdmParams::default_for_window_log`].
    #[cfg(feature = "ldm")]
    #[must_use]
    pub fn with_ldm(mut self, params: LdmParams) -> Self {
        self.ldm_params = Some(params);
        self
    }
}

/// Default compression level used when level 0 is requested.
pub const DEFAULT_LEVEL: i32 = 1;

/// Lowest compression level. Level -8, a separate level in earlier
/// versions, is still accepted and runs as -7.
pub const MIN_LEVEL: i32 = -7;

/// Highest compression level.
pub const MAX_LEVEL: i32 = 4;

/// Returns the compression parameters for a given level, or `None` if out of range.
///
/// Level 0 is treated as "library default" and maps to level 1.
/// Uses the large-input (>256 KB) parameter tier.
pub fn level_params(level: i32) -> Option<LevelParams> {
    level_params_for_size(level, usize::MAX)
}

/// Returns the compression parameters for a given level, sized for `src_len`.
///
/// Uses fixed parameters per level with log values clamped down for small
/// inputs. Inputs of up to 256 KiB search with their own minimum match,
/// step and search strength at most levels (see `short_input_search`).
///
/// Level 0 is treated as "library default" and maps to level 1.
pub fn level_params_for_size(level: i32, src_len: usize) -> Option<LevelParams> {
    let mut params = level_params_inner(level)?;
    let search = short_input_search(level, src_len);
    if let Some(search) = search {
        if let Some(target_length) = search.target_length {
            params.target_length = target_length;
        }
        params.min_match = search.min_match;
        params.search_strength = search.search_strength;
    }
    params.hash_log = params.hash_log.clamp(HASH_LOG_MIN, HASH_LOG_MAX);
    params.chain_log = params.chain_log.clamp(HASH_LOG_MIN, HASH_LOG_MAX);
    params.window_log = params.window_log.clamp(WINDOW_LOG_MIN, WINDOW_LOG_MAX);
    if (2..usize::MAX).contains(&src_len) {
        let src_log = src_log(src_len);
        let slack = search.map_or(0, |s| s.hash_slack);
        params.hash_log = params.hash_log.min(src_log + slack).max(HASH_LOG_MIN);
        params.chain_log = params.chain_log.min(src_log).max(HASH_LOG_MIN);
        params.window_log = params.window_log.min(src_log);
    }
    Some(params)
}

/// Returns the bits that address `src_len` bytes: its log2, rounded up.
pub(crate) fn src_log(src_len: usize) -> u32 {
    usize::BITS - src_len.saturating_sub(1).leading_zeros()
}

/// Returns the compression parameters for `input_len` bytes after a
/// `prefix_len`-byte dictionary. The tables and window cover both, while the
/// search follows the input alone, as in a frame without a dictionary: its
/// density at L1 to L4, also the step and minimum match at the negative
/// levels.
pub(crate) fn level_params_with_prefix(
    level: i32,
    prefix_len: usize,
    input_len: usize,
) -> Option<LevelParams> {
    let mut params = level_params_for_size(level, prefix_len.saturating_add(input_len))?;
    if level < 0 {
        let own = level_params_for_size(level, input_len)?;
        params.target_length = own.target_length;
        params.min_match = own.min_match;
        params.search_strength = own.search_strength;
    } else if let Some(search) = short_input_search(level, input_len) {
        params.search_strength = search.search_strength;
    }
    Some(params)
}

/// Largest input of the small tier, C zstd's smallest parameter table.
const SMALL_INPUT_LIMIT: usize = 16 * 1024;

/// Largest input of the short tier, two blocks.
const SHORT_INPUT_LIMIT: usize = 256 * 1024;

/// Search settings for an input size tier.
#[derive(Clone, Copy, Debug)]
struct ShortSearch {
    /// Replaces the level's `target_length` if set.
    target_length: Option<u32>,
    min_match: u32,
    search_strength: u32,
    /// Lets the table grow this many bits past the input's size.
    hash_slack: u32,
}

/// Search for inputs of up to [`SMALL_INPUT_LIMIT`] and
/// [`SHORT_INPUT_LIMIT`] bytes, as C zstd picks its parameters by input
/// size. `None` for larger inputs, which take the level's own.
///
/// Up to 16 KiB, L-6 searches as L-7 does, eight-byte matches at a short
/// step, and differs from it in Huffman literals for the inputs whose
/// literals shrink enough (see `negative_literal_limits`). L-5 to L-1 hash
/// six bytes at steps that shrink or grow slower level by level. Six-byte
/// hashes found fewer matches than five on raw literals, but gave smaller
/// output once literals take Huffman codes, and keep the levels in order
/// either way. Short inputs hold few long repeats, so seven-byte hashes miss
/// many of their matches: L1 wrote 11.5% more than C zstd on 4 KiB blocks
/// of metric points, where C hashes five bytes. Up to 16 KiB, L1 hashes six
/// bytes and L2 five (six with a table twice the input's size up to 2 KiB,
/// where hashing five bytes beat L3), and L3 searches denser up to 1 KiB.
/// From 16 KiB to 256 KiB, L2 grows its step slower than L1, which matched
/// L1 there, and L3 and L4 search sparser than on large inputs: at C
/// zstd's search strength of 8, zrip's DFast search ran at 0.7-0.8x C's
/// speed; these keep about C's speed at L3 for 4-9% more output.
fn short_input_search(level: i32, input_len: usize) -> Option<ShortSearch> {
    let s = |target_length, min_match, search_strength| {
        Some(ShortSearch {
            target_length,
            min_match,
            search_strength,
            hash_slack: 0,
        })
    };
    if input_len <= SMALL_INPUT_LIMIT {
        return match level {
            -6 => s(Some(2), 8, 4),
            -5 => s(Some(3), 6, 4),
            -4 => s(Some(3), 6, 6),
            -3 => s(Some(2), 6, 5),
            -2 => s(Some(2), 6, 7),
            -1 => s(Some(1), 6, 6),
            1 => s(None, 6, 4),
            2 if input_len <= 2048 => Some(ShortSearch {
                target_length: None,
                min_match: 6,
                search_strength: 4,
                hash_slack: 1,
            }),
            2 => s(None, 5, 4),
            3 if input_len <= 1024 => s(None, 5, 5),
            3 => s(None, 5, 4),
            4 => s(None, 5, 6),
            _ => None,
        };
    }
    if input_len <= SHORT_INPUT_LIMIT {
        return match level {
            2 => s(None, 7, 5),
            3 => s(None, 5, 4),
            4 => s(None, 5, 6),
            _ => None,
        };
    }
    None
}

/// Smallest hash or chain log, a 64-entry table.
pub const HASH_LOG_MIN: u32 = 6;
/// Largest hash or chain log.
pub const HASH_LOG_MAX: u32 = 30;
/// Smallest window log, a 1 KiB window.
pub const WINDOW_LOG_MIN: u32 = 10;
/// Largest window log, a 128 MiB window.
pub const WINDOW_LOG_MAX: u32 = 27;
#[cfg(feature = "ldm")]
const LDM_DEFAULT_WINDOW_LOG: u32 = 27;

/// Whether an input of `input_len` bytes keeps raw literals at these
/// parameters.
pub(crate) fn keeps_raw_literals(params: &LevelParams, _input_len: usize) -> bool {
    params.force_raw_literals
}

/// Per-block encoding choices derived from the level parameters.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BlockPolicy {
    /// Try custom sequence tables.
    pub custom_tables: bool,
    /// Try Huffman literals (within the entropy limits below). Without it,
    /// only literal-heavy blocks may still take them.
    pub huffman_literals: bool,
    /// Literals whose sampled order-0 entropy exceeds this many bits per byte
    /// (8.8 fixed point) stay raw. `u32::MAX` disables the check.
    pub max_literal_entropy_fp8: u32,
    /// The same limit for a block whose matches cover an eighth of it or
    /// more. See [`BlockPolicy::for_matches`].
    pub max_matched_literal_entropy_fp8: u32,
}

impl BlockPolicy {
    /// The policy for a block of `block_len` bytes whose matches cover
    /// `total_match` of them: a block with matches over an eighth or more
    /// takes the limit for such blocks as its literal limit.
    ///
    /// A block that is almost all literals only shrinks through Huffman
    /// coding, so it keeps the higher limit.
    pub(crate) fn for_matches(self, block_len: usize, total_match: usize) -> Self {
        if total_match < block_len / 8 {
            return self;
        }
        Self {
            max_literal_entropy_fp8: self.max_matched_literal_entropy_fp8,
            ..self
        }
    }
}

/// Encoding choices for a block of an input of `input_len` bytes at
/// `level`, or at custom parameters if `None`.
///
/// The fast negative levels skip Huffman coding for dense literals: on
/// low-compressibility data it costs more time than it saves space (see
/// `negative_literal_limits`). L-7 keeps literals raw always. The faster a
/// level searches, the more literals it leaves, and Huffman literals decode
/// slower than matches, so in blocks with matches L-6 to L-3 keep literals
/// raw from a limit that rises with the level: 5.1, 5.25, 5.4 and 6 bits per
/// byte. Each level then decodes faster than the next, at a few percent of
/// ratio on text-like data.
pub(crate) fn block_policy(
    params: &LevelParams,
    level: Option<i32>,
    input_len: usize,
) -> BlockPolicy {
    let limits = if params.strategy != Strategy::Fast {
        None
    } else if let Some(level) = level {
        negative_literal_limits(level, input_len)
    } else {
        custom_literal_limits(params)
    };
    let (max_literal_entropy_fp8, max_matched_literal_entropy_fp8) =
        limits.unwrap_or((u32::MAX, u32::MAX));
    let raw = keeps_raw_literals(params, input_len);
    BlockPolicy {
        custom_tables: !raw && !small_negative_input(level, input_len),
        huffman_literals: !raw && max_literal_entropy_fp8 >= HUFFMAN_FLOOR,
        max_literal_entropy_fp8,
        max_matched_literal_entropy_fp8,
    }
}

/// Literal limit below which a level does not try Huffman literals at all:
/// so few literals pass it that counting them would cost more time than it
/// saves space.
const HUFFMAN_FLOOR: u32 = 3 * 256;

/// Whether L-6 to L-1 grade their literal limits for this input (see
/// [`negative_literal_limits`]). Such inputs also keep the predefined
/// sequence tables and skip sampling blocks for incompressible data before
/// the match search: both cost a few microseconds per input, more than they
/// save there.
pub(crate) fn small_negative_input(level: Option<i32>, input_len: usize) -> bool {
    level.is_some_and(|l| (-6..=-1).contains(&l)) && input_len <= GRADED_LIMIT
}

/// Largest input whose literal limits [`negative_literal_limits`] grades.
const GRADED_LIMIT: usize = GRADED_MAX;

/// Literal entropy limits (8.8 fixed point bits per byte) at L-6 to L-1 for
/// an input of `input_len` bytes, as `(literals, literals in blocks with
/// matches)`. `None` for the other levels.
///
/// Large inputs keep literals over 6.25 bits per byte raw at L-6 to L-3,
/// and over 6.37 at L-2 and L-1. With matches over an eighth of the block,
/// the limits are 5.1, 5.25, 5.4 and 6 bits at L-6 to L-3, 6.64 at L-2 and
/// 6.48 at L-1.
///
/// Building a Huffman table costs a few microseconds regardless of the
/// input's size, so on small inputs only literals that shrink a lot pay for
/// it. Up to [`GRADED_LIMIT`] bytes, the limit starts at 4.8 bits per byte
/// at L-1 on 512 B, rises by a quarter bit each time the input doubles, and
/// drops by 0.35 bits per level below L-1. Inputs whose literals differ in
/// entropy (text, markup, binary) then turn to Huffman literals at
/// different levels, and the switch from raw literals at L-7 to Huffman
/// literals at L1 spreads over the negative levels at every size, where it
/// was one step worth 22-37% of ratio and half the speed.
fn negative_literal_limits(level: i32, input_len: usize) -> Option<(u32, u32)> {
    let (literals, matched) = match level {
        -6 => (DENSE, 5 * 256 + 26),
        -5 => (DENSE, 5 * 256 + 64),
        -4 => (DENSE, 5 * 256 + 102),
        -3 => (DENSE, 6 * 256),
        -2 => (6 * 256 + 94, 6 * 256 + 164),
        -1 => (6 * 256 + 94, 6 * 256 + 124),
        _ => return None,
    };
    if input_len > GRADED_LIMIT {
        return Some((literals, matched));
    }
    let doublings = src_log(input_len.max(1)).saturating_sub(9);
    let rank = (-level - 1) as u32;
    let graded =
        (GRADED_BASE + GRADED_PER_DOUBLING * doublings).saturating_sub(GRADED_PER_LEVEL * rank);
    Some((literals.min(graded), matched.min(graded)))
}

/// Literal limits for custom parameters, from the search they run: those of
/// the negative level that searches alike on large inputs.
fn custom_literal_limits(params: &LevelParams) -> Option<(u32, u32)> {
    if params.min_match < 5 {
        return None;
    }
    let level = match params.target_length {
        0 | 1 => return None,
        _ if params.min_match < 8 => -1,
        2 | 3 if params.hash_log > 13 => -3,
        2 | 3 => -4,
        4 => -5,
        _ => -6,
    };
    negative_literal_limits(level, usize::MAX)
}

/// The limit above which literals stay raw at L-6 to L-3 on large inputs.
const DENSE: u32 = 6 * 256 + 64;
/// [`negative_literal_limits`] on small inputs, in 8.8 fixed point bits per
/// byte: L-1's limit on 512 B, its rise per doubling of the input, and its
/// drop per level below L-1.
const GRADED_BASE: u32 = 4 * 256 + 205;
const GRADED_PER_DOUBLING: u32 = 64;
const GRADED_PER_LEVEL: u32 = 90;
const GRADED_MAX: usize = 32 * 1024;

fn level_params_inner(level: i32) -> Option<LevelParams> {
    Some(match level {
        0 => return level_params_inner(DEFAULT_LEVEL),
        // The fastest level, for LZ4's niche: raw literals, as in C zstd,
        // and only matches of eight bytes or more (`min_match` 8 hashes and
        // confirms eight bytes), so frames decode with few sequences. Blocks
        // that are almost all literals try Huffman literals instead, so
        // low-compressibility data still shrinks a little. It steps 3 bytes
        // and grows the step every 16 bytes without a match. Matches are its
        // only compression, so it searches denser than L-6, in a table of
        // the same size: a table twice the size gained as much ratio but
        // missed the cache so often that L-6 encoded highly compressible
        // data faster. L-8, a separate level before, runs as L-7.
        -8 | -7 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 13,
            chain_log: 13,
            search_log: 0,
            min_match: 8,
            target_length: 2,
            search_strength: 4,
            force_raw_literals: true,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        // L-6 to L-3 hash and confirm 8 bytes. With Huffman literals, longer
        // matches at short steps beat shorter matches at long steps on both
        // ratio and speed, as fewer bytes are left to code as literals and
        // frames decode with fewer sequences. The step grows every 16 bytes
        // without a match, so sparse regions cost few probes.
        -6 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 13,
            chain_log: 13,
            search_log: 0,
            min_match: 8,
            target_length: 5,
            search_strength: 4,
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
            min_match: 8,
            target_length: 4,
            search_strength: 4,
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
            min_match: 8,
            target_length: 2,
            search_strength: 4,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        // L-3 searches as L-4 does, in a table twice the size. Seven-byte
        // hashes at a longer step, as before, gained almost no ratio over
        // L-4 on highly compressible data, and lost ratio on inputs up to
        // 256 KiB.
        -3 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 14,
            chain_log: 14,
            search_log: 0,
            min_match: 8,
            target_length: 2,
            search_strength: 4,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        // L-2 searches as L-3 and Huffman-codes denser literals (see
        // `negative_literal_limits`). L-1 hashes seven bytes and grows the step every 32
        // bytes without a match, so frequent matches keep the initial step
        // while long literal runs need few probes.
        -2 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 14,
            chain_log: 14,
            search_log: 0,
            min_match: 8,
            target_length: 2,
            search_strength: 4,
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
            min_match: 7,
            target_length: 2,
            search_strength: 5,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        // L1 and L2 hash seven bytes to favor longer matches and grow the
        // step every 16 bytes without a match. Dense matches keep the
        // initial step, while long literal runs need fewer probes. L1 hashes
        // into a table of C zstd's size, L2 into half of C's, which trades
        // a little ratio for speed.
        1 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 19,
            hash_log: 14,
            chain_log: 14,
            search_log: 0,
            min_match: 7,
            target_length: 1,
            search_strength: 4,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        2 => LevelParams {
            strategy: Strategy::Fast,
            window_log: 20,
            hash_log: 15,
            chain_log: 15,
            search_log: 0,
            min_match: 7,
            target_length: 1,
            search_strength: 4,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        // C zstd's level 3 tables and search. A block after one that
        // compressed to more than 60% of its size grows the step after every
        // miss (see `DfastSkip`), so low-compressibility data stays fast.
        3 => LevelParams {
            strategy: Strategy::DFast,
            window_log: 21,
            hash_log: 17,
            chain_log: 16,
            search_log: 1,
            min_match: 5,
            target_length: 1,
            search_strength: 8,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        // C zstd's level 4 search with a 16 MiB window, which gives smaller
        // output than C's 2 MiB at the same speed, and a short table twice
        // C's size. With C's, L4 encoded highly compressible data faster
        // than L3.
        4 => LevelParams {
            strategy: Strategy::DFast,
            window_log: 24,
            hash_log: 18,
            chain_log: 19,
            search_log: 0,
            min_match: 5,
            target_length: 1,
            search_strength: 8,
            force_raw_literals: false,
            #[cfg(feature = "ldm")]
            ldm_params: None,
        },
        _ => return None,
    })
}

/// Carries how well the last DFast block compressed into the next block's
/// search. After a block that compressed to more than 60% of its size, the
/// next one searches as at `search_strength` 0, growing its step after every
/// miss: a dense search costs time on low-compressibility data and finds
/// little. Text, code and database blocks stay under 60%. Inputs of up to
/// two blocks keep their search, which `short_input_search` sets.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DfastSkip {
    enabled: bool,
    sparse: bool,
}

impl DfastSkip {
    /// For an input of `input_len` bytes, or `usize::MAX` if its size is
    /// unknown.
    #[inline]
    pub(crate) fn for_input(input_len: usize) -> Self {
        Self {
            enabled: input_len > SHORT_INPUT_LIMIT,
            sparse: false,
        }
    }

    /// The parameters for the next block.
    #[inline]
    pub(crate) fn params(self, params: &LevelParams) -> LevelParams {
        let mut block_params = *params;
        if self.sparse {
            block_params.search_strength = 0;
        }
        block_params
    }

    /// Records that the last block compressed `input_len` bytes to
    /// `output_len`.
    #[inline]
    pub(crate) fn record(&mut self, input_len: usize, output_len: usize) {
        self.sparse = self.enabled && output_len.saturating_mul(5) > input_len.saturating_mul(3);
    }
}

/// Options orthogonal to level: a larger window, long distance matching,
/// and whether frames carry a content checksum.
///
/// Pass to [`compress_opts`](crate::compress_opts) or
/// `FrameEncoder::with_options` (with `std`).
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub(crate) window_log: Option<u32>,
    #[cfg_attr(not(feature = "ldm"), allow(dead_code))]
    pub(crate) ldm: bool,
    pub(crate) no_content_checksum: bool,
}

impl Options {
    /// Sets the window to 2^`log` bytes. `log` is clamped to
    /// [`WINDOW_LOG_MIN`]..=[`WINDOW_LOG_MAX`], 1 KiB to 128 MiB, and `0`
    /// selects the level's default, as in C zstd. One-shot calls shrink the
    /// window to the input.
    #[must_use]
    pub fn window_log(mut self, log: u32) -> Self {
        self.window_log = (log != 0).then_some(log);
        self
    }

    /// Enables long distance matching. Unless `window_log` is set, the window
    /// defaults to 2^27 bytes. One-shot calls shrink it to the input.
    #[cfg(feature = "ldm")]
    #[must_use]
    pub fn ldm(mut self, enable: bool) -> Self {
        self.ldm = enable;
        self
    }

    /// Enables or disables the content checksum, on by default: the low 32
    /// bits of the XXH64 of the content, 4 bytes at the end of the frame,
    /// which decoders verify. C zstd leaves it out by default.
    #[must_use]
    pub fn content_checksum(mut self, enabled: bool) -> Self {
        self.no_content_checksum = !enabled;
        self
    }

    /// Whether frames carry a content checksum.
    pub(crate) const fn checksum(&self) -> bool {
        !self.no_content_checksum
    }
}

/// Applies `opts` to `params`: the window log, clamped to
/// [`WINDOW_LOG_MIN`]..=[`WINDOW_LOG_MAX`], and with LDM on, a 2^27-byte
/// window unless set and [`LdmParams`] sized for the window.
pub fn apply_options(params: &mut LevelParams, opts: &Options) {
    if let Some(wl) = opts.window_log {
        params.window_log = wl.clamp(WINDOW_LOG_MIN, WINDOW_LOG_MAX);
    }
    #[cfg(feature = "ldm")]
    if opts.ldm {
        if opts.window_log.is_none() {
            params.window_log = LDM_DEFAULT_WINDOW_LOG;
        }
        params.ldm_params = Some(LdmParams::default_for_window_log(params.window_log));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_limits_rise_with_level_and_input_size() {
        let sizes = [1, 512, 513, 4096, 16 * 1024, GRADED_LIMIT];
        for &n in &sizes {
            let limits: Vec<(u32, u32)> = (-6..=-1)
                .map(|level| negative_literal_limits(level, n).unwrap())
                .collect();
            assert!(
                limits.windows(2).all(|w| w[0].0 <= w[1].0),
                "{n}: {limits:?}"
            );
        }
        for level in -6..=-1 {
            let limits: Vec<u32> = sizes
                .iter()
                .map(|&n| negative_literal_limits(level, n).unwrap().0)
                .collect();
            assert!(
                limits.windows(2).all(|w| w[0] <= w[1]),
                "L{level}: {limits:?}"
            );
            assert_eq!(
                negative_literal_limits(level, GRADED_LIMIT + 1),
                negative_literal_limits(level, usize::MAX)
            );
        }
        assert_eq!(negative_literal_limits(-7, 512), None);
        assert_eq!(negative_literal_limits(1, 512), None);
    }
}
