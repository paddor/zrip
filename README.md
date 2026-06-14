# zrip

Pure Rust zstd codec. Levels -7 through 4 (Fast and DFast strategies).
Optimized for encode throughput in transfer pipelines that need standard
zstd frames at high speed.

```toml
zrip = "0.1"
```

![zstd pipeline benchmark](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/summary.svg)

![encode speed vs compression ratio](https://raw.githubusercontent.com/paddor/zrip/main/doc/charts/x86_64/scatter.svg)

## Why zrip

**Fastest pure-Rust zstd encoder available.** 85% faster encode than
structured-zstd 0.0.37 at L3, 23% faster at L1. Faster decode than
structured-zstd at L3 (36%) and L-1 (12%). 3x faster decode than
ruzstd 0.8.2 at L1.

**Negative levels (-7..-1) for high-throughput pipelines.** Most zstd
libraries only expose levels 1+.

**`no_std` + `alloc`.** Works in embedded and kernel contexts with the `alloc`
feature; `frame` requires `std`.

**Dictionary compression.** COVER and FastCOVER training built in for
small-message workloads (log lines, JSON records, RPC payloads).

## Performance

Geomean across a 16-file Silesia + misc corpus on Intel i7-8700B (x86_64,
SSE2/AVX2), performance governor, turbo off. Ratio is `original / compressed`;
higher is better.

### zrip vs C zstd 1.5.7

**Compressible** (12-file geomean: Silesia text, XML, JSON, PDF, binaries)

| Level | Strategy | zrip enc | C enc | zrip dec | C dec | zrip ratio | C ratio |
|------:|:---------|:--------:|------:|:--------:|------:|:----------:|--------:|
|    -7 | Fast     | 329 MB/s |   479 | 790 MB/s |  1564 |      2.46x |   2.59x |
|    -6 | Fast     | 269 MB/s |   457 | 729 MB/s |  1517 |      2.87x |   2.71x |
|    -1 | Fast     | 241 MB/s |   364 | 661 MB/s |  1296 |      3.62x |   3.57x |
|     1 | Fast     | 238 MB/s |   347 | 631 MB/s |  1185 |      3.89x |   4.33x |
|     3 | DFast    | 176 MB/s |   237 | 748 MB/s |  1073 |      4.08x |   4.63x |
|     4 | DFast    | 173 MB/s |   231 | 748 MB/s |  1038 |      4.11x |   4.65x |

Encode is 59-75% of C zstd, decode 48-72%. Ratio trails C zstd by
~12% at L1-L4. The gap is pure Rust vs hand-tuned C with SIMD assembly.

**Incompressible** (3-file geomean: SAO star catalog, X-ray, MRI)

| Level | Strategy | zrip enc | C enc | zrip dec | C dec | zrip ratio | C ratio |
|------:|:---------|:--------:|------:|:--------:|------:|:----------:|--------:|
|    -7 | Fast     | 620 MB/s |  1055 |1380 MB/s |  4372 |      1.23x |   1.23x |
|    -6 | Fast     | 480 MB/s |   986 |1263 MB/s |  3979 |      1.35x |   1.24x |
|    -1 | Fast     | 277 MB/s |   622 |1005 MB/s |  3355 |      1.40x |   1.30x |
|     1 | Fast     | 197 MB/s |   348 | 774 MB/s |  1024 |      1.47x |   1.56x |
|     3 | DFast    | 114 MB/s |   112 | 752 MB/s |   721 |      1.57x |   1.73x |
|     4 | DFast    | 110 MB/s |   104 | 710 MB/s |   669 |      1.61x |   1.79x |

Encode is 45-59% of C zstd at negative levels, closing to parity at
L3-L4. Decode is 32-76% of C zstd. Both codecs produce near-1.0x
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

### Dictionary compression

```rust
let dict = zrip::Dictionary::from_bytes(&dict_bytes)?;
let compressed = zrip::compress_with_dict(input, 1, &dict)?;
let original   = zrip::decompress_with_dict(&compressed, &dict)?;
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

All compression and decompression logic is `#![forbid(unsafe_code)]`. Unsafe
is confined to two places:

- `unchecked.rs` modules inside `bitstream/`, `decode/`, `encode/`, `fse/`,
  `huffman/`: small `unsafe fn` wrappers (`get_unchecked`, `read_unaligned`)
  with `debug_assert!` guards, called only after block-level bounds checks.
- `simd/`: intrinsics and raw pointer arithmetic for wildcopy, copy-match,
  and the SIMD sequence decoder. Dispatch happens at block boundaries, not
  per-sequence.

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
