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

## SIMD dispatch

`crates/core/src/simd/mod.rs` defines a `CpuTier` enum:

| Tier | x86_64 | aarch64 |
|------|--------|---------|
| Scalar | Always | Always |
| Sse2 | `is_x86_feature_detected!` | n/a |
| Bmi2 | `is_x86_feature_detected!` | n/a |
| Avx2 | `is_x86_feature_detected!` | n/a |
| Neon | n/a | `is_aarch64_feature_detected!` |

With `std`: runtime detection cached in `OnceLock<CpuTier>`. Without `std`:
compile-time tier from `cfg!(target_feature)`.

**Dispatch happens at block boundaries, not per-operation.** The decoder checks
`cpu_tier()` once per block and calls either the fused SIMD decoder or the scalar
fallback. The encoder uses SIMD only for wildcopy in output and hash table
prefetch, not in the match-finding loop.

## Fused SIMD sequence decoder

`crates/decode/src/simd_decode/x86_64/decode.rs` and `aarch64/decode.rs`.

A single function fuses FSE state machine + sequence execution + literal/match
copy. The inline bitstream reader uses macros (`read_bits!`, `refill_fast!`)
to avoid function call overhead. AVX2 provides 32-byte wildcopy
(`_mm256_loadu_si256`/`_mm256_storeu_si256`) for both literal and match copy.
NEON provides 16-byte equivalents.

Const-generic `HAS_HISTORY: bool` eliminates dictionary history-check branches
when no dictionary is present.

Scalar fallback lives in `crates/decode/src/exec.rs`: index-based sequence
execution using `Vec<u8>` operations (`extend_from_slice`, `copy_within`, byte
loop for overlapping matches). This is the sole scalar implementation for both
default and `paranoid` builds. On hardware with AVX2 or NEON, the SIMD decoder
handles all blocks and `exec.rs` is never reached.

## Huffman 4-stream decoder

`crates/core/src/huffman/decode_4stream.rs`.

Interleaved 4-stream parallel decode: the compressed literals are split into 4
segments, each with an independent `ReverseBitReader`. The fast loop decodes 5
symbols per stream per iteration (20 symbols total), checking both output bounds
and bitstream bounds. Raw pointer output and inline `refill!`/`decode_one!`
macros.

BMI2 variant via `#[target_feature(enable = "bmi2")]` for faster bit extraction.
Slow path finishes remaining symbols one stream at a time near exhaustion.

## `paranoid` feature

`cargo build --features paranoid` adds `#![forbid(unsafe_code)]` to all four
crate roots. Each `primitives.rs` module has dual `#[cfg(feature = "paranoid")]`
/ `#[cfg(not(feature = "paranoid"))]` bodies: the paranoid path uses direct
indexing, `from_le_bytes`, `resize`, and `extend_from_slice` in place of
unchecked operations. SIMD modules (`core/simd/x86_64/`, `core/simd/aarch64/`,
`decode/simd_decode/`) are gated out entirely. `cpu_tier()` returns
`CpuTier::Scalar`, routing all blocks through `exec.rs`. The Huffman 4-stream
interleaved decoder is replaced by sequential per-stream decode via
`decode_stream_tail`.

## Unsafe boundary

All compression and decompression logic is `#[forbid(unsafe_code)]` (30 modules).
Unsafe is confined to 14 leaf modules in three categories:

1. **`primitives.rs`** (one per crate): `#[inline(always)]` wrappers around
   `get_unchecked`, `read_unaligned`, `set_len`, `copy_nonoverlapping`. Each
   function has a `debug_assert!` guard. Pattern:
   ```rust
   debug_assert!(pos + 4 <= src.len());
   unsafe { (src.as_ptr().add(pos) as *const u32).read_unaligned() }
   ```

2. **`simd/`** submodules: intrinsics and raw pointer arithmetic for wildcopy,
   copy-match, and common-prefix-length.

3. **`simd_decode/`**: the fused SIMD sequence decoders.

`assert_rep_valid` uses both `debug_assert!` and `core::hint::assert_unchecked`
to help the optimizer prove bounds.

## Compile-time specialization

Const generics eliminate dead code at compile time:

- `compress_fast_block_impl::<HASH_LOG, MLS>`: LLVM specializes per MLS (4, 5, 7),
  dead-code eliminates unused hash/match paths.
- `compress_dfast_block_impl::<HASH_LOG, SHORT_LOG, MLS>`: same for DFast.
- `hash_pos::<HASH_LOG, MLS>`: selects hash4/hash5/hash7 at compile time.
- `HAS_HISTORY: bool` in SIMD decoders: eliminates dictionary checks.
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

`reset()` finishes the current frame, swaps the writer, and reuses all internal
buffers (hash tables, workspace, block encoder).

First block with a dictionary uses `compress_fast_with_prefix_reuse` /
`compress_dfast_with_prefix_reuse`. Subsequent blocks clear hash tables and run
standard block compression.

## Scope

Greedy, Lazy, BtLazy2, BtOpt, and BtUltra strategies are out of scope.
zrip targets the Fast/DFast region of the speed-ratio curve (levels -7 through 4)
where encode throughput matters more than compression ratio.
