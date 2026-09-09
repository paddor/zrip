# Changelog

## [Unreleased]

## [0.8.7] - 2026-09-10

- Add `CompressContext::set_content_checksum` and `content_checksum` for
  reusable contexts. Checksums remain enabled by default.
- Keep `FrameEncoder` failed after write, flush, or finalization errors,
  including partial writes and `WouldBlock`.
- Update `zrip-core` to `0.10.1`.
