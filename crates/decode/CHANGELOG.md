# Changelog

## [Unreleased]

## [0.8.18] - 2026-11-21

- `decode_literals_ws` and `parse_sequence_tables_ws` dispatch to
  x86-64-v3 in builds without AVX2, as the sequence executor does, so they
  run with BMI2 and LZCNT.
- Sequence FSE tables are built in one pass with
  `zrip_core::fse::table_builder::build_decode_rows`.

## [0.8.17] - 2026-11-20

- Removed the unused `BlockDecodeWorkspace::fse_symbol_next`.

- Decoding matches C zstd 1.5.7: one-shot decoding holds only compressed
  blocks to `Block_Maximum_Size`, and their output may pass it by 32 bytes,
  or by any amount when the literals stay outside the output, as there.
  Literals over the block maximum fail. Streaming decoders reject windows
  over 128 MiB also for single-segment frames, hold block output to the
  block maximum, read a compressed block of size 0 as empty, skip the
  content size check after an empty last block, and no longer fail a
  content size over the output limit up front.
- A block's sequences that pass its room fail with `BlockTooLarge` instead
  of `CorruptSequences`.
- `skip_skippable_frame` no longer overflows on 32-bit targets for sizes
  near `u32::MAX`.

## [0.8.16] - 2026-11-20

- `PushDecoder::decode_uninit` writes into an uninitialized buffer.
- `PushDecoder::at_frame_end` tells whether a frame ended and no later
  input was read.
- A compressed block without a sequence count, or with bytes after a zero
  count, fails with `CorruptSequences`, and a block over
  `Block_Maximum_Size` (the window, at most 128 KiB, compressed size
  included) with `BlockTooLarge`, as in C zstd.
- `FrameState` counts output in `u64`, so 32-bit targets decode past 4 GiB.

## [0.8.15] - 2026-11-19

- Decoders keep a dictionary's Huffman and sequence tables over frames that
  only repeat them, instead of copying them for every frame.

## [0.8.14] - 2026-11-18

- Add `push::PushDecoder`: push-style (sans-io) decoding into caller
  buffers. Reads multi-frame streams and skippable frames, takes a
  dictionary and an output limit, and needs only `alloc`.
- One-shot decoding accepts frames whose window exceeds 128 MiB. One-shot
  output is the history, so `max_output` bounds memory, not the window.
- `FrameDecoder` decodes each block straight into its window history,
  reserves a known content size up front, and builds the predefined
  sequence tables once per decoder. Silesia decodes 12-64% faster at
  levels 1 and 3.
- `FrameDecoder::with_limit` reports a crossed limit as `OutputTooSmall`
  instead of `CorruptSequences`, and a bad magic number fails as soon as it
  is read.
- Literal sections under 24 KiB decode with single-symbol Huffman tables,
  which are faster there.

## [0.8.13] - 2026-11-02

- `FrameDecoder` keeps up to two windows of history and drops the older one
  when full, instead of trimming it after every block. A stream with a
  128 MiB window decodes at 2.5 GB/s instead of 24 MB/s, and holds about
  260 MiB of history instead of 130 MiB.
- `FrameDecoder::with_dict` keeps the whole dictionary reachable until the
  frame has produced a full window, as RFC 8878 allows. It dropped the start
  of the dictionary early, so C zstd frames that copied from there failed
  with `InvalidOffset`.

## [0.8.12] - 2026-10-31

- `DecompressContext::with_dict` and `FrameDecoder::with_dict` load the
  predefined sequence table when a block selects Predefined_Mode. They kept
  decoding with the dictionary's table, so valid frames failed with
  `CorruptSequences` or returned wrong bytes. Only dictionaries with their
  own sequence tables were affected, such as those from `zstd --train`.

## [0.8.11] - 2026-10-19

- Enforce `#![forbid(unsafe_code)]` for every build. The `paranoid` feature
  remains available and has no effect today.
- Faster sequence and literal decoding with safe output copies and
  two-symbol Huffman tables for dense literal sections.
- Reject sequence bitstreams that run out of bits early with
  `CorruptSequences` instead of returning incorrect output.

## [0.8.10] - 2026-09-27

- Use a frame's declared content size as its output limit. Up to 128 MiB,
  the output is allocated once at that size plus 64 bytes instead of
  doubling near the end of the frame. A compressed block past the declared
  size fails with `CorruptSequences`, and a raw or RLE block with
  `FrameSizeMismatch`.

## [0.8.9] - 2026-09-26

- Make `DecompressContext` available with the `alloc` feature. It no longer
  requires `std`.
- Mark the live sequence tables as no longer matching the table cache
  whenever they are replaced: when a failed frame dropped them and when the
  streaming decoder starts or resets a frame. Stale tables had made a reused
  context accept malformed frames and reject valid ones.
- Invalidate the cached Huffman header and weights when the streaming
  decoder installs a dictionary's Huffman table, as the one-shot decoder
  already did.

## [0.8.8] - 2026-09-25

- Add caller-owned output methods to `DecompressContext`, retaining reusable
  decoder workspace without requiring decoded buffers to be copied.
- Upgrade `fearless_simd` to 1.0.
- Copy literal runs with a fixed 16-byte copy when enough source remains,
  including a safe variant for `paranoid` builds.
- Fill custom LL/ML/OF sequence tables in place instead of building and
  moving a new table per block.
- Copy only the used entries of sequence tables when caching them or
  restoring predefined and dictionary tables, and refresh the table cache
  in place instead of allocating a new one per block.
- Update `zrip-core` to `0.11.0`.

## [0.8.7] - 2026-09-10

- Correct `FrameDecoder` skippable-frame lengths and reject truncated payloads.
- Reject incomplete streaming frame headers, including after a complete frame.
- Discard failed output and require `FrameDecoder::reset` after decoding or
  reader errors.
- Update `zrip-core` to `0.10.1`.
