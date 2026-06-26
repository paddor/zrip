# zrip

Pure Rust zstd codec. Levels -7 through 4 (Fast and DFast strategies).
Optimized for encode throughput in transfer pipelines that need standard
zstd frames at high speed.

## Why zrip

**Fastest pure-Rust zstd encoder.** Consistently outperforms other pure-Rust
zstd implementations on both encode and decode across all supported levels.
See the [benchmarks below](#performance).

**Negative levels (-7 through -1).** Unlocks zstd's fastest compression tiers,
useful when throughput matters more than ratio.

**Safe decoder.** The entire decode crate is `#![forbid(unsafe_code)]` with zero
`unsafe` blocks. Platform dispatch uses `fearless_simd` for safe runtime
multiversioning. Encoder unsafe is confined to small, auditable `primitives.rs`
with `debug_assert!` guards. The `paranoid` feature eliminates all remaining
unsafe. See [SAFETY.md](SAFETY.md).

**Small codebase.** ~12k lines of Rust. Levels above 4 add complexity for
compression ratios that only matter in archival storage, not transfer
pipelines.

**`no_std` + `alloc`.** Works in embedded and kernel contexts with the `alloc`
feature; `frame` requires `std`.

**WebAssembly.** Available as [`@paddor/zrip`](https://jsr.io/@paddor/zrip)
on JSR. Auto-detects WASM SIMD support. 15% faster encode than C zstd compiled
to WASM.

**Dictionary compression.** COVER and FastCOVER training built in for
small-message workloads (log lines, JSON records, RPC payloads).

## Performance

![zstd pipeline benchmark](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/summary.svg)

<details>
<summary>x86_64 details (per-file pipeline, scatter, matrix)</summary>

![per-file pipeline](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/pipeline.svg)
![encode speed vs compression ratio](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/scatter.svg)
![per-file encode/decode matrix](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/matrix.svg)
![small input encode throughput](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/small.svg)
</details>

<details>
<summary>aarch64 (Apple M4)</summary>

![aarch64 pipeline summary](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/aarch64/summary.svg)
![aarch64 per-file pipeline](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/aarch64/pipeline.svg)
![aarch64 encode speed vs compression ratio](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/aarch64/scatter.svg)
![aarch64 per-file encode/decode matrix](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/aarch64/matrix.svg)
</details>

<details>
<summary>wasm32 (wasmtime)</summary>

![wasm32 pipeline summary](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/wasm32/summary.svg)
![wasm32 per-file pipeline](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/wasm32/pipeline.svg)
![wasm32 encode speed vs compression ratio](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/wasm32/scatter.svg)
![wasm32 per-file encode/decode matrix](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/wasm32/matrix.svg)
</details>

## API

```rust
// One-shot (allocating)
let compressed = zrip::compress(input, 1)?;
let original   = zrip::decompress(&compressed)?;

// One-shot into caller buffer
let n = zrip::compress_into(input, &mut output_buf, 1)?;
zrip::decompress_into(&compressed, &mut output_vec)?;

// Reusable context (amortizes table allocation across calls)
let mut ctx = zrip::CompressContext::new(1)?;
let compressed = ctx.compress(input)?;

let mut dec = zrip::DecompressContext::new();
let original = dec.decompress(&compressed)?;
```

### Streaming

```rust
use std::io::Write;

let mut enc = zrip::FrameEncoder::new(Vec::new(), 1)?;
enc.write_all(b"hello")?;
enc.write_all(b" world")?;
let compressed = enc.finish()?;

use std::io::Read;
let mut dec = zrip::FrameDecoder::new(&compressed[..]);
let mut out = String::new();
dec.read_to_string(&mut out)?;
```

`FrameEncoder` and `FrameDecoder` own persistent hash tables and
workspace buffers. Call `reset()` to start a new frame while reusing
all allocations:

```rust
let mut enc = zrip::FrameEncoder::new(Vec::new(), 1)?;
enc.write_all(b"first frame")?;
let first  = enc.reset(Vec::new())?;  // finishes frame, keeps buffers
enc.write_all(b"second frame")?;
let second = enc.finish()?;
```

### Large-window and long distance matching

The normal match finder operates within a sliding window (512 KiB at L1).
LDM finds matches at distances up to `1 << window_log` bytes by sampling
positions into a separate hash table. Useful for data with long-range
repeats: log files, database dumps, source archives. See
[DESIGN.md](DESIGN.md) for how LDM works.

```rust
let opts = zrip::Options::default().window_log(24).ldm(true);
let compressed = zrip::compress_opts(input, 1, &opts)?;
```

Streaming with LDM:

```rust
use std::io::Write;
let opts = zrip::Options::default().window_log(24).ldm(true);
let mut enc = zrip::FrameEncoder::with_options(Vec::new(), 1, &opts)?;
enc.write_all(input)?;
let compressed = enc.finish()?;
```

`window_log` controls the maximum match distance (24 = 16 MiB, 27 = 128 MiB).
The main cost is memory: the encoder allocates an 8 MiB LDM hash table plus
a window buffer of `1 << window_log` bytes, and the decoder allocates a
window buffer of the same size (declared in the frame header). At
`window_log=27` that is ~136 MiB on each side. On data without long-range
repeats, LDM adds overhead with no ratio benefit.

### Dictionary compression

```rust
let dict = zrip::Dictionary::from_bytes(&dict_bytes)?;
let compressed = zrip::compress_with_dict(input, 1, &dict)?;
let original   = zrip::decompress_with_dict(&compressed, &dict)?;
```

Streaming with a dictionary:

```rust
let mut enc = zrip::FrameEncoder::with_dict(Vec::new(), 1, dict.clone())?;
enc.write_all(input)?;
let compressed = enc.finish()?;

let mut dec = zrip::FrameDecoder::with_dict(&compressed[..], dict);
let mut output = Vec::new();
dec.read_to_end(&mut output)?;
```

`CompressContext::with_dict()` and `DecompressContext::with_dict()`
provide the same reuse for one-shot compression.

### Dictionary training

Build a dictionary from sample data using the built-in FastCOVER trainer.
Requires the `dict_builder` feature.

```rust
use zrip::dict::{train_dict_fastcover, fastcover::FastCoverParams};

let samples: Vec<&[u8]> = messages.iter().map(|m| m.as_bytes()).collect();
let dict = train_dict_fastcover(&samples, 16384, FastCoverParams::default());
// Use with compress_with_dict() / decompress_with_dict()
```

## Features

| Feature        | Default | Description                                   |
|:---------------|:-------:|:----------------------------------------------|
| `std`          | yes     | Enables `CompressContext`, `DecompressContext` |
| `frame`        | yes     | Frame header parsing and writing; implies `std` |
| `alloc`        | yes     | `no_std` + heap via `alloc` crate              |
| `ldm`          | yes     | Long distance matching for large-window compression |
| `dict_builder` | no      | COVER/FastCOVER dictionary training            |
| `simd`         | yes     | `fearless_simd` runtime dispatch (AVX2+BMI2, NEON, SIMD128) |
| `paranoid`     | no      | Pure safe Rust: `forbid(unsafe_code)` on all crates    |
| `nightly`      | no      | `#[optimize]` attributes on hot functions      |

## Safety

[SAFETY.md](SAFETY.md) documents the unsafe boundary and catalogs C zstd
memory safety bugs that Rust prevents by construction.

All codec paths are fuzz-tested (16 targets, ~10.7M executions) and verified
under Miri on both x86_64 and aarch64. Fuzz targets cover round-trip
correctness, cross-validation against C zstd, streaming, dictionary modes,
and corruption resistance (bitflip, splice, truncate, overwrite).

## Design

[DESIGN.md](DESIGN.md) covers the encode/decode pipeline, SIMD dispatch,
compile-time specialization, and divergences from C zstd.

## Levels

| Level | Strategy | Hash table | Min match | Literals | Sequences |
|------:|:---------|:-----------|:---------:|:---------|:----------|
| -7 | Fast | 32 KB | 5 | Raw | Predefined FSE |
| -6..-1 | Fast | 32 KB | 5 | Huffman | Predefined/custom FSE |
| 1 | Fast | 64 KB | 4 | Huffman | Predefined/custom FSE |
| 2 | Fast | 256 KB | 4 | Huffman | Predefined/custom FSE |
| 3 | DFast | 2x 128 KB | 4 | Huffman | Predefined/custom FSE |
| 4 | DFast | 2x 256 KB | 4 | Huffman | Predefined/custom FSE |

Level 0 maps to the library default (currently level 1). See
[DESIGN.md](DESIGN.md) for parameter details and pipeline behavior per level.
