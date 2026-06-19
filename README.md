# zrip

Pure Rust zstd codec. Levels -7 through 4 (Fast and DFast strategies).
Optimized for encode throughput in transfer pipelines that need standard
zstd frames at high speed.

![zstd pipeline benchmark](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/summary.svg)

<details>
<summary>x86_64 details (per-file pipeline, scatter, matrix)</summary>

![per-file pipeline](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/pipeline.svg)
![encode speed vs compression ratio](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/scatter.svg)
![per-file encode/decode matrix](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/matrix.svg)
</details>

<details>
<summary>aarch64 (Apple M4)</summary>

![aarch64 pipeline summary](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/aarch64/summary.svg)
![aarch64 per-file pipeline](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/aarch64/pipeline.svg)
![aarch64 encode speed vs compression ratio](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/aarch64/scatter.svg)
![aarch64 per-file encode/decode matrix](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/aarch64/matrix.svg)
</details>

## Why zrip

**Fastest pure-Rust zstd encoder available.** 72% faster encode than
structured-zstd 0.0.40 at L3, 33% faster at L1. Faster decode than
structured-zstd at L3 (40%) and L-1 (21%). 2.9x faster decode than
ruzstd 0.8 at L1.

**Negative levels (-7..-1) for high-throughput pipelines.** Most zstd
libraries only expose levels 1+.

**`no_std` + `alloc`.** Works in embedded and kernel contexts with the `alloc`
feature; `frame` requires `std`.

**Dictionary compression.** COVER and FastCOVER training built in for
small-message workloads (log lines, JSON records, RPC payloads).

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
| `nightly`      | no      | `#[optimize]` attributes on hot functions      |

## Safety

zrip uses unsafe for performance, not a zero-unsafe codebase. All algorithm
and control-flow code is `#![forbid(unsafe_code)]`. Unsafe is confined to
small, auditable leaf modules:

- `primitives.rs` modules in `bitstream/`, `huffman/`, `encode/`, `decode/`:
  `#[inline(always)]` wrappers around `get_unchecked`, `read_unaligned`,
  `set_len`, and `copy_nonoverlapping` with `debug_assert!` guards.
- `simd/` and `simd_decode/`: intrinsics and raw pointer arithmetic for
  wildcopy, copy-match, and the fused SIMD sequence decoder.
- `huffman/decode_4stream.rs`: pointer-based interleaved 4-stream Huffman
  decoder.

## Levels

| Level | Strategy | Hash table | Literals | Sequences | Notes |
|------:|:---------|:-----------|:---------|:----------|:------|
| -7 | Fast | 32 KB | Raw | Predefined FSE | Max throughput, no entropy coding |
| -6..-1 | Fast | 32 KB | Huffman | Predefined/custom FSE | Standard encode pipeline |
| 1 | Fast | 64 KB | Huffman | Predefined/custom FSE | 7-byte min match |
| 2 | Fast | 256 KB | Huffman | Predefined/custom FSE | 6-byte min match, 1 MB window |
| 3 | DFast | 2x 128 KB | Huffman | Predefined/custom FSE | Dual hash (short + long matches) |
| 4 | DFast | 2x 256 KB | Huffman | Predefined/custom FSE | Best ratio in this crate |

Level 0 maps to the library default (currently level 1).

**L-7** skips Huffman table construction and always emits raw literal blocks
with predefined FSE tables. This eliminates the most expensive part of the
encode pipeline (Huffman tree build, stream encoding, custom FSE table
estimation) at the cost of compression ratio. The result is a valid zstd
frame that any decoder handles, but with LZ4-class encode throughput.

**L-6 through L2** use the full encode pipeline: Huffman-compressed literals
(with treeless reuse across blocks) and predefined or custom FSE tables for
sequences, whichever produces smaller output.

**L3 and L4** use the DFast strategy with two hash tables (short 4-byte and
long 8-byte matches) for better match quality at lower throughput.
