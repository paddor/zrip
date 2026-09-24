# Changelog

## [Unreleased]

- Upgrade `fearless_simd` to 1.0.
- Copy literal runs with a fixed 16-byte copy when enough source remains,
  including a safe variant for `paranoid` builds.
- Fill custom LL/ML/OF sequence tables in place instead of building and
  moving a new table per block.

## [0.8.7] - 2026-09-10

- Correct `FrameDecoder` skippable-frame lengths and reject truncated payloads.
- Reject incomplete streaming frame headers, including after a complete frame.
- Discard failed output and require `FrameDecoder::reset` after decoding or
  reader errors.
- Update `zrip-core` to `0.10.1`.
