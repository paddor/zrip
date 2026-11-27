# Safety

## No unsafe

All four crates are `#![forbid(unsafe_code)]` in every build. SIMD dispatch
goes through [`fearless_simd`](https://crates.io/crates/fearless_simd), which
keeps the `#[target_feature]` calls inside that crate. It needs `unsafe` for
them (113 lines in 1.3.0), so a build with `simd`, a default feature, runs
`unsafe` code from that crate.

| Build | `unsafe` that runs | `unsafe` in the dependency tree |
|-------|--------------------|---------------------------------|
| default (`simd`) | `fearless_simd`'s | `fearless_simd` |
| `simd` and `paranoid` | none | `fearless_simd`, never called |
| `default-features = false`, without `simd` | none | none |

Cargo features only add, so `paranoid` cannot drop `fearless_simd` when any
crate in the build enables `simd`. It makes zrip take its scalar paths
instead. Only a build without `simd` leaves `fearless_simd` out.

The scalar paths cost speed in portable builds, which have no
`-C target-cpu`: on lsm-tree's 4 and 16 KiB data blocks, reads took 19-36%
longer and writes up to 7% longer (i7-8700B). `paranoid` with `simd` runs
as fast as a build without `simd`.

Hot loops avoid bounds checks by construction: exact-size hash tables, reads
clamped to limits the compiler can prove, and zeroed decoder output room. A
broken match finder invariant reads wrong input bytes instead of panicking;
debug builds assert the invariants.

If a later release adds `unsafe` to zrip for speed, builds with `paranoid`
leave it out too.

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
| [CVE-2026-124851](https://www.cve.org/CVERecord?id=CVE-2026-124851) (1.5.1 to 1.5.7, fixed in 1.6.0 by [PR #5023](https://github.com/meta/zstd/pull/5023)) | Crafted input to streaming decompression exposes a 64 KiB internal buffer in the output: uninitialized memory, or literals of earlier decompressions with the same context | Safe Rust cannot read uninitialized memory. A block outputs only the bytes it wrote: literal copies are bounded by the section's literal count, and matches by the frame's own history, so reused buffers never reach the output |
| [PR #4731](https://github.com/meta/zstd/pull/4731) (fixed in 1.6.0) | OOB read and write in the DDict hash table with `ZSTD_d_refMultipleDDicts` | Slice indexing panics on OOB |
| [PR #4848](https://github.com/meta/zstd/pull/4848) (fixed in 1.6.0) | Use-after-free of a `ZSTD_DCtx_refPrefix()` prefix after `ZSTD_DCtx_reset()`, following a malformed frame header | A borrowed prefix cannot outlive its owner; the borrow checker rejects the reuse |
