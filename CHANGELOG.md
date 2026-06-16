# Changelog

## [Unreleased]

## [0.2.0]

### Changed

- Split into workspace crates (zrip-core, zrip-encode, zrip-decode). Public API
  unchanged.
- Encode throughput improved 4-17% across all levels:
  - Reduced instruction count 9.6% via gather loop and stack-allocated FSE
    tables in the block encoder.
  - Likely/unlikely branch hints on encode and decode fast paths.
  - Inlined DFast rep offsets and hash helpers to eliminate aliasing barriers.
  - Switched DFast short hash to h5 for +5% encode speed.
  - Fixed DFast `rep_match_loop` to avoid redundant hash lookups.
  - Reduced match finder register pressure by eliminating ip3 and deferring
    hash computation.
- Decode throughput improved 5-24% across all levels:
  - Reduced decode instruction count via inline rep offsets and no-history
    specialization.
  - Likely/unlikely branch hints on decode fast paths.
- Switched negative levels to 5-byte hash for fewer false collisions.
- Fixed L1/L2 accidentally using hash5 when min_match >= 5.
- Removed dead code and unused unchecked modules across workspace.
- Updated dev-dependency structured-zstd to 0.0.40.

## [0.1.4]

### Changed

- Split README benchmark table by compressible/incompressible corpus.

## [0.1.3]

### Changed

- Level 0 now maps to the library default (level 1) instead of returning an
  error. Exported `DEFAULT_LEVEL` constant.
- L-7 now emits raw literals and uses predefined FSE tables only, skipping
  Huffman encoding and custom FSE table construction. Lower ratio, higher
  throughput.

## [0.1.2]

### Changed

- Aligned negative-level params to C zstd 1.5.7: window_log 14->19,
  hash_log 14->13. The larger window improves ratio at negative levels
  (L-7: 2.19x -> 2.39x geomean) and the smaller hash table (32KB, fits L1
  cache) keeps encode speed competitive.
- Updated dev-dependency structured-zstd to 0.0.37.

### Added

- Encode speed vs compression ratio scatter chart showing all levels L-7..L4.

## [0.1.1]

### Changed

- Aligned L1 and L2 compression parameters to C zstd 1.5.7 defaults
  (hash_log 17->14 at L1, 18->16 at L2), reducing hash table pressure on L1
  cache and improving encode throughput on all data.
- L1 encode throughput improved ~18% (193->228 MB/s geomean).
- L1 decode throughput improved ~18% (566->667 MB/s geomean).
- L3 DFast window_log increased from 20 to 21 to match C zstd.

### Added

- Mid-block match ratio bail-out in the Fast encoder: blocks with poor match
  rates are detected and emitted as raw blocks early, avoiding wasted work on
  incompressible data.
- Encoder pre-check in block_encoder that skips FSE/Huffman encoding when
  total match bytes cannot beat a raw block.

## [0.1.0]

- Initial release: Fast/DFast encode (levels -7..4), decode, streaming,
  dictionary compression (COVER/FastCOVER), SIMD-dispatched hot paths, no_std.
