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

## JSR / WebAssembly

`jsr/build.sh` generates bundler bindings with native WASM imports. The npm
build requests web bindings separately and writes them into `npm/src/pkg`.

```bash
cd jsr
bash build.sh
deno task test
```

Use Deno 2.8.3 or newer for `deno bundle` and install `zstd` for cross-checks.
They cover the default SIMD path, forced scalar loading, synchronous bytes,
and initialization races. Bundled default initialization runs without file or
network permissions.

## Releasing

`release-plz` runs on every push to `main`
(`.github/workflows/release-plz.yml`). It opens or updates a release PR,
creates annotated tags after merge, publishes to crates.io, and creates
GitHub releases. Configuration lives in `release-plz.toml`.

Publishing uses crates.io trusted publishing through GitHub Actions OIDC.
Do not add a crates.io token secret.

### Steps

1. **Review the release-plz PR.** Verify semver bumps and dependency versions
   for every affected crate.

2. **Curate changelogs.** Insert a new `## [x.y.z]` section immediately below
   `## [Unreleased]` for each bumped crate and place its release notes there.
   Never rename or modify an existing versioned section.

3. **Bump the WASM package.** Update `jsr/deno.json` and
   `jsr/wasm/Cargo.toml`.
   Keep `npm/package.json` at the same WASM version.

4. **Build and verify.** Run the JSR commands below and any additional release
   audit warranted by the changes. Run `npm test` in `npm/` as well. Run the
   [performance gate](#performance-gate) with `--builds all` on a machine
   with floors.

5. **Merge the release PR after CI passes.** Monitor `release-plz` for crate
   publication and `.github/workflows/jsr.yml` for JSR publication.
   JSR rejects duplicate versions. Then publish npm using the tag or dispatch
   described below.

6. **Confirm publication.** Verify every intended crate version on crates.io
   and the new package version on each JavaScript registry. Run roundtrips
   against the published packages, including a bundled JSR roundtrip.

```bash
cd jsr
bash build.sh
deno task test
```

Use Deno 2.8.3 or newer for the bundled tests. They cover standalone bundles,
synchronous bytes, and initialization races. Bundled default initialization
runs without file or network permissions.

### npm

`@paddor/zrip` publishes to npm from `.github/workflows/release-npm.yml`.
The workflow version comes from a `zrip-npm-v*` tag or the manual
`workflow_dispatch` input. It builds the WASM package, runs Node tests, packs
the tarball, smoke-tests an installed tarball, then publishes from the `npm`
environment.

Publishing uses npm trusted publishing with GitHub Actions OIDC. In npm,
configure a trusted publisher for `@paddor/zrip` before tagging:

```text
GitHub owner: paddor
GitHub repository: zrip
Workflow filename: release-npm.yml
Environment name: npm
Allowed action: npm publish
```

If the package does not exist yet, trusted publishing may need a bootstrap
publish first. Add a publish-capable GitHub Actions secret named `NPM_TOKEN`,
run `.github/workflows/release-npm.yml`, then remove or rotate that token and
configure the trusted publisher before the next CI release.

```sh
git tag -a zrip-npm-v0.5.8 -m "zrip npm 0.5.8"
git push origin zrip-npm-v0.5.8
gh run watch --repo paddor/zrip --exit-status
```

## Fuzz

Fuzz targets live in `fuzz/fuzz_targets/`. Round-trip targets cross-validate
against C zstd. Corruption targets feed mutated compressed data to the decoder.

```bash
cargo +nightly fuzz run zrip_fuzz_roundtrip_frame -- -max_len=65536
cargo +nightly fuzz run zrip_fuzz_c_compress_zrip_decompress
```

## Pre-release fuzz run

Before tagging a release, run every fuzz target, corrupt targets first. The
code has no `unsafe`, so targets build without a sanitizer. They catch
panics, hangs, out-of-memory, and round-trip or C zstd mismatches.

The script defaults to the release durations. Targets that feed the decoder
arbitrary bytes (corrupt, push decode, dict decode paths, context reuse) run
20 minutes each, since they keep finding new paths for hours. Round-trip and
C zstd targets feed it valid frames and run 3 minutes each. That is about
4.5 hours on one host:

```bash
scripts/overnight_fuzz.sh
```

To split the run across hosts, give each one a shard. Shards take every n-th
target, so each gets an even share of long and short targets:

```bash
FUZZ_SHARD=1/3 scripts/overnight_fuzz.sh   # on host 1; 2/3 and 3/3 on the others
```

Each target's corpus persists in `fuzz/corpus/`, so fuzzing time adds up
across runs on the same host. Logs, artifacts, and TSV summaries are written
under `tmp/overnight-fuzz/$RUN_ID/`.

Useful controls:

```bash
SMOKE=1 scripts/overnight_fuzz.sh      # quick harness check
CPU_COUNT=8 scripts/overnight_fuzz.sh  # build/fuzz parallelism
FUZZ_SECONDS_LONG=3600 FUZZ_SECONDS_SHORT=300 scripts/overnight_fuzz.sh
FUZZ_SECONDS_PER_TARGET=600 scripts/overnight_fuzz.sh  # sets both
FUZZ_TARGETS="zrip_fuzz_push_decode zrip_fuzz_push_roundtrip" scripts/overnight_fuzz.sh
```

The dict decode test uses a pre-built fixture
(`tests/fixtures/corpus_dict_roundtrip.bin`). Regenerate after corpus changes:
`cargo test --features dict_builder -- fuzz_corpus_dict_generate`

### Push API soak

`tests/push_uninit_soak.rs` drives the push encoder and decoder in lockstep:
the same random calls go to `encode`/`decode` with initialized buffers and
to `encode_uninit`/`decode_uninit` with poisoned or truly uninitialized
ones (a vector's spare capacity). Every call must return the same result
and bytes, write nothing past `produced`, and leave the same state. Scenarios
cover every level, options, dictionaries, flushes, abandoned frames, buffers
of 0 B to 1 MiB, C zstd frames, skippable frames, output limits, and cut or
bit-flipped input. `cargo test` runs a short pass. Before a release, run the
timed soak, one process per core:

```bash
ZRIP_SOAK_SECS=1800 cargo test --release --test zrip_push_uninit_soak -- \
  --ignored --nocapture
```

A failure prints the case's `ZRIP_SOAK_SEED`, which replays it alone.
Under Miri, the short pass runs a few small cases without C zstd:
`cargo +nightly miri test --test zrip_push_uninit_soak`.

### Adversarial corpus

If you have an adversarial corpus of small/malformed zstd files (e.g. from
prior fuzzing campaigns), seed them into the corrupt_decompress target:

```bash
cargo +nightly fuzz run zrip_fuzz_corrupt_decompress /path/to/adversarial/corpus \
  -- -max_total_time=10800 -jobs=2
```

## Weekly extended CI

`.github/workflows/extended.yml` runs every Sunday. Each fuzz target gets its
own job running `scripts/overnight_fuzz.sh`: 30 minutes for decoder targets,
5 for round-trip targets. Corpora are cached from run to run. The workflow
also runs the ignored large-input tests and the push soak for 30 minutes,
one process per core. A failed fuzz job prints its log tails and uploads its
logs and crash inputs as an artifact. Start a run by hand with:

```bash
gh workflow run extended.yml -f fuzz_seconds=3600 -f soak_seconds=3600
```

Pull requests that change the workflow, the fuzz script, or `fuzz/` run a
short pass of every job.

## Performance gate

`zrip_perf_verify` measures throughput and ratio for the ways zrip is used:
one-shot calls, contexts reused or created per message, `compress_into`, push
streams with and without flushes, `FrameEncoder`, dictionary frames, and the
matching decoders (one-shot, contexts, `decompress_into`, `FrameDecoder`,
`PushDecoder`). Inputs run from 64 B messages to 4 MiB streams: generated
logs, sensor records, training metric points, and slices of every Silesia
file. It fails when a case falls below its floor. Each case runs in a fresh
worker process pinned to one CPU and reports the best of 7 rounds; a pass
over the 80 cases takes about 35 s per build.

```bash
cargo run --release --manifest-path bench/Cargo.toml --bin zrip_perf_verify -- \
  --builds all
```

Code generation differs between builds, so `--builds` builds the gate several
ways and runs each, serially:

| Build | Flags |
|-------|-------|
| `native` | the repository's `.cargo/config.toml`: `target-cpu=native` |
| `portable` | no rustflags, as dependent crates build: runtime SIMD dispatch |
| `portable-lto` | portable with fat LTO and one codegen unit |
| `paranoid` | native with the `paranoid` feature |

Without `--builds`, the gate runs as built, which is `native` from the
repository root. Most users get `portable`. A regression can show in only
one build: an `or_else` closure in the DFast loop stayed out of line only in
portable builds and cost them a third of their level 3 speed.

Floors are per machine and per build, in the ignored `.perf_hw` at the
repository root:

```text
[portable.context_l3]
16k_mb_s=187.6
16k_ratio=4.722
```

Keys are `<case>_mb_s` and, for encode cases, `<case>_ratio`. Without the
file, loose smoke floors catch only collapses. To set floors, run a
known-good build on an idle machine and review the file:

```bash
cargo run --release --manifest-path bench/Cargo.toml --bin zrip_perf_verify -- \
  --builds all --repeat 5 --suggest 0.95 > .perf_hw
```

`--suggest F` keeps `F` of each case's median throughput, or less for a case
whose `--repeat` runs spread widely: three spreads below the median, but no
less than 85%. On a 2018 Mac Mini VM, repeated runs of one binary spread by
0.5-0.8% at the median and 2-3% at the 90th percentile, and an unrelated
code change moved cases by up to 3.5%, so 0.95 catches drops of about 5% or
more without false alarms. Floors only hold for the machine and toolchain
they were measured on; reset them from a known-good build after a toolchain
update.

Options: `--builds LIST` (comma-separated, or `all`), `--list`, `--case NAME`
(a section or one case, repeatable), `--repeat N` (median of N worker
processes), and `--measure-only`. `ZRIP_PERF_CPU` picks the pinned CPU (`none`
to leave it unpinned), `ZRIP_PERF_ROUNDS` and `ZRIP_PERF_ROUND_MS` set the
timing, `ZRIP_PERF_HW` names another floors file, and `ZRIP_PERF_CORPUS`
another Silesia directory. Without the corpus, the `silesia_*` cases are
skipped. Each build goes to `perf-<build>` next to the gate's own target
directory.

Run the gate with `--builds all` before merging encoder or decoder changes
and before each release. A case below its floor is measured once more,
keeping the faster run. To compare two commits more finely than the floors
allow, build the gate in a worktree for each and run them alternately.

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

Options: `--impl zrip|"C zstd"|all`, `--levels -7,1,3`,
`--files dickens,mozilla`, `--extra /path/to/file`.

The benchmark harness prepares its own ignored corpus under `bench/corpus/`.
It downloads the 12 pinned Silesia inputs. `--small-only` slices the first bytes
from the base corpus in memory; no small corpus files need to be generated.
`--tiny` takes 64 slices of 512 B to 8 KiB from random offsets in every
Silesia file. The offsets are seeded by the input name, so every run picks the
same slices. Rows go to `~/.cache/zrip/<arch>/tiny/`.

Results cache under `~/.cache/zrip/` is append-only benchmark history. Do not
delete, truncate, or rewrite cache files when re-benchmarking after code
changes. Run the benchmark again and append fresh rows.

C zstd results are stable across zrip code changes and rarely need rerunning.
Routine refreshes after zrip code changes benchmark only `zrip` for the
relevant chart inputs. Do not run `--impl all` unless the user asks for all
implementations, external baselines must be refreshed, or the benchmark
harness or corpus changed in a way that affects all implementations.

### Benchmarking all levels

zrip supports levels -7 through 4. The default bench runs all zrip levels.
To bench a subset of levels:

```bash
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --levels -1,1,3
```

### WebAssembly benchmarks

The JSR benchmark measures the JavaScript API in Deno, including WASM memory
copies. It compares level 1 against `zstd-codec`, using the first 4 MiB of
each Silesia file. Throughput uses decimal MB/s:

```bash
cd jsr
bash build.sh
deno run --allow-read src/bench.ts
```

The WASI benchmark measures full Silesia files in wasmtime at levels -7
through 4. Override the repository's native CPU flags when building it:

```bash
RUSTFLAGS="-C target-feature=+simd128" cargo build \
  --manifest-path bench/wasm/Cargo.toml --target wasm32-wasip1 --release
wasmtime run \
  "${CARGO_TARGET_DIR:-bench/wasm/target}/wasm32-wasip1/release/zrip-bench-wasm.wasm" \
  --impl zrip --label dickens --json < bench/corpus/silesia/dickens
```

Run each of the 12 Silesia inputs serially. Append each JSON row to
`~/.cache/zrip/wasm32/L<level>/<codec>.jsonl`, replacing spaces in codec names
with underscores. Preserve existing cache history.

When refreshing the external baselines, set `WASI_SDK_PATH` to an installed
WASI SDK and build the additional codecs:

```bash
CC_wasm32_wasip1="$WASI_SDK_PATH/bin/clang" \
AR_wasm32_wasip1="$WASI_SDK_PATH/bin/llvm-ar" \
CFLAGS_wasm32_wasip1="-O3 -msimd128" \
RUSTFLAGS="-C target-feature=+simd128" \
cargo build --manifest-path bench/wasm/Cargo.toml --target wasm32-wasip1 \
  --release --features czstd,structured-zstd
```

Run that binary with `--impl all` for a baseline refresh. After appending
the measurements, regenerate the corpus charts:

```bash
for chart in scatter summary matrix pipeline; do
  cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- \
    "$chart" --profile wasm32
done
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
| `cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- tiny-matrix` | `tiny_matrix.svg` | Geomean pipeline stacks for random 512 B to 8 KiB Silesia slices, Fast levels |
| `cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- sweep` | `sweep.svg`, `sweep_l3.svg` | Compress, roundtrip and ratio vs slice size, with and without a dictionary, at levels 1 and 3 |

Chart refresh means benchmark first, appending new rows to the cache, then run
the chart tool. Re-running the chart tool against old cache data only replots;
it does not refresh the charts.

For normal zrip code changes, refresh only the changed implementations:

```bash
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --small-only
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --small-only --decode-only --levels 3 --impl zrip
```

That updates the `zrip` rows used by the charts without rerunning stable
external implementations.

### Regenerating charts

After a full-corpus benchmark run, regenerate only the full-corpus charts:

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

Each small input is up to 64 distinct consecutive slices of one size, and
each timed pass walks all of them. Repeating one slice lets the branch
predictor learn it and distorts results. `small_decode.svg` draws each
implementation decoding C zstd's L3 frames (thick) and its own L3 output
(thin, from the small encode rows).

`small_encode.svg` includes zrip, C zstd, structured-zstd, mzstd, rusty_zstd,
and lz4rip.
Default `--small-only` benchmarks zrip only:

```bash
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --small-only
cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- small-encode \
  doc/charts/x86_64/
```

`tiny_matrix.svg` stacks the pipeline for the `--tiny` slices by
compressibility at every Fast strategy level, -7 through 2. Each segment is
the geometric mean over all slice inputs in a group, so every size counts the
same. The transfer segment assumes a 1 GB/s link instead of 100 MB/s, since
tiny frames take only microseconds to compress. lz4rip has one level and shows
next to level -7:

```bash
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --tiny --levels -7,-6,-5,-4,-3,-2,-1,1,2
cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- tiny-matrix
```

### Dictionary sweep

`--sweep` runs zrip, C zstd, structured-zstd, mzstd and rusty_zstd, each with
and without a dictionary, on 64 consecutive slices of 64 B to 1 MiB from every
Silesia file, at levels 1 and 3. Each file gets one 16 KiB dictionary, trained
by C zstd on 1 KiB samples past the first MiB, so the slices up to 16 KiB are
not in its training set. Codecs reuse their compressor and decoder, except
rusty_zstd, which has neither. Rows hold per-message means and go to
`~/.cache/zrip/<arch>/sweep/`. `sweep.svg` (level 1) and `sweep_l3.svg`
(level 3) plot the geomean over the files:

```bash
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --sweep --impl all
cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- sweep
```

### All-implementation benchmark + chart workflow

Use this only when refreshing all baselines, validating a benchmark harness
change, or when explicitly asked to re-run every implementation.

```bash
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --impl all
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --small-only --impl all
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --small-only --decode-only --levels 3
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --tiny --impl all --levels -7,-6,-5,-4,-3,-2,-1,1,2
cargo run --manifest-path bench/Cargo.toml --example zrip_bench --release -- \
  --sweep --impl all
export ZRIP_HW_EXTRAS="performance governor,turbo off"
cargo run --manifest-path bench/Cargo.toml --bin zrip_charts -- all
```
