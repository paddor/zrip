# Design

zrip is a zstd codec in safe Rust. It encodes levels -7 to 4, the Fast and
DFast strategies, and decodes any standard zstd frame. Its niche is fast
compression for transfers, the role of LZ4, with standard zstd frames and
better ratios.

This file gives the high-level design. [SAFETY.md](SAFETY.md) covers the
no-`unsafe` approach, [DEVELOPMENT.md](DEVELOPMENT.md) build, test and
benchmark commands.

## Crates

| Crate | Path | Role |
|-------|------|------|
| `zrip-core` | `crates/core` | Bitstreams, FSE, Huffman, frame headers, XXH64, dictionary training |
| `zrip-encode` | `crates/encode` | Match finders, block encoder, streaming encoder, LDM, adaptive level |
| `zrip-decode` | `crates/decode` | Block and sequence decoding, streaming decoder |
| `zrip` | `src/lib.rs` | Facade that re-exports the public API |

Everything builds with `no_std` + `alloc`. Frames, streaming, LDM and
dictionary training need `std`. The `jsr/` and `npm/` packages wrap the
same code as WebAssembly.

## Encoder

### Pipeline

```
compress -> level_params_for_size -> per 128 KiB block:
  Fast or DFast match finder -> sequences
  -> block encoder: literals (raw or Huffman) + sequences (FSE)
  -> compressed block, or raw block if not smaller
```

`CompressContext` owns the hash tables, sequence buffer and block workspace,
and reuses them across calls.

### Levels

Parameters start from C zstd's `ZSTD_defaultCParameters`, except L-7 to L-4.
L-3 to L4 are retuned to search less in long literal runs.
Level 0 maps to level 1. Level -8 runs as L-7. Small inputs clamp the hash
and window logs to the input size, so tables stay cheap to clear.

| Level | Strategy | Hash table | Window | Min match | Target | Search strength |
|------:|:---------|:-----------|:-------|:---------:|-------:|----------------:|
| -7 | Fast | 32 KiB | 512 KiB | 8 | 2 | 4 |
| -6 | Fast | 32 KiB | 512 KiB | 8 | 9 | 5 |
| -5 | Fast | 32 KiB | 512 KiB | 8 | 5 | 6 |
| -4 | Fast | 32 KiB | 512 KiB | 7 | 4 | 5 |
| -3 | Fast | 64 KiB | 512 KiB | 6 | 4 | 5 |
| -2 | Fast | 64 KiB | 512 KiB | 6 | 3 | 5 |
| -1 | Fast | 64 KiB | 512 KiB | 6 | 2 | 5 |
| 1 | Fast | 128 KiB | 512 KiB | 7 | 1 | 4 |
| 2 | Fast | 256 KiB | 1 MiB | 7 | 1 | 4 |
| 3 | DFast | 512 + 256 KiB | 2 MiB | 5 | 1 | 0 |
| 4 | DFast | 2x 1 MiB | 16 MiB | 5 | 1 | 3 |

`Target` sets the search step. The step grows by one every
`1 << search_strength` bytes without a match.

- **L-7** aims at LZ4: raw literals and matches of eight bytes or more, so
  frames hold few sequences, no Huffman tables, and decode fast.
- **L-6 to L-4** hash 8 (or 7) bytes at shorter steps than C's negative
  levels. Longer matches leave fewer bytes to Huffman coding, the costly part.
- **L-3 to L2** grow their step faster than C, so long literal runs cost
  fewer probes. Frequent matches keep the initial step and most of the ratio.
  L1 and L2 hash seven bytes to favor longer matches.
- **L3 and L4** list the search strength for inputs above 256 KiB. L3 grows
  its step after each unsuccessful probe. One-shot calls and `CompressContext`
  use 4 (L3) and 6 (L4) for inputs up to 256 KiB, not counting a dictionary,
  because sparse initial tables cost too much ratio there. `FrameEncoder` does
  not know its input size and always uses the listed values.

### Fast match finder (`fast.rs`)

A port of C zstd's four-cursor `ZSTD_compressBlock_fast_noDict_generic`. Each
iteration probes two positions and reuses hashes across them. The rep offset
is checked at the step-ahead cursor. After a match, a loop takes rep matches
with no literals in between. Const generics fix the hash width (`MLS`) and,
for common table sizes, the hash log, so the hash shift is a constant and
table indexes need no bounds checks.

MLS 5 to 7 set only the hash width; a match needs four equal bytes, as in C.
MLS 8 is zrip's own and needs eight. A block that finds too few matches goes
out raw.

### DFast match finder (`dfast.rs`)

Follows C zstd's `ZSTD_compressBlock_doubleFast`: a long table of 8-byte
hashes and a short table of `min_match`-byte hashes. One position at a time:
rep 0 one byte ahead, then the long table, then the short one. A short match
also tries the long table at the next position.

### Block encoder (`block_encoder.rs`)

- **Sequences.** One pass packs all sequences into codes and extra bits and
  counts code frequencies. FSE encoding then walks the packed array. The
  encoder picks the smallest of predefined, custom and repeat tables. Blocks
  with under 64 sequences use predefined tables.
- **Literals.** Huffman with treeless reuse of the previous table, four
  streams from 1 KiB up. Literals stay raw when Huffman cannot win: order-0
  entropy above 7.97 bits per byte, or above 6.25 at L-6 to L-1. Long literal
  runs are judged on samples, as in C zstd.
- **Small inputs.** Building a Huffman table costs a few microseconds at any
  size. So L-6 to L-1 keep literals raw up to a per-level size (2 KiB at L-1
  to 12 KiB at L-6) and search denser to make up for it.
- **Fallback.** A block that does not shrink goes out raw, and the rep
  offsets roll back.

### Dictionaries

`PreparedDict` parses the dictionary once: content, rep offsets, and the
Huffman and FSE tables turned into encode tables for the first block. It
keeps a prefilled hash table of the dictionary content per minimum match the
level uses. `CompressContext::set_level()` prepares it again.

- **Attach** (up to 8 KiB, C zstd's fast-strategy limit, or one block
  outside the snapshot's size tier): the regular Fast finder with a `DICT`
  const flag. A fresh input table; misses fall back to the read-only dict
  table.
- **Copy** (larger, in the snapshot's tier): copy the prefilled table and
  search normally. Outside the tier, prefill per call. DFast always does this.

`zrip-core` trains dictionaries with FastCOVER or COVER and writes the
standard zstd dictionary format.

### Streaming (`streaming.rs`)

`FrameEncoder<W: Write>` buffers 128 KiB blocks and keeps earlier blocks in a
window buffer, so matches cross block boundaries. When the buffer passes
twice the window, it shifts the hash entries and compacts. `Options` set the
window log and LDM. `reset()` starts a new frame and keeps all buffers.

### Long distance matching (`ldm.rs`)

For windows beyond the level default, up to 128 MiB, the default with LDM on.
A gear rolling hash picks split points, about one position in
`2^hash_rate_log`. Each split
stores position and checksum in a bucketed table (up to 2^20 entries, 16 per
bucket) and probes for a confirmed match of 64 bytes or more. The regular
Fast or DFast finder fills the gaps between LDM matches. The output is plain
zstd sequences, so decoders need nothing special, only the larger window.

### Adaptive level (`adaptive.rs`)

`AdaptiveLevel` picks a level for a sender whose bottleneck moves, like C
zstd's `--adapt`. The caller records each frame in `CompressStats` and names
each window's bottleneck: output (go for ratio), CPU (go for speed) or idle
input. The level moves one step at a time, reverts a step that clearly hurts,
and settles on the best measured level. It allocates nothing and works
without `std`.

## Decoder

### Pipeline

```
decompress -> frame header -> per block:
  literals (raw, RLE or Huffman) + sequence tables (FSE)
  -> decode_execute_sequences: decode one sequence, copy its literals and match
```

`DecompressContext` reuses the output buffer and the block workspace.
`FrameDecoder<R: Read>` streams, keeps the last window of output as history,
and caps the window at 128 MiB.

### Sequence execution (`exec.rs`, `fast_vec.rs`)

One loop for all platforms, shaped after C zstd's
`ZSTD_decompressSequences_body`. Sequences are decoded and executed in the
same pass.

- **Bit reader.** `SeqBits` carries the stream's unread bits as one number
  instead of C's container, fill and position. That frees two registers in a
  loop that spills, about 7% faster.
- **Tables.** Each FSE row holds the next state and the value's baseline and
  extra bits. Tables are fixed-size and masked, and bit counts are a 32-variant
  enum, so LLVM knows their range.
- **Checks.** Three tests per sequence: the block limit, literals far enough
  from their end, and the offset. They imply the bounds of each copy's first
  chunk, so LLVM drops those checks. Anything unusual goes to a `#[cold]` exact
  path with the original error order.
- **Wild copies.** Literals and matches copy in 32-byte chunks, may write past
  the sequence into zeroed slack, and spread short offsets the C zstd way.
  Decoded literals carry 32 bytes of padding.

### Huffman (`core/src/huffman/decode_4stream.rs`)

Four streams decode interleaved, five symbols per stream per round. Codes of
at most five bits use a two-symbol table, like C zstd's `HUF_DEltX2`.

## Platform dispatch

The encoder wraps hot per-block functions in `simd_body!`, which runs them
through `fearless_simd`'s AVX2 path when the CPU has it. The decoder's
sequence loop uses `fearless_simd::dispatch!` (AVX2 + BMI2 on x86_64, NEON on
aarch64, SIMD128 on wasm32). Without `std` or `simd`, the scalar build runs.
No crate has `unsafe`; `fearless_simd` keeps the `#[target_feature]` calls.

## Out of scope

Greedy, Lazy, BtLazy2, BtOpt and BtUltra, so levels 5 to 22. zrip stays in
the Fast and DFast part of the curve, where encode speed matters more than
ratio.
