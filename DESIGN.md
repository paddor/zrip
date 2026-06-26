# Design

Architecture and divergences from C zstd. Each section explains what zrip does,
how it differs, and why.

## Encode pipeline

### Fast strategy (`crates/encode/src/fast.rs`)

Port of C zstd's `ZSTD_compressBlock_fast_noDict_generic` 4-cursor match finder.
The pipeline probes positions ip0, ip1=ip0+1, ip2=ip0+step, ip3=ip2+1 per
iteration, reusing hash computations across shifts. Rep offset is checked at the
step-ahead position (ip2) only. Rep2 is checked in a post-match loop
(`rep2_match_loop`), with 1-byte backward extension on rep matches (C zstd style).

Const-generic dispatch on minimum match length via
`compress_fast_block_impl::<HASH_LOG, MLS>`:

| MLS | Hash function | Used at levels |
|-----|---------------|----------------|
| 4   | `hash4` (PRIME32_1) | 1 |
| 5   | `hash5` (PRIME64_1) | -6..-1 |
| 7   | `hash7` (custom PRIME7) | -7 |

Hash table prefetch on x86_64 (`_mm_prefetch`) and aarch64 (inline asm
`prfm pldl1keep`) hides memory latency in the match-finding loop.

Incompressibility bail-out: at probe intervals, if
`total_match_bytes * 6 < scanned`, the compressor clears sequences and returns
early (the block is emitted raw).

### DFast strategy (`crates/encode/src/dfast.rs`)

Dual hash tables: short (4-byte matches, `chain_log`) + long (8-byte matches,
`hash_log`). Same 4-cursor pipeline adapted for two tables with prefetch for
both. `search_log` controls when to try improving a short match with a long
match. Same incompressibility bail-out as Fast.

### Level parameters (`crates/encode/src/strategy.rs`)

Match C zstd's `ZSTD_defaultCParameters` table for all implemented levels:

| Level | Strategy | Hash table | Window | Min match | Target length | Notes |
|------:|:---------|:-----------|:-------|:---------:|:-------------:|:------|
| -7 | Fast | 32 KB | 512 KB | 5 | 8 | `force_raw_literals` |
| -6 | Fast | 32 KB | 512 KB | 5 | 7 | |
| -5 | Fast | 32 KB | 512 KB | 5 | 6 | |
| -4 | Fast | 32 KB | 512 KB | 5 | 5 | |
| -3 | Fast | 32 KB | 512 KB | 5 | 4 | |
| -2 | Fast | 32 KB | 512 KB | 5 | 3 | |
| -1 | Fast | 32 KB | 512 KB | 5 | 2 | |
| 1 | Fast | 64 KB | 512 KB | 4 | 1 | |
| 2 | Fast | 256 KB | 1 MB | 4 | 1 | |
| 3 | DFast | 2x 128 KB | 2 MB | 4 | 1 | Dual hash |
| 4 | DFast | 2x 256 KB | 8 MB | 4 | 1 | |

Negative levels trade ratio for throughput by increasing `target_length`
(skip acceleration). Higher `target_length` means more positions are skipped
on consecutive match-finding misses.

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

## Block encoder decisions

`crates/encode/src/block_encoder.rs` makes per-block choices:

**Literals**: Level -7 always emits raw literal blocks (skips Huffman tree
construction entirely). All other levels try Huffman-compressed literals with
treeless reuse across blocks via `prev_huffman`.

**Sequences**: Blocks with >= 64 sequences estimate both predefined and custom
FSE table cost, pick whichever is smaller. Blocks with < 64 sequences always use
predefined tables (custom table header overhead exceeds savings).

**Incompressibility fast path**: `block_looks_incompressible()` samples the first
1024 bytes. If >= 200 distinct byte values AND max frequency <= sample/24, the
block is emitted raw without running the match finder.

**Fallback**: If the compressed block >= source size, rep offsets are reverted and
a raw block is emitted.

## CPU feature dispatch

Two dispatch mechanisms coexist:

**Decoder:** `fearless_simd::dispatch!` with a cached `OnceLock<Level>`. The
macro compiles `decode_execute_sequences` with platform-specific target features
(AVX2+BMI2 on x86_64, NEON on aarch64, SIMD128 on wasm32) and selects the best
version at runtime. Zero `unsafe` in user code. With `paranoid` or without
`std`, the scalar path runs directly.

**Huffman BMI2:** `crates/core/src/simd/mod.rs` defines a `CpuTier` enum
(`Bmi2`/`Avx2` on x86_64). Runtime detection cached in `OnceLock<CpuTier>`.
The Huffman 4-stream decoder dispatches to a `#[target_feature(enable = "bmi2")]`
variant for faster bit extraction.

## Sequence decoder

`crates/decode/src/exec.rs` (`#![forbid(unsafe_code)]`).

A single safe Rust implementation handles all platforms. FSE state machine +
sequence execution using `extend_from_slice` for literals and
`extend_from_within` for match copies (with doubling strategy for overlapping
copies and RLE special-case for offset-1). `fearless_simd::dispatch!` compiles
the function with platform target features, enabling the compiler to generate
BMI2 bit extraction instructions (`shlx`, `shrx`) and auto-vectorize bitstream
operations. Standard library `memcpy`/`memmove` already use platform-optimal
SIMD for copies.

## Huffman 4-stream decoder

`crates/core/src/huffman/decode_4stream.rs`.

Interleaved 4-stream parallel decode: the compressed literals are split into 4
segments, each with an independent `ReverseBitReader`. The fast loop decodes 5
symbols per stream per iteration (20 symbols total), checking both output bounds
and bitstream bounds. Index-based output with safe slice indexing and inline
`refill!`/`decode_one!` macros.

BMI2 variant via `#[target_feature(enable = "bmi2")]` for faster bit extraction.
Slow path finishes remaining symbols one stream at a time near exhaustion.

## `paranoid` feature

`cargo build --features paranoid` adds `#![forbid(unsafe_code)]` to all four
crate roots. The decoder is already safe by default; `paranoid` disables
`fearless_simd` dispatch (scalar path only) and affects encoder and core
`primitives.rs` modules. Each has dual `#[cfg(feature = "paranoid")]`
/ `#[cfg(not(feature = "paranoid"))]` bodies: the paranoid path uses direct
indexing, `from_le_bytes`, `resize`, and `extend_from_slice` in place of
unchecked operations. Huffman BMI2 `#[target_feature]` dispatch is gated out.

## Unsafe boundary

The decoder implementation is `#![forbid(unsafe_code)]` with zero `unsafe`
blocks. Platform dispatch is handled by `fearless_simd::dispatch!`, which
encapsulates `#[target_feature]` calling internally.

Encoder unsafe is confined to `encode/src/primitives.rs`: `#[inline(always)]`
wrappers around `get_unchecked`, `read_unaligned`, `set_len`. Each function has
a `debug_assert!` guard. Pattern:
```rust
debug_assert!(pos + 4 <= src.len());
unsafe { (src.as_ptr().add(pos) as *const u32).read_unaligned() }
```

Core crate unsafe follows the same pattern in `bitstream/`, `huffman/`,
`xxhash/` primitives modules.

`assert_rep_valid` uses both `debug_assert!` and `core::hint::assert_unchecked`
to help the optimizer prove bounds.

## Compile-time specialization

Const generics eliminate dead code at compile time:

- `compress_fast_block_impl::<HASH_LOG, MLS>`: LLVM specializes per MLS (4, 5, 7),
  dead-code eliminates unused hash/match paths.
- `compress_dfast_block_impl::<HASH_LOG, SHORT_LOG, MLS>`: same for DFast.
- `hash_pos::<HASH_LOG, MLS>`: selects hash4/hash5/hash7 at compile time.
- Strategy dispatch (`Fast` vs `DFast`) happens at block boundaries, not
  per-position.

## Dictionary support

`train_dict_fastcover()` in `crates/core/src/dict/mod.rs`: FastCOVER segment
selection + `finalize_dictionary()` produces standard zstd dictionary format
(magic + dict_id + Huffman table + FSE tables + rep offsets + content).
`FastCoverParams` defaults: k=2048, d=8, accel=1. Full COVER also exists in
`core/src/dict/cover.rs`.

Encoder prefix mode: concatenates dict content + source, pre-fills hash table
from dict content, then runs the normal match finder over the combined buffer.

## Streaming

`crates/encode/src/streaming.rs`: `FrameEncoder<W: Write>` buffers input up to
128 KiB (`MAX_BLOCK_SIZE`), then flushes as a compressed block. Frame header
includes magic, descriptor (with content checksum flag), window descriptor, and
optional dict ID. `finish()` flushes the final block with `last=true` and writes
the XXH64 content checksum (lower 32 bits).

Cross-block matching: the encoder maintains a persistent window buffer
(`window_buf`) containing previous blocks' output. Hash table positions reference
into this combined view, allowing matches across block boundaries. When the
window buffer grows past the window size, positions are shifted and the buffer
is compacted.

`Options` API controls window size and LDM: `Options::default().window_log(24).ldm(true)`.
`FrameEncoder::with_options(writer, level, &opts)` applies these to the frame.

`reset()` finishes the current frame, swaps the writer, and reuses all internal
buffers (hash tables, workspace, block encoder).

First block with a dictionary uses `compress_fast_with_prefix_reuse` /
`compress_dfast_with_prefix_reuse`. Subsequent blocks clear hash tables and run
standard block compression.

## Long distance matching (LDM)

`crates/encode/src/ldm.rs`. Two-pass integration with Fast/DFast strategies.

### Gear hash sampling

The gear hash is a 1-byte-at-a-time rolling hash:
`rolling = (rolling << 1) + GEAR_TABLE[byte]`, where `GEAR_TABLE` is 256
random `u64` values. After each byte, `rolling & stop_mask` is checked. When
it equals zero, the position is a "split point" where LDM samples a candidate.
`stop_mask` is derived from `hash_rate_log`: it has `hash_rate_log` bits set in
the upper part of the accumulator, so roughly 1 in `2^hash_rate_log` positions
trigger a check. At the default `hash_rate_log=6`, about 1 in 64 positions are
sampled.

The `feed()` method processes input in 4-byte unrolled batches (`LDM_BATCH_SIZE`
= 64 split points per call) for cache efficiency.

### Hash table

The table stores `LdmEntry` values: `offset: u32` (position in the input) +
`checksum: u32` (upper 32 bits of xxh64 over `min_match_length` bytes at that
position). At `hash_log=20`, this is 1M entries = 8 MiB. The table does not
store input data, so a fixed-size table covers arbitrarily large inputs.

Entries are organized into buckets of `1 << bucket_size_log` slots (default 8).
The lower bits of the xxh64 select a bucket, the upper 32 bits become the
checksum. Each bucket has a `u8` write counter in `bucket_offsets[]` that wraps
with `(counter + 1) & bucket_mask`. This is a per-bucket ring buffer: new
entries overwrite whichever slot the counter points at, round-robin. No age
tracking, no LRU.

When a split fires, if an existing entry in the selected bucket has a matching
checksum, the actual bytes at both positions are compared to confirm a match.
Backward and forward extension finds the full match length.

### Two-pass compression

First pass: `generate_sequences()` scans the input with the gear hash, probes
the hash table at each split point, and collects `RawLdmSeq` entries (literal
length, match length, offset). Second pass: `compress_block()` iterates over
the LDM sequences. For each gap between LDM matches, the regular Fast or DFast
compressor runs on that segment and emits short-range sequences. LDM sequences
and gap-fill sequences are interleaved into the final sequence list.

On the first block, `fill_only()` populates the hash table without searching
(there is no prior context to match against).

### Parameters

`LdmParams` controls: `hash_log` (table size), `bucket_size_log` (slots per
bucket), `min_match_length` (minimum bytes for a valid LDM match), and
`hash_rate_log` (sampling density). `LdmParams::default_for_window_log(wl)`
computes defaults for a given window size.

### Streaming and cross-block

In streaming mode, `LdmState` persists across blocks: the hash table retains
entries from previous blocks. `reduce_positions(shift)` subtracts `shift` from
all stored offsets when the window buffer compacts, discarding entries that
would point before the new buffer start.

### Memory and decoder impact

The encoder allocates the 8 MiB LDM hash table plus a window buffer of
`1 << window_log` bytes. The decoder does not know LDM was used. LDM sequences
are standard zstd sequences with larger offsets. The decoder allocates a window
buffer matching the `window_size` declared in the frame header, which is set by
`window_log`. At `window_log=27` this is ~128 MiB on each side.

Enable via `Options::ldm(true)` (requires the `ldm` feature, on by default).

## Scope

Greedy, Lazy, BtLazy2, BtOpt, and BtUltra strategies are out of scope.
zrip targets the Fast/DFast region of the speed-ratio curve (levels -7 through 4)
where encode throughput matters more than compression ratio.
