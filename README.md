# zrip

Pure Rust zstd codec. Levels -7 through 4 (Fast and DFast strategies).
Optimized for encode throughput in transfer pipelines that need standard
zstd frames at high speed.

![zstd pipeline benchmark](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/summary.svg)

<details>
<summary>Encode speed vs compression ratio</summary>

![encode speed vs compression ratio](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/scatter.svg)
</details>

<details>
<summary>Encode/decode throughput by level and compressibility</summary>

![per-file encode/decode matrix](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/matrix.svg)
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

## Performance

Geomean across a 15-file Silesia + misc corpus on Intel i7-8700B (x86_64,
SSE2/AVX2), performance governor, turbo off. Ratio is `original / compressed`;
higher is better.

### zrip vs C zstd 1.5.7

**Compressible** (12-file geomean: Silesia text, XML, JSON, PDF, binaries)

| Level | Strategy | zrip enc | C enc | zrip dec | C dec | zrip ratio | C ratio |
|------:|:---------|:--------:|------:|:--------:|------:|:----------:|--------:|
|    -7 | Fast     | 385 MB/s |   485 | 981 MB/s |  1576 |      2.37x |   2.56x |
|    -6 | Fast     | 323 MB/s |   461 | 885 MB/s |  1528 |      2.69x |   2.68x |
|    -1 | Fast     | 276 MB/s |   364 | 777 MB/s |  1297 |      3.51x |   3.51x |
|     1 | Fast     | 247 MB/s |   345 | 614 MB/s |  1180 |      3.87x |   4.32x |
|     3 | DFast    | 190 MB/s |   233 | 788 MB/s |  1045 |      4.05x |   4.62x |
|     4 | DFast    | 189 MB/s |   227 | 786 MB/s |  1009 |      4.08x |   4.65x |

Encode is 72-82% of C zstd, decode 52-75%. Ratio trails C zstd by
~10% at L1-L4. The gap is pure Rust vs hand-tuned C with SIMD assembly.

**Incompressible** (3-file geomean: SAO star catalog, X-ray, MRI)

| Level | Strategy | zrip enc | C enc | zrip dec | C dec | zrip ratio | C ratio |
|------:|:---------|:--------:|------:|:--------:|------:|:----------:|--------:|
|    -7 | Fast     |1074 MB/s |   990 |2948 MB/s |  3996 |      1.26x |   1.30x |
|    -6 | Fast     |1060 MB/s |   926 |2974 MB/s |  3669 |      1.30x |   1.30x |
|    -1 | Fast     | 484 MB/s |   610 |1814 MB/s |  3216 |      1.38x |   1.38x |
|     1 | Fast     | 236 MB/s |   357 | 978 MB/s |  1023 |      1.49x |   1.58x |
|     3 | DFast    | 133 MB/s |   116 |1008 MB/s |   778 |      1.54x |   1.74x |
|     4 | DFast    | 123 MB/s |   108 | 917 MB/s |   724 |      1.58x |   1.80x |

Encode is 79% of C zstd at negative levels, closing to parity at
L3-L4. Decode is 74-96% of C zstd. Both codecs produce near-1.0x
ratios, so throughput is the only differentiator here.

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
