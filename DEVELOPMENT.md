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

## Fuzz

Fuzz targets live in `fuzz/fuzz_targets/`. Round-trip targets cross-validate
against C zstd. Corruption targets feed mutated compressed data to the decoder.

```bash
cargo +nightly fuzz run roundtrip_frame -- -max_len=65536
cargo +nightly fuzz run c_compress_zrip_decompress
```

## Benchmarks

### Quick (synthetic data)

```bash
cargo bench                                            # binggan, in benches/
```

### Corpus benchmarks

```bash
cargo run --example zrip_bench --release               # zrip-only, default levels (L1+L3)
cargo run --example zrip_bench --release -- --impl all  # include C zstd baseline
```

Options: `--impl zrip|"C zstd"|all`, `--levels 1,3,4`,
`--files dickens.txt,mozilla`, `--extra /path/to/file`.

Results cache in `~/.cache/zrip/{codec}.jsonl`. Delete the relevant file
before re-benchmarking after code changes:

```bash
rm ~/.cache/zrip/zrip.jsonl
```

C zstd results are stable across zrip code changes and rarely need rerunning.

### Benchmarking all levels

zrip supports levels -7 through 4. The default bench runs L1 and L3 only.
To bench additional levels:

```bash
rm ~/.cache/zrip/zrip.jsonl
cargo run --example zrip_bench --release                          # L1 + L3
cargo run --example zrip_bench --release -- --levels -7,-6,-5,-4,-3,-2,-1,2,4  # the rest
```

## Charts

Three plotting scripts in `scripts/`, all reading from `~/.cache/zrip/*.jsonl`:

| Script | Output | Description |
|--------|--------|-------------|
| `plot_scatter.py` | `scatter.svg` | Encode speed vs compression ratio (geomean) |
| `plot_bench.py` | `bench.svg` | Per-file bar chart |
| `plot_summary.py` | `summary.svg` | Summary comparison table |

### Regenerating the scatter chart

```bash
ZRIP_HW_EXTRAS="performance governor,turbo off" python3 scripts/plot_scatter.py
```

Output goes to `doc/charts/{arch}/scatter.svg`.

The `ZRIP_HW_EXTRAS` env var is required when the CPU governor and turbo state
cannot be auto-detected (e.g. in a VM or container). It appends the given
labels to the hardware subtitle in the chart. On bare metal with sysfs access,
the script detects these automatically.
