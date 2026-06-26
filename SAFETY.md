# Safety

## Unsafe boundary

The decoder has zero `unsafe` blocks. All files in `crates/decode/src/` are
`#![forbid(unsafe_code)]`. Platform dispatch (AVX2+BMI2 on x86_64, NEON on
aarch64) is handled by `fearless_simd::dispatch!`, which encapsulates
`#[target_feature]` calling internally. No `unsafe` is exposed in the public API.

Encoder unsafe is confined to `encode/src/primitives.rs` (16 blocks):
`get_unchecked`, `read_unaligned`, `set_len`, `count_match_raw`, `prefetch`.
Callers prove bounds at block level. Every block has a `debug_assert!` guard.

Core crate unsafe is confined to `primitives.rs` in `bitstream/`, `huffman/`,
`xxhash/`: `#[inline(always)]` wrappers around `get_unchecked`, `read_unaligned`,
`set_len`. Same pattern as encoder.

## `paranoid` feature

`cargo build --features paranoid` compiles zrip with `#![forbid(unsafe_code)]`
on all four crates. The decoder is already safe by default; `paranoid` affects
the encoder and core primitives:

| Category | Default | Paranoid |
|:---------|:--------|:---------|
| Encoder indexing | `get_unchecked`, `read_unaligned` | Direct indexing, `from_le_bytes` |
| Encoder Vec length | `set_len` | `resize` |
| Decoder SIMD dispatch | `fearless_simd::dispatch!` | Same (no unsafe needed) |
| Decoder wild-copy | 16-byte unaligned load/store | `extend_from_slice`, `extend_from_within` |
| Huffman BMI2 dispatch | `#[target_feature]` wrapper | Gated out; generic call |

The safe alternatives use the same algorithms and produce identical output.
Encode throughput drops roughly 40% (corpus dependent). Decode throughput
drops roughly 20% due to safe wild-copy fallbacks in `fast_vec.rs`.

The feature exists for users who need a guarantee of zero unsafe, or for
auditing and benchmarking the cost of safe-only codepaths.

## Why Rust matters here

C zstd's decompression path has had memory safety bugs that Rust's type system
and bounds checking prevent by construction:

| CVE / Fix | Bug | Rust prevents because |
|-----------|-----|----------------------|
| [#50](https://github.com/facebook/zstd/issues/50) | OOB heap read in `ZSTD_copy8` (via `ZSTD_wildcopy` during `ZSTD_execSequence`): 8-byte read past buffer on malformed input (AFL) | Wildcopy uses slice operations bounded by destination length |
| [#49](https://github.com/facebook/zstd/issues/49) | OOB stack read in `HUF_readStats`: `rankVal` array accessed past bounds on malformed Huffman table (AFL) | Stack arrays are slices; indexing panics on OOB |
| [#22](https://github.com/facebook/zstd/issues/22) | Destination buffer bounds not checked during compress/decompress, heap overflow on undersized output | Output written via `&mut [u8]` slice; writes past end panic |
| [#4499](https://github.com/facebook/zstd/issues/4499) | Stack buffer overflow read in `FASTCOVER_hashPtrToIndex` training with many 1-byte samples | Slice indexing panics on OOB |
| [OSV-2020-405](https://osv.dev/vulnerability/OSV-2020-405) (HIGH) | Stack buffer overflow write in `ZSTD_decodeLiteralsBlock` (OSS-Fuzz #16445) | Slice write panics on OOB |
| [OSV-2020-654](https://osv.dev/vulnerability/OSV-2020-654) (MEDIUM) | Heap buffer overflow read 16 in `ZSTD_copy16` during `ZSTD_decompressSequences_bmi2` (OSS-Fuzz #17451) | Wildcopy bounded by slice length |
| [OSV-2020-429](https://osv.dev/vulnerability/OSV-2020-429) (MEDIUM) | Heap buffer overflow read 4 in `MEM_read32` during legacy v05 sequence decoding (OSS-Fuzz #14368) | `u32::from_le_bytes` requires a bounds-checked `[u8; 4]` |
| [OSV-2021-859](https://osv.dev/vulnerability/OSV-2021-859) (HIGH) | Heap buffer overflow write 1 in `FSE_writeNCount_generic` during Huffman weight compression (OSS-Fuzz #35209) | Slice write panics on OOB |
| [OSV-2022-110](https://osv.dev/vulnerability/OSV-2022-110) (HIGH) | Heap buffer overflow write 1 in `ZSTD_compressLiterals` (OSS-Fuzz #44239) | Slice write panics on OOB |
| [OSV-2022-96](https://osv.dev/vulnerability/OSV-2022-96) (HIGH) | Heap buffer overflow write 16 in sequence compression API (OSS-Fuzz #44122) | Slice write panics on OOB |
| [CVE-2024-11477](https://www.crowdfense.com/cve-2024-11477-7zip-zstd-buffer-overflow/) (CVSS 7.8, 7-Zip's zstd) | Integer underflow in decompression wraps array index, heap buffer overflow write | `usize` underflow panics in debug; slice indexing catches OOB in release |
| [PR #1803](https://github.com/facebook/zstd/pull/1803) | Memory over-read from wildcopy changes (PR #1756), required increasing `WILDCOPY_OVERLENGTH` to 16 (OSS-Fuzz) | Wildcopy in Rust uses slice operations bounded by destination length |
| [PR #2784](https://github.com/facebook/zstd/pull/2784) | Multiple Huffman decompression bugs: pointer underflow and NULL pointer issues (fuzzer) | No null references in Rust; pointer arithmetic replaced by slice indexing |
| [PR #1722](https://github.com/facebook/zstd/pull/1722) | Buffer overflow in legacy v0.3 decompression | Slice indexing panics on OOB |
| [PR #1590](https://github.com/facebook/zstd/pull/1590) | OOB read in `ZSTD_decompressBound` on malformed frame (fuzzer) | Slice indexing panics on OOB |
