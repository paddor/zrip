# Development

## Build

```bash
cargo build
cargo build --no-default-features --features alloc   # no_std + alloc
```

## Test

```bash
cargo test
cargo test --no-default-features --features alloc     # no_std tests
```

## Releasing

`release-plz` runs on every push to `main`
(`.github/workflows/release-plz.yml`). It opens or updates a release PR,
creates annotated tags after merge, publishes to crates.io, and creates
GitHub releases. Configuration lives in `release-plz.toml`.

### Steps

1. **Review the release-plz PR.** Verify semver bumps.

2. **Curate changelogs.** For each bumped crate, insert a new
   `## [x.y.z]` section below `## [Unreleased]`. Never modify existing
   versioned sections.

3. **Bump the JSR package when publishing WASM.** Update
   `jsr/deno.json`, rebuild the package with `cd jsr && bash build.sh`, and
   refresh wasm32 charts if Rust codec performance changed.

4. **Run any needed release audit.** Use the Miri and fuzz commands below
   when the release risk warrants an extended audit.

5. **Merge the release PR.** release-plz tags and publishes to crates.io
   automatically.

## Kani

Proves decoder and encoder bounds safety via bounded model checking.
Requires [Kani](https://model-checking.github.io/kani/)
(`cargo install --locked kani-verifier && cargo kani setup`).

Eighteen proof harnesses across two crates:

- **Decoder** (`crates/decode/src/fast_vec.rs`, 11 harnesses): one arithmetic
  proof that `BlockOutput::new` reserves sufficient capacity for all wildcopy
  variants, plus per-primitive proofs for `build_pattern_u64`,
  `fast_extend_from_ptr`, `wild_copy_match_unchecked` (four offset tiers),
  `wild_copy_match_16plus_unchecked`, and `wild_copy_match_single_unchecked`
  (three dispatch paths).

- **Encoder** (`crates/encode/src/primitives.rs`, 7 harnesses): `rd32`,
  `rd64`, `hash_load`, `hash_store`, `count_match` (8-byte fast loop + byte
  tail), and two `BitstreamScratch` proofs (`flush`/`write_byte` + `finish`
  never exposes uninitialized bytes via `set_len`).

```sh
# decoder (~2 min)
cargo kani -p zrip-decode --output-format terse

# encoder (~7 sec)
cargo kani -p zrip-encode --output-format terse
```

The proofs are per-primitive with targeted preconditions, not end-to-end.
Each harness isolates one unsafe function, constrains symbolic parameters to
the documented safety contract, and proves every code path stays in bounds.
The `block_output_capacity_sufficient` arithmetic proof separately verifies
that the safe API establishes those preconditions. This layered approach
keeps individual proofs fast (small SAT problems) but does not exercise the
full decompressor the way an exhaustive all-inputs proof would.

## Fuzz

Fuzz targets live in `fuzz/fuzz_targets/`. Round-trip targets cross-validate
against C zstd. Corruption targets feed mutated compressed data to the decoder.

```bash
cargo +nightly fuzz run roundtrip_frame -- -max_len=65536
cargo +nightly fuzz run c_compress_zrip_decompress
```

## Pre-release Miri + fuzz audit

Before tagging a release, run the scripted memory audit. It runs:

- targeted Miri tests for unsafe encode and decode primitives;
- full alloc-mode Miri with strict flags and 256 seed variations;
- Miri decode smoke tests, including the dictionary corpus fixture;
- every fuzz target under AddressSanitizer, with corrupt targets first.

For the full release duration, run each fuzz target for 3 hours:

```bash
FUZZ_SECONDS_PER_TARGET=10800 scripts/overnight_memory_audit.sh
```

For an overnight pass, the script defaults to 30 minutes per fuzz target.
Logs, artifacts, and TSV summaries are written under
`tmp/overnight-memory-audit/$RUN_ID/`.

The targeted Miri phase delegates to `scripts/miri_unsafe_primitives.sh`,
which can still be run directly for a bounded unsafe-primitives audit.

Useful controls:

```bash
SMOKE=1 scripts/overnight_memory_audit.sh          # quick harness check
CPU_COUNT=8 scripts/overnight_memory_audit.sh      # build/fuzz parallelism
RUN_FULL_MIRI=0 scripts/overnight_memory_audit.sh  # fuzz + unsafe Miri only
```

The dict decode test uses a pre-built fixture
(`tests/fixtures/corpus_dict_roundtrip.bin`). Regenerate after corpus changes:
`cargo test --features dict_builder -- fuzz_corpus_dict_generate`

### Adversarial corpus

If you have an adversarial corpus of small/malformed zstd files (e.g. from
prior fuzzing campaigns), seed them into the corrupt_decompress target:

```bash
cargo +nightly fuzz run fuzz_corrupt_decompress /path/to/adversarial/corpus \
  -- -max_total_time=10800 -jobs=2
```

## Benchmarks

### Corpus benchmarks

All benchmark and chart commands below run from the repository root. The
`bench/` crate is excluded from the workspace, so use
`--manifest-path bench/Cargo.toml`.

```bash
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --impl all
```

Options: `--impl zrip|"C zstd"|all`, `--levels -8,1,3`,
`--files dickens,mozilla`, `--extra /path/to/file`.

The benchmark harness prepares its own ignored corpus under `bench/corpus/`.
It downloads the 12 pinned Silesia inputs. `--small-only` slices the first bytes
from the base corpus in memory; no small corpus files need to be generated.

Results cache under `~/.cache/zrip/` is append-only benchmark history. Do not
delete, truncate, or rewrite cache files when re-benchmarking after code
changes. Run the benchmark again and append fresh rows.

C zstd results are stable across zrip code changes and rarely need rerunning.
Routine refreshes after zrip code changes benchmark only `zrip` and
`zrip paranoid` for the relevant chart inputs. Do not run `--impl all` unless
the user asks for all implementations, external baselines must be refreshed,
or the benchmark harness or corpus changed in a way that affects all
implementations.

### Paranoid mode (safe Rust, no unsafe)

The `paranoid` feature compiles zrip with `forbid(unsafe_code)`. SIMD dispatch
still works via `fearless_simd` (safe `#[target_feature]` wrappers). It must
be benchmarked as a separate build:

```bash
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release \
  --features paranoid
```

This produces `zrip_paranoid.jsonl` cache entries. Always run this after the
normal bench so the chart scripts have data for all codecs.

### Benchmarking all levels

zrip supports levels -8 through 4. The default bench runs all zrip levels.
To bench a subset of levels:

```bash
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --levels -1,1,3
```

## Charts

Rust chart generation lives in the `bench` crate and reads
`~/.cache/zrip/*.jsonl`:

| Command | Output | Description |
|---------|--------|-------------|
| `cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- scatter` | `scatter.svg` | Encode speed vs ratio geomean |
| `cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- summary` | `summary.svg` | Summary comparison table |
| `cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- matrix` | `matrix.svg` | Per-file/level heatmap matrix |
| `cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- pipeline` | `pipeline.svg` | Encode+decode pipeline throughput |
| `cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- small-encode` | `small_encode.svg` | Encode speed vs input size |
| `cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- small-decode` | `small_decode.svg` | Small decode comparison |

Chart refresh means benchmark first, appending new rows to the cache, then run
the chart tool. Re-running the chart tool against old cache data only replots;
it does not refresh the charts.

For normal zrip code changes, refresh only the changed implementations:

```bash
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release \
  --features paranoid
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --small-only
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --small-only --decode-only --levels 3 --impl zrip
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release \
  --features paranoid -- \
  --small-only --decode-only --levels 3 --impl zrip
```

That updates the `zrip` and `zrip paranoid` rows used by the charts without
rerunning stable external implementations.

### Regenerating charts

After a full-corpus benchmark run, including paranoid, regenerate only the
full-corpus charts:

```bash
cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- scatter
cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- summary
cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- matrix
cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- pipeline
```

The `all` chart command also renders small-input charts, so it needs
small-input encode and decode caches too. With no output dir, charts go under
`doc/charts/<arch>/`.

Optional local hardware labels can live in ignored `.chart_hw`:

```text
prefix=Linux VM on a 2018 Mac Mini
postfix=performance governor, turbo off
```

The chart tool reads `.chart_hw` from the current dir or parent dir. Env vars
`ZRIP_HW_PREFIX`, `ZRIP_HW_POSTFIX`, and `ZRIP_HW_EXTRAS` override or extend
local detection.

### Small-input benchmark + chart workflow

`small_encode.svg` only includes zrip, C zstd, and structured-zstd (no
paranoid). Default `--small-only` benchmarks zrip only:

```bash
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --small-only
cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- small-encode \
  doc/charts/x86_64/
```

### All-implementation benchmark + chart workflow

Use this only when refreshing all baselines, validating a benchmark harness
change, or when explicitly asked to re-run every implementation.

```bash
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --impl all
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release \
  --features paranoid
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --small-only --impl all
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --small-only --decode-only --levels 3
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release \
  --features paranoid -- \
  --small-only --decode-only --levels 3 --impl zrip
export ZRIP_HW_EXTRAS="performance governor,turbo off"
cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- all
```
