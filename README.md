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

| Level | Strategy | zrip enc | C enc | zrip dec | C dec | zrip ratio | C ratio |
|------:|:---------|:--------:|------:|:--------:|------:|:----------:|--------:|
|    -7 | Fast     | 312 MB/s |   559 | 854 MB/s |  1906 |      2.39x |   2.24x |
|    -1 | Fast     | 248 MB/s |   404 | 727 MB/s |  1560 |      2.99x |   2.91x |
|     1 | Fast     | 229 MB/s |   346 | 663 MB/s |  1150 |      3.20x |   3.53x |
|     2 | Fast     | 207 MB/s |   279 | 620 MB/s |  1025 |      3.29x |   3.66x |
|     3 | DFast    | 164 MB/s |   203 | 755 MB/s |   989 |      3.35x |   3.80x |
|     4 | DFast    | 165 MB/s |   196 | 754 MB/s |   948 |      3.39x |   3.84x |

Encode is 56-84% of C zstd depending on level. Decode is 45-80% of C zstd.
Ratio beats C zstd at negative levels (larger hash table finds more matches),
widening to ~12% behind at L4. The encode gap is pure Rust vs hand-tuned C
with SIMD assembly in its hot paths.

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

| Level | Strategy | Notes                                    |
|------:|:---------|:-----------------------------------------|
|    -7 | Fast     | Fastest; 16 KiB window, aggressive skip  |
|  -6..-1 | Fast   | Progressively larger target lengths      |
|     1 | Fast     | Standard L1; 512 KiB window              |
|     2 | Fast     | Larger window, 5-byte min match          |
|     3 | DFast    | Dual hash table (short + long)           |
|     4 | DFast    | Larger window; best ratio in this crate  |

Level 0 is not valid; use 1 for the standard fast level. All levels match
C zstd's `ZSTD_defaultCParameters` table.
