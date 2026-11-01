# zrip

Pure Rust zstd codec. The decoder reads standard zstd blocks and frames
produced at any zstd compression level. The encoder supports levels -7 through
4 for fast transfer pipelines. First-class dictionary support.

## Why zrip

- **Speed.** Built for throughput. [Performance](#performance) compares it
  with C zstd and other Rust implementations.
- **Negative levels.** Levels -7 to -1 trade ratio for speed, as in C zstd.
- **No `unsafe`.** Every crate is `#![forbid(unsafe_code)]`. See
  [SAFETY.md](SAFETY.md).
- **Small.** About 15k lines of Rust. No levels above 4; their extra ratio
  matters for storage, not transfers.
- **Dictionaries.** Built-in COVER and FastCOVER training.
- **`no_std` + `alloc`.** The `frame` feature needs `std`.
- **WebAssembly.** [`@paddor/zrip`](https://jsr.io/@paddor/zrip) on JSR and
  npm, with WASM SIMD detection.

## Performance

![zstd pipeline benchmark](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/summary.svg)

<details>
<summary>x86_64 details (per-file pipeline, scatter, matrix)</summary>

![per-file pipeline](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/pipeline.svg)
![encode speed vs compression ratio](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/scatter.svg)
![per-file encode/decode matrix](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/matrix.svg)
![small input encode throughput](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/small_encode.svg)
![small input decode throughput](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/small_decode.svg)
</details>

No current aarch64 charts. If you have an aarch64 machine, run the benchmark
and chart commands from [`DEVELOPMENT.md`](DEVELOPMENT.md) and send a PR with
`doc/charts/aarch64/`.

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

// Reuse context state while writing into caller-owned buffers.
let mut frame = vec![0u8; zrip::compress_bound(input.len())];
let n = ctx.compress_into(input, &mut frame)?;
let mut owned = Vec::new();
dec.decompress_into(&frame[..n], &mut owned)?;
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

`.ldm(true)` without `window_log` uses a 128 MiB window (`window_log` 27), like
`zstd --long`. One-shot calls shrink it to the input. `FrameEncoder` does not
know its input size, so its frames always advertise 128 MiB, even short ones,
and decoders must allow that window. Set `window_log` for a smaller window.

`window_log` controls the maximum match distance (24 = 16 MiB, 27 = 128 MiB).
One-shot frames up to 128 MiB advertise their content size as the window, so
there LDM can reach the whole input. The main cost is memory: the encoder
allocates an LDM hash table of up to 8 MiB. `FrameEncoder` also buffers up to
twice the window, and `FrameDecoder` keeps one window. On a long stream at
`window_log=27` that is ~275 MiB to encode and ~130 MiB to decode. On data
without long-range repeats, LDM adds overhead with no ratio benefit.

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
| `std`          | yes     | Enables `CompressContext`, `FrameEncoder`, `FrameDecoder` |
| `frame`        | yes     | Frame header parsing and writing; implies `std` |
| `alloc`        | yes     | `no_std` + heap via `alloc` crate; includes `DecompressContext` |
| `ldm`          | yes     | Long distance matching for large-window compression |
| `dict_builder` | no      | COVER/FastCOVER dictionary training            |
| `simd`         | yes     | `fearless_simd` runtime dispatch (AVX2+BMI2, NEON, SIMD128) |
| `paranoid`     | no      | Keeps any future `unsafe` out; no effect today  |
| `nightly`      | no      | `#[optimize]` attributes on hot functions      |

## Safety

[SAFETY.md](SAFETY.md) describes how zrip stays fast without `unsafe` and
catalogs C zstd memory safety bugs that Rust prevents by construction.

All codec paths are fuzz-tested (17 targets). Fuzz targets cover round-trip
correctness, cross-validation against C zstd, streaming, dictionary modes,
and corruption resistance (bitflip, splice, truncate, overwrite).

## Design

[DESIGN.md](DESIGN.md) covers the encode/decode pipeline, performance
architecture, SIMD dispatch, compile-time specialization, and divergences
from C zstd.

## Levels

Level 0 means the default, level 1. [DESIGN.md](DESIGN.md) lists each
level's parameters.

`zrip::adaptive::AdaptiveLevel` picks the level at run time. The caller reports
each window's compression work and what limited it: output, CPU, or idle input.
It settles on the best measured level, which the caller applies with
`CompressContext::set_level()`.
