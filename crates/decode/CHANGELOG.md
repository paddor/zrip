# Changelog

## [Unreleased]

## [0.8.7] - 2026-09-10

- Correct `FrameDecoder` skippable-frame lengths and reject truncated payloads.
- Reject incomplete streaming frame headers, including after a complete frame.
- Discard failed output and require `FrameDecoder::reset` after decoding or
  reader errors.
- Update `zrip-core` to `0.10.1`.
