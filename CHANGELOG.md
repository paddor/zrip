# Changelog

## [Unreleased]

## [0.3.3]

### Fixed

- Bump sub-crate versions to allow publishing on crates.io (0.3.2
  sub-crates were never published).

## [0.3.2]

### Added

- DESIGN.md: encode/decode pipeline, SIMD dispatch, compile-time
  specialization, level parameters, and divergences from C zstd.
- SAFETY.md: catalogs 17 verified C zstd memory safety bugs that Rust
  prevents by construction (CVEs, OSS-Fuzz findings, AFL reports).
- Per-file pipeline chart (`plot_pipeline.py` and `pipeline.svg`).
- aarch64 benchmark charts behind collapsible in README.
- Dictionary training example in README (`train_dict_fastcover`).

### Changed

- Replaced stale README performance tables with auto-generated charts.
- Chart legend label changed from "safe API" to "encapsulated unsafe".
- Fixed README Levels table: min-match was wrong for L1 (said 7, is 4)
  and L2 (said 6, is 4).
- `detect_hardware()` in plot scripts now supports `ZRIP_CPU` env var
  for macOS/aarch64 chart subtitle.
- Added `include` field to Cargo.toml for crates.io packaging.

## [0.3.1]

### Fixed

- UB in SIMD decode: `wildcopy_neon_write32` computed an out-of-bounds pointer
  before checking if the fast path applies, triggering UB even when `len < 32`.
- Removed dead `decode_sequences_neon` that referenced undefined variables and
  would fail to compile on aarch64.
- Fixed `eprintln!("")` clippy warning in decode_compare example.

### Changed

- Split monolithic 3k-line test file into focused modules (`roundtrip`,
  `interop`, `streaming`, `dict`, `adversarial`, `proptest_tests`).
- Made tests Miri-compatible: avoid C zstd FFI and SIMD paths under Miri,
  use pure-Rust scalar fallbacks.
- Removed stale version badge from README.

## [0.3.0]

### Added

- Streaming dictionary compression: `FrameEncoder::with_dict()`,
  `FrameDecoder::with_dict()`, and `reset_with_dict()` for buffer-reusing
  streaming with trained dictionaries.
- `CompressContext::with_dict()` and `DecompressContext::with_dict()` for
  reusable one-shot dictionary compression.

### Changed

- Encapsulated all unsafe behind `primitives.rs` modules with safe
  `pub(crate)` signatures. Every algorithm module now enforces
  `#![forbid(unsafe_code)]`. Unsafe confined to primitives, SIMD intrinsics,
  and the 4-stream Huffman decoder.

## [0.2.1]

### Fixed

- Sub-crate feature flags: `std` now correctly enables `alloc` in zrip-encode
  and zrip-decode, fixing compilation when published to crates.io.
- Added version specifiers to workspace path dependencies for crates.io
  publishing.

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
