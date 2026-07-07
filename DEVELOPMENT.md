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

3. **Run any needed release audit.** Use the Miri and fuzz commands below
   when the release risk warrants an extended audit.

4. **Merge the release PR.** release-plz tags and publishes to crates.io
   automatically.

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

### Quick (synthetic data)

```bash
cargo bench                                            # binggan, in benches/
```

### Corpus benchmarks

```bash
cargo run --example zrip_bench --release        # zrip-only corpus bench
cargo run --example zrip_bench --release -- --impl all
```

Options: `--impl zrip|"C zstd"|all`, `--levels -8,1,3`,
`--files dickens.txt,mozilla`, `--extra /path/to/file`.

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
cargo run --example zrip_bench --release --features paranoid
```

This produces `zrip_paranoid.jsonl` cache entries. Always run this after the
normal bench so the chart scripts have data for all codecs.

### Benchmarking all levels

zrip supports levels -8 through 4. The default bench runs all zrip levels.
To bench a subset of levels:

```bash
cargo run --example zrip_bench --release
cargo run --example zrip_bench --release -- --levels -1,1,3
```

## Charts

Plotting scripts in `scripts/`, all reading from `~/.cache/zrip/*.jsonl`:

| Script | Output | Description |
|--------|--------|-------------|
| `plot_scatter.py` | `scatter.svg` | Encode speed vs ratio geomean |
| `plot_summary.py` | `summary.svg` | Summary comparison table |
| `plot_matrix.py` | `matrix.svg` | Per-file/level heatmap matrix |
| `plot_pipeline.py` | `pipeline.svg` | Encode+decode pipeline throughput |
| `plot_small.py` | `small_encode.svg` | Encode speed vs input size |
| `plot_small_decode.py` | `small_decode.svg` | Small decode comparison |

Chart refresh means benchmark first, appending new rows to the cache, then run
the plotting scripts. Re-running plot scripts against old cache data only
replots; it does not refresh the charts.

For normal zrip code changes, refresh only the changed implementations:

```bash
cd bench
cargo run --example zrip_bench --release
cargo run --example zrip_bench --release --features paranoid
cargo run --example zrip_bench --release -- --small-only
cargo run --example zrip_bench --release -- \
  --small-only --decode-only --levels 3 --impl zrip
cargo run --example zrip_bench --release --features paranoid -- \
  --small-only --decode-only --levels 3 --impl zrip
cd ..
```

That updates the `zrip` and `zrip paranoid` rows used by the charts without
rerunning stable external implementations.

### Regenerating all charts

After any full-corpus benchmark run, including paranoid, regenerate all
charts:

```bash
export ZRIP_HW_EXTRAS="performance governor,turbo off"
python3 scripts/plot_scatter.py doc/charts/x86_64/
python3 scripts/plot_summary.py doc/charts/x86_64/
python3 scripts/plot_matrix.py doc/charts/x86_64/
python3 scripts/plot_pipeline.py doc/charts/x86_64/
python3 scripts/plot_small.py doc/charts/x86_64/
python3 scripts/plot_small_decode.py doc/charts/x86_64/
```

The `ZRIP_HW_EXTRAS` env var is required when the CPU governor and turbo state
cannot be auto-detected (e.g. in a VM or container). It appends the given
labels to the hardware subtitle in the chart. On bare metal with sysfs access,
the script detects these automatically.

### Small-input benchmark + chart workflow

`small_encode.svg` only includes zrip, C zstd, and structured-zstd (no
paranoid). Default `--small-only` benchmarks zrip only:

```bash
cd bench
cargo run --example zrip_bench --release -- --small-only
cd ..
export ZRIP_HW_EXTRAS="performance governor,turbo off"
python3 scripts/plot_small.py doc/charts/x86_64/
```

### All-implementation benchmark + chart workflow

Use this only when refreshing all baselines, validating a benchmark harness
change, or when explicitly asked to re-run every implementation.

```bash
cd bench
cargo run --example zrip_bench --release -- --impl all
cargo run --example zrip_bench --release --features paranoid
cd ..
export ZRIP_HW_EXTRAS="performance governor,turbo off"
python3 scripts/plot_scatter.py doc/charts/x86_64/
python3 scripts/plot_summary.py doc/charts/x86_64/
python3 scripts/plot_matrix.py doc/charts/x86_64/
python3 scripts/plot_pipeline.py doc/charts/x86_64/
python3 scripts/plot_small.py doc/charts/x86_64/
python3 scripts/plot_small_decode.py doc/charts/x86_64/
```
