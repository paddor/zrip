# Changelog

## [Unreleased]

- Add the `simd` feature (default): runtime AVX2/BMI2 dispatch through
  `fearless_simd` 1.0.
- Choose Huffman table reuse by estimated size.
- Word-wise backward match extension in the fast and dfast match finders.
- Fixed-width literal copies and a safe sequence bit writer.
- Retune level parameters (6-byte hash for L-8 to L2). The dfast probe is
  removed.
- L-8 is L-7 with target length 21 and search strength 5. It uses Huffman
  literals and custom sequence tables.
- The literal entropy check runs after the cheap raw-literal exit and reuses
  the literal histogram for up to 4 KiB of literals.
- Encode literal-only blocks when a block has no usable matches.
- Skip Huffman literals above a per-level entropy limit on the fast levels.
- Drop the Huffman table after a raw-block fallback.
- Remove `strategy::apply_raw_literals_size_override`. `level_params_for_size`
  sets `force_raw_literals` for negative-level inputs up to a per-level size,
  2 KiB at L-1 to 24 KiB at L-8. The negative levels use a denser match
  search on them. These inputs skip the incompressibility sampling.
- Remove the few-matches bail-out from the fast match finder.

## [0.8.7] - 2026-09-10

- Add `CompressContext::set_content_checksum` and `content_checksum` for
  reusable contexts. Checksums remain enabled by default.
- Keep `FrameEncoder` failed after write, flush, or finalization errors,
  including partial writes and `WouldBlock`.
- Update `zrip-core` to `0.10.1`.
