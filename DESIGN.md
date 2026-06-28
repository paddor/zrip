# Design

Architecture, performance techniques, and divergences from C zstd. Each
section explains what zrip does, how it differs, and why.


## Encode pipeline

### Fast strategy (`crates/encode/src/fast.rs`)

Port of C zstd's `ZSTD_compressBlock_fast_noDict_generic` 4-cursor match finder.
The pipeline probes positions ip0, ip1=ip0+1, ip2=ip0+step, ip3=ip2+1 per
iteration, reusing hash computations across shifts. Amortizes loop overhead
and branch mispredictions vs a single-position-per-iteration approach. Rep
offset is checked at the step-ahead position (ip2) only.

Rep2 post-match chaining (`rep2_match_loop`): after any match, immediately
checks `rep_offset2` for consecutive rep matches (zero-literal-length
sequences). Chains matches without re-entering the main loop. 1-byte
backward extension on rep matches (C zstd style), multi-byte backward
extension on regular matches.

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

Based on C zstd's `ZSTD_defaultCParameters` table, with L-8 added by zrip:

| Level | Strategy | Hash table | Window | Min match | Target length | Notes |
|------:|:---------|:-----------|:-------|:---------:|:-------------:|:------|
| -8 | Fast | 32 KB | 512 KB | 5 | 7 | `force_raw_literals` (zrip-only) |
| -7 | Fast | 32 KB | 512 KB | 5 | 7 | |
| -6 | Fast | 32 KB | 512 KB | 5 | 6 | |
| -5 | Fast | 32 KB | 512 KB | 5 | 5 | |
| -4 | Fast | 32 KB | 512 KB | 5 | 4 | |
| -3 | Fast | 32 KB | 512 KB | 5 | 3 | |
| -2 | Fast | 32 KB | 512 KB | 5 | 2 | |
| -1 | Fast | 32 KB | 512 KB | 5 | 1 | |
| 1 | Fast | 64 KB | 512 KB | 4 | 1 | |
| 2 | Fast | 512 KB | 1 MB | 4 | 1 | |
| 3 | DFast | 2x 1 MB | 2 MB | 4 | 1 | Dual hash |
| 4 | DFast | 2x 2 MB | 8 MB | 4 | 1 | |

Negative levels trade ratio for throughput by increasing `target_length`
(skip acceleration). Higher `target_length` means more positions are skipped
on consecutive match-finding misses.

**L-8** skips Huffman table construction and always emits raw literal blocks
with predefined FSE tables. This eliminates the most expensive part of the
encode pipeline (Huffman tree build, stream encoding, custom FSE table
estimation) at the cost of compression ratio. The result is a valid zstd
frame that any decoder handles, but with LZ4-class encode throughput.

**L-7 through L2** use the full encode pipeline: Huffman-compressed literals
(with treeless reuse across blocks) and predefined or custom FSE tables for
sequences, whichever produces smaller output.

**L3 and L4** use the DFast strategy with two hash tables (short 4-byte and
long 8-byte matches) for better match quality at lower throughput.

**Two-tier raw literals decision.** Avoids the expensive
build-Huffman-table-encode-discard cycle for blocks where Huffman overhead
exceeds savings:

1. Per-level size ramp (`strategy.rs:apply_raw_literals_size_override`):
   inputs below a level-dependent threshold always use raw literals.
   L-7: 16 KB, L-6: 8 KB, L-5: 4 KB, L-4: 2 KB, L-3: 1 KB,
   L-2 and above: 0 (rely on entropy pre-check). L-8 always uses raw
   regardless of size.

2. Entropy pre-check (`block_encoder.rs:huf_worth_trying`): for blocks
   <= 32 KB that passed the size ramp, estimates compressed size from a
   byte histogram using integer-only fixed-point log2. Skips Huffman if
   estimated savings (including tree description overhead) are below
   ~3%. Also bails immediately if `max_sym > 128` (high byte range means
   expensive tree description for marginal gain). Blocks > 32 KB always
   try Huffman since tree overhead is negligible at that scale.


## Block encoder (`crates/encode/src/block_encoder.rs`)

Per-block decisions:

**Literals**: Level -7 always emits raw literal blocks (skips Huffman tree
construction entirely). All other levels try Huffman-compressed literals with
treeless reuse across blocks via `prev_huffman`. For literal blocks >= 1024
bytes, encodes into 4 independent streams to enable decoder-side parallelism.

**3-way FSE table mode selection.** Compares predefined, custom
(data-derived), and repeat (carry-over from previous block or dict) FSE
encodings. Picks the smallest. Estimates predefined cost without full
encoding to skip the custom path when it cannot win. Blocks with < 64
sequences always use predefined tables (custom table header overhead exceeds
savings).

**Pre-packed sequence batch** (`pack_sequences_and_literals`). Pre-computes
all LL/ML/OF codes, extra bits, and rep-offset resolution into `PackedSeq`
structs in a single pass. FSE encoding then iterates the packed array without
recomputing.

**SymbolTT encoding scheme.** Two-field per-symbol struct (`delta_nb_bits`,
`delta_find_state`) encodes variable-width emission and state transition in
one lookup. ~1.3 KB total footprint vs ~20 KB for full tables.

**Incompressibility fast path**: `block_looks_incompressible()` samples the
first 1024 bytes. Skips compression if >= 200 distinct byte values and max
frequency < 1/24. Applied before entering the match finder.

**Fallback**: If the compressed block >= source size, rep offsets are reverted
and a raw block is emitted.

**Predefined encode table caching.** `LazyLock` builds LL/ML/OF predefined
encode tables once per process.


## Dictionary compression

`train_dict_fastcover()` in `crates/core/src/dict/mod.rs`: FastCOVER segment
selection + `finalize_dictionary()` produces standard zstd dictionary format
(magic + dict_id + Huffman table + FSE tables + rep offsets + content).
`FastCoverParams` defaults: k=2048, d=8, accel=1. Full COVER also exists in
`core/src/dict/cover.rs`.

### 3-tier dict setup (`context.rs:compress_with_prepared`)

Adapts setup cost to input size:

| Tier | Input size | Hash setup | Match finder | Literals |
|------|-----------|-----------|-------------|----------|
| A (micro) | <= per-level threshold | zero small input table | `compress_fast_attached` (linear scan, dual-lookup) | raw |
| B (small) | threshold-16 KB | zero moderate input table | `compress_fast_attached` (linear scan, dual-lookup) | Huffman from dict |
| C (large) | > 16 KB | copy full snapshot | `compress_fast_block` (4-cursor pipeline) | Huffman from dict |

Thresholds: `ATTACH_THRESHOLD` (16 KB), per-level raw literals ramp (see
level parameters above). DFast strategy (levels 3-4) always uses the copy
path.

### Dict attachment (`fast.rs:compress_fast_attached`)

For tiers A and B, the PreparedDict's hash snapshot is referenced as a frozen
read-only table. A separate zeroed input hash table accumulates entries from
the current input. Two hash lookups per position (input table first, dict
table fallback). Zero-copy: no `memcpy` of the dict hash per frame.

Why a separate loop: the 4-cursor pipeline has ~15 live values for x86_64's
15 GPRs, with 5 stack spills per iteration. Adding dict attachment state
(dict hash pointer, second hash value, dict match candidate) would push to
~17+ live values and worsen spills. The simpler linear-scan loop has fewer
live variables and tolerates the extra state. For small inputs, the pipeline
startup cost dominates anyway.

### Hash log clamping (`strategy.rs:level_params_for_size`)

Clamps `hash_log`, `chain_log`, and `window_log` to `src_log` for small
inputs. The input-side hash table in the attached path uses this clamped size
while the dict-side table keeps the level's full `hash_log`. Reduces table
init cost without losing dict coverage.

### Hash table prefill (`fast.rs:prefill_hash_table`)

Fills the hash table from the dict prefix with strided sampling (step =
prefix_len / hash_size), then densely hashes the last 64 bytes. Balances
coverage against initialization cost.

### Dict entropy table carryover (`context.rs:PreparedDict::new`)

Converts the dictionary's decode FSE/Huffman tables to encode tables at
construction time. The first block uses dict tables as FSE "repeat" mode.

### Encoder prefix mode

Concatenates dict content + source, pre-fills hash table from dict content,
then runs the normal match finder over the combined buffer.


## Streaming

`crates/encode/src/streaming.rs`: `FrameEncoder<W: Write>` buffers input up to
128 KiB (`MAX_BLOCK_SIZE`), then flushes as a compressed block. Frame header
includes magic, descriptor (with content checksum flag), window descriptor, and
optional dict ID. `finish()` flushes the final block with `last=true` and writes
the XXH64 content checksum (lower 32 bits).

Cross-block matching: the encoder maintains a persistent window buffer
(`window_buf`) containing previous blocks' output. Hash table positions reference
into this combined view, allowing matches across block boundaries.

**Hash table position reduction** (`streaming.rs:reduce_hash_table`). When the
window buffer exceeds 2x window size, shifts all hash entries by `shift` and
compacts the window. Preserves hash state across blocks.

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


## Decode pipeline

### Sequence execution (`crates/decode/src/exec.rs`)

`#![forbid(unsafe_code)]`. A single safe Rust implementation handles all
platforms. FSE decode tables are fixed-size `[FseSeqDecodeEntry; 512]` arrays
indexed with `state & 511`, which LLVM proves is always in bounds (emits
`andl $511` with no bounds check).

2-sequence unrolled decode loop: decodes 2 sequences per iteration while the
bit buffer has >= 16 bytes remaining. Cuts loop overhead in half.

Pre-allocated output buffer: reserves `MAX_BLOCK_SIZE + 64` bytes upfront so
wild copies never trigger reallocation.

Literal copies and match copies use wild-copy functions in `fast_vec.rs`
(see [Wild copies](#wild-copies) below).

`fearless_simd::dispatch!` compiles the function with platform target features,
enabling the compiler to generate BMI2 bit extraction instructions (`shlx`,
`shrx`) and auto-vectorize bitstream operations.


### FSE tables

**Fixed-size tables with mask** (`crates/core/src/fse/mod.rs`).
`[FseSeqDecodeEntry; 512]` arrays with `& FSE_SEQ_TABLE_MASK` instead of
variable-size Vec with bounds checks. Eliminates heap allocation per block
and enables bitwise masking instead of modular arithmetic.

**Promoted FSE tables** (`fse/mod.rs:promote_ll_table` etc.).
`FseSeqDecodeEntry` pre-computes `extra_bits` and `baseline_value` from the
LL/ML/OF code tables. Decode reads one struct instead of doing a separate
baseline/bits table lookup per symbol.

**In-place table building** (`sequences.rs:parse_sequence_tables_ws`). Builds
decode tables into workspace buffers instead of returning new Vecs.

**Predefined table caching** (`sequences.rs`). `LazyLock` for LL/ML/OF
predefined tables. Built once, array-copied per use.


### Bitstream (`crates/core/src/bitstream/reader_reverse.rs`)

**bitsConsumed model.** Tracks consumed bits instead of available bits.
Peek uses `(container << consumed) >> (64 - n)` (2 ops) vs the 3-op
available-bits model.

**Branchless bit reads** (`read_bits_branchless`).
`(container << (consumed & 63)) >> 1 >> (63 - n)` avoids the n==0 branch
in the hot FSE decode loop.

**Fast refill** (`refill_fast`). Byte-aligned reload without bounds checking
when the pointer is far from buffer start.

**Branch hints** (`hint.rs:likely`, `unlikely`). `#[cold]` function call to
hint branch prediction. Used for error paths and loop exits.


### Huffman 4-stream decoder (`crates/core/src/huffman/decode_4stream.rs`)

Interleaved 4-stream parallel decode: the compressed literals are split into 4
segments, each with an independent `ReverseBitReader`. The fast loop decodes 5
symbols per stream per iteration (20 symbols total), checking both output bounds
and bitstream bounds. Index-based output with safe slice indexing and inline
`refill!`/`decode_one!` macros.

4-symbol unrolled 1-stream inner loop: decodes 4 symbols per iteration when
bits remaining >= 4 * table_log.

BMI2 variant via `#[target_feature(enable = "bmi2")]` for faster bit extraction
(PEXT/PDEP). Dispatched via `cpu_tier() >= CpuTier::Bmi2`.
Slow path finishes remaining symbols one stream at a time near exhaustion.

Treeless block reuse: `huf_valid` flag allows treeless literal blocks to reuse
the previous Huffman decode table with no re-parse or rebuild.

Literal buffer padding: appends 32 zero bytes past literal data. Enables
unconditional SIMD loads in the sequence decode loop without bounds checks.


### Wild copies (`crates/decode/src/fast_vec.rs`)

All unsafe gated by `#[cfg(not(feature = "paranoid"))]`.

- **16-byte chunk copy** (`copy_16`). Two unaligned u64 load/stores for
  literal and match copies.
- **fast_extend_from_slice**. Tiered: >= 16 bytes uses 16-byte chunks,
  8-15 uses two 8-byte copies, 4-7 uses two 4-byte copies, < 4 uses
  `copy_nonoverlapping`. All paths use `set_len` to skip initialization.
- **wild_copy_match with offset dispatch**. Five paths by offset: >= 16
  (non-overlapping 16-byte copies), == 1 (RLE via `write_bytes`), 8-15
  (overlapping 8-byte copies), 2-7 (pattern stamping via
  `build_pattern_u64`).
- **build_pattern_u64**. Builds an 8-byte repeating pattern from `offset`
  source bytes without reading uninitialized memory.

Under `paranoid`, these fall back to `extend_from_slice` and
`extend_from_within`.


## CPU feature dispatch

Two dispatch mechanisms coexist:

**Decoder:** `fearless_simd::dispatch!` with a cached `OnceLock<Level>`. The
macro compiles `decode_execute_sequences` with platform-specific target features
(AVX2+BMI2 on x86_64, NEON on aarch64, SIMD128 on wasm32) and selects the best
version at runtime. Zero `unsafe` in user code. Works under `paranoid` since
`fearless_simd` requires no unsafe. Without `std` or `simd`, the scalar path
runs directly.

**Huffman BMI2:** `crates/core/src/simd/mod.rs` defines a `CpuTier` enum
(`Bmi2`/`Avx2` on x86_64). Runtime detection cached in `OnceLock<CpuTier>`.
The Huffman 4-stream decoder dispatches to a `#[target_feature(enable = "bmi2")]`
variant for faster bit extraction.

**Compile-time tier for no_std.** Falls back to `cfg!(target_feature = "...")`
checks when std is unavailable.


## Compile-time specialization

Const generics eliminate dead code at compile time:

- `compress_fast_block_impl::<HASH_LOG, MLS>`: LLVM specializes per MLS (4, 5, 7),
  dead-code eliminates unused hash/match paths.
- `compress_dfast_block_impl::<HASH_LOG, SHORT_LOG, MLS>`: same for DFast.
- `hash_pos::<HASH_LOG, MLS>`: selects hash4/hash5/hash7 at compile time.
- Strategy dispatch (`Fast` vs `DFast`) happens at block boundaries, not
  per-position.


## Allocation reuse

**CompressContext** (`crates/encode/src/context.rs`). Hash tables, output
buffer, sequence vec, and workspace are allocated once and reused across calls.

**BlockEncodeWorkspace** (`crates/encode/src/block_encoder.rs`). All scratch
buffers (lit_buf, lit_section, pred_seq, cust_seq, repeat_seq, huf_concat,
huf_stream, writer bufs, packed_seqs) allocated once, reused across blocks.

**Cow output borrowing** (`context.rs:take_or_borrow_output`). Small outputs
return `Cow::Borrowed` (zero allocation), large outputs return `Cow::Owned`
(moves the buffer out).

**DecompressContext** (`crates/decode/src/context.rs`). Output buffer and
`BlockDecodeWorkspace` allocated once, reused across frames.

**BlockDecodeWorkspace** (`crates/decode/src/lib.rs`). All scratch vectors
(literal_buf, huf_table, huf_all_weights, huf_rank_count, huf_rank_start,
fse_dist, fse_symbol_next, fse_build_buf) allocated once.

**Dict FSE/Huffman caching** (`crates/decode/src/lib.rs:cache_dict`).
Pre-promotes dict FSE decode tables (LL/ML/OF) and Huffman table at
construction time. Each frame clones the cached promoted tables instead of
rebuilding from raw dictionary entries. Avoids 3 intermediate Vec allocations
per frame.


## Unsafe boundary

The decoder implementation is `#![forbid(unsafe_code)]` with zero `unsafe`
blocks. Platform dispatch is handled by `fearless_simd::dispatch!`, which
encapsulates `#[target_feature]` calling internally.

Encoder unsafe is confined to `encode/src/primitives.rs` (16 blocks):

- **Unchecked hash table access** (`hash_load`, `hash_store`).
  `get_unchecked`/`get_unchecked_mut` eliminates bounds checks in the
  hot loop.
- **Unaligned 4/8-byte reads** (`rd32`, `rd64`). `read_unaligned` for
  hash input, match confirmation, and match counting.
- **Pointer-based match counting** (`count_match_raw`). Compares 8 bytes
  at a time via XOR + `trailing_zeros()`. Falls to a byte loop for the
  tail.
- **16-byte literal copy** (`copy_literals_fast`). For len <= 16: two
  unconditional 8-byte copies (overlapping ok). For len > 16:
  `copy_nonoverlapping`.
- **Unchecked bitstream flush** (`bitstream_flush`). Direct
  `write_unaligned` into Vec at known-valid position.
- **Unchecked Vec write-at** (`vec_write_at`). Writes into uninitialized
  Vec capacity without updating len.
- **Cold panic path** (`cold_rep_panic`). `#[cold] #[inline(never)]`
  keeps panic code out of the hot path.

Each function has a `debug_assert!` guard. Pattern:
```rust
debug_assert!(pos + 4 <= src.len());
unsafe { (src.as_ptr().add(pos) as *const u32).read_unaligned() }
```

Core crate unsafe follows the same pattern in `bitstream/`, `huffman/`,
`xxhash/` primitives modules.

`assert_rep_valid` uses both `debug_assert!` and `core::hint::assert_unchecked`
to help the optimizer prove bounds.


## Core bitstream primitives

**Unaligned u64 read/write** (`bitstream/primitives.rs`). `read_unaligned`/
`write_unaligned` for 8-byte operations in all bit readers and writers.

**BitWriter batch flush** (`bitstream/writer.rs`). Accumulates bits in a u64
register, flushes when >= 32 bits. Single unaligned write per flush.


## `paranoid` feature

`cargo build --features paranoid` adds `#![forbid(unsafe_code)]` to all four
crate roots. `fearless_simd` dispatch still works (it requires no unsafe), so
the decoder gets full SIMD multiversioning under paranoid. What changes:

- Encoder and core `primitives.rs` switch to safe alternatives via dual
  `#[cfg(feature = "paranoid")]` / `#[cfg(not(feature = "paranoid"))]` bodies:
  direct indexing, `from_le_bytes`, `resize` in place of unchecked operations.
- Decoder wild-copy (`fast_vec.rs`) falls back to `extend_from_slice` and
  `extend_from_within`.
- Huffman BMI2 `#[target_feature]` dispatch is gated out (uses `unsafe` call).


## Cross-cutting

**Nightly optimize attributes.** `#![cfg_attr(feature = "nightly",
feature(optimize_attribute))]` on all three crates. Enables per-function
`#[optimize]` control on nightly builds.

**no_std + alloc.** All core codec logic works without std. Predefined table
caching degrades gracefully (rebuilds per use in no_std vs cached via
`LazyLock` in std).


## Scope

Greedy, Lazy, BtLazy2, BtOpt, and BtUltra strategies are out of scope.
zrip targets the Fast/DFast region of the speed-ratio curve (levels -7 through 4)
where encode throughput matters more than compression ratio.
