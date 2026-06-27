# Changelog

## [Unreleased]

## [0.7.2]

### Fixed

- README: corrected "Safe decoder" claim. The decode crate has unsafe in
  `fast_vec.rs`; only the `paranoid` feature makes it fully safe.
- Regenerated small-input benchmark chart with structured-zstd 0.0.45.

## [0.7.1]

### Fixed

- Excluded `jsr/` directory from the published crate package.

## [0.7.0]

### Changed

- Decoder SIMD dispatch replaced with `fearless_simd::dispatch!`. Removed
  the entire `simd_decode/` directory (3 platform-specific fused decode
  loops, ~1300 lines) and all platform SIMD primitives in `core/simd/`
  (~800 lines). A single safe Rust implementation in `exec.rs` now handles
  all platforms; `fearless_simd` compiles it with the appropriate target
  features at build time.
- FSE sequence decode tables changed from `Vec` + raw-pointer
  `FseSeqTableView` to fixed-size `[FseSeqDecodeEntry; 512]` arrays with
  `state & 511` indexing. LLVM proves the index is always in bounds,
  eliminating bounds checks and all associated unsafe. Smaller stack frame
  (232 bytes vs 296 bytes).
- Decoder literal and match copies use wild-copy functions in `fast_vec.rs`:
  16-byte unaligned load/store loops for non-overlapping copies, pattern
  stamping for small overlapping offsets, `write_bytes` for RLE. Under
  `paranoid`, these fall back to `extend_from_slice` and
  `extend_from_within`.
- `paranoid` feature now gets full SIMD multiversioning via `fearless_simd`
  (no unsafe needed). Only Huffman BMI2 dispatch and wild-copy are gated
  out. Paranoid decode throughput improved significantly compared to 0.6.0
  where SIMD dispatch was unavailable.
- Bumped structured-zstd bench dependency to 0.0.45 and regenerated all
  benchmark charts (x86_64, wasm32).
- Updated DEVELOPMENT.md with paranoid bench instructions and full chart
  workflow.

### Removed

- `crates/decode/src/simd_decode/` (x86_64, aarch64, wasm32 fused decode).
- `crates/core/src/simd/` platform modules (avx2, sse2, bmi2, neon,
  simd128, scalar, copy). Only `CpuTier` enum and `cpu_tier()` remain for
  Huffman BMI2 dispatch.

## [0.6.0]

### Added

- WASM SIMD128 decode path: fused FSE decode+execute loop
  (`simd_decode/wasm32/decode.rs`) and core SIMD primitives (wildcopy,
  copy_match, common_prefix_len) in `simd/wasm32/simd128.rs`. Dispatched
  via `CpuTier::Wasm32Simd128` at compile time when
  `target_feature = "simd128"` is enabled.
- Long distance matching (LDM) encoder (`ldm.rs`): gear hash scan with
  gap fill via Fast/DFast strategy. Controlled via `Options::ldm(true)`
  and `Options::window_log(n)`. Enabled by default via the `ldm` feature
  flag, with zero runtime cost when not activated.
- Cross-block match references in streaming encoder: matches can now
  reference data from prior blocks within the window, improving
  compression ratio on multi-block inputs.
- `Options` API for configuring `window_log` and `ldm` on
  `compress_opts()` and `FrameEncoder::with_options()`.
- wasm32 benchmark charts (`doc/charts/wasm32/`) comparing zrip
  (SIMD128), zrip paranoid (no SIMD), C zstd (libzstd via wasi-sdk),
  and structured-zstd.
- `--profile` flag for plot scripts (`scripts/profiles.py`) enabling
  per-target chart generation with consistent codec colors and labels.
- Streaming, LDM, and cross-block matching tests in
  `tests/streaming.rs`.

### Fixed

- 32-bit overflow in `parse_compressed_header` format-3 branch
  (`literals.rs`): `(data[4] as usize) << 28` overflows on wasm32
  where usize is 32 bits. Fixed by accumulating into u64.
- `cpu_tier()` now returns `Scalar` under Miri to avoid UB from CPUID
  intrinsics.
- Unused variable warning on targets without prefetch support.

### Changed

- Bench cache layout restructured to per-target directories
  (`~/.cache/zrip/{arch}/L{level}/{codec}.jsonl`).
- Corpus paths resolved via `CARGO_MANIFEST_DIR` so bench downloads
  land in `bench/corpus/` regardless of working directory.
- JSR package `@paddor/zrip` bumped to 0.3.0.

## [0.5.1]

### Fixed

- Decoder now enforces per-table accuracy log limits: literal lengths
  and match lengths max 9, offsets max 8 (RFC 8878 section 3.1.1.3.2.1).
- Decoder rejects Huffman weight FSE tables with accuracy log > 6
  (RFC 8878 section 4.2.1).
- Decoder rejects Huffman streams with leftover bits after decoding.
- Decoder validates match offset does not exceed total output produced
  so far (current block + previous blocks + dictionary).
- Decoder validates sequence execution does not consume more literals
  than available, and that the last sequence uses all remaining literals.
- Decoder validates cooked match length and literal length values are
  within bounds.
- Decoder rejects Repeat_Mode on first block when no prior FSE table
  exists.

### Added

- `tests/spec_conformance.rs`: comprehensive test suite covering all
  new decoder validation paths and existing error paths.
- JSR publish workflow (`.github/workflows/jsr.yml`) with provenance
  via OIDC. Builds WASM and publishes `@paddor/zrip` on push to main.

### Changed

- JSR package `@paddor/zrip` bumped to 0.2.0: added package description,
  JSDoc on all exported symbols (`Compressor`, `Decompressor`,
  `Dictionary`), and rebuilt WASM artifacts.
- Bumped structured-zstd bench dependency to 0.0.44 and regenerated
  benchmark charts.

## [0.5.0]

### Added

- `paranoid` feature: compiles pure safe Rust with zero unsafe code.
  `#![forbid(unsafe_code)]` enforced on all four crate roots. Every
  unchecked operation has a safe alternative behind `#[cfg(feature =
  "paranoid")]`. SIMD modules gated out entirely; `cpu_tier()` returns
  `Scalar`. Even with paranoid, zrip L1 encode is >2x faster than
  ruzstd.
- CI job for `--features paranoid`: test + clippy.
- WebAssembly package published as
  [`@paddor/zrip`](https://jsr.io/@paddor/zrip) on JSR. Scalar and
  SIMD128 variants with auto-detection. 15% faster encode and 14%
  faster decode than C zstd compiled to WASM.

### Changed

- Replaced pointer-based `exec.rs` (scalar sequence decoder) with safe
  index-based implementation using `Vec<u8>` operations. Zero
  performance impact on default builds (SIMD handles all decode on
  AVX2/NEON hardware). Deleted `decode/src/primitives.rs` (dead code
  after this change).
- Regenerated x86_64 benchmark charts with paranoid data.

## [0.4.0]

### Added

- `PreparedDict` for hot-loop dictionary compression: pre-computes hash
  tables and entropy tables once, amortizing setup across many small
  compress calls.
- `decompress_with_limit(input, max_output_size)` and
  `SAFE_DECOMPRESS_LIMIT` constant for bounded decompression that
  rejects frames claiming unreasonable output sizes.
- Dict entropy table reuse and small-input optimizations for Fast
  strategy: skips redundant Huffman/FSE table construction when
  dictionary tables can be reused, improving throughput on small inputs.

### Fixed

- Integer overflow in `compress_bound` for near-`usize::MAX` inputs.

### Changed

- Split `CorruptSequences` error into specific variants
  (`CorruptLiteralLength`, `CorruptMatchLength`, `CorruptOffset`) for
  better diagnostics.
- Restricted internal types from leaking through `pub use` of
  sub-crates.
- Deduplicated `parse_fse_table_description`, `build_decode_table`,
  `write_seq_count`, and `write_frame_header` helpers across encode and
  decode paths.
- Replaced `assert_unchecked` with cold-path panic in
  `assert_rep_valid` for safer debug builds.
- Added `debug_assert` guards for Huffman table and unsafe invariants.
- Gated std-only tests and examples behind `feature = "std"`.
- Updated benchmark charts with structured-zstd 0.0.42 results and
  added L-3/L-1 lines to small-input chart.
- Added codebase size note to README.

## [0.3.4]

### Added

- SECURITY.md with private vulnerability reporting instructions.

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
