# Changelog

## [Unreleased]

- Make `DecompressContext` available with the `alloc` feature. It no longer
  requires `std`.

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
