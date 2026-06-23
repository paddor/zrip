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

**Encapsulated unsafe.** All algorithm and control-flow code is
`#![forbid(unsafe_code)]`. Unsafe is confined to small, auditable primitives
modules with `debug_assert!` guards. The `paranoid` feature eliminates all
unsafe entirely, compiling pure safe Rust with zero SIMD intrinsics. See
[SAFETY.md](SAFETY.md).

**Small codebase.** ~12k lines of Rust, roughly a fifth of full-spec
implementations. Levels above 4 add complexity for compression ratios
that only matter in archival storage, not transfer pipelines.

**`no_std` + `alloc`.** Works in embedded and kernel contexts with the `alloc`
feature; `frame` requires `std`.

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
| `dict_builder` | no      | COVER/FastCOVER dictionary training            |
| `paranoid`     | no      | Pure safe Rust: no SIMD, no unchecked indexing  |
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
