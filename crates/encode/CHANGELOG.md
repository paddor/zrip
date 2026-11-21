# Changelog

## [Unreleased]

## [0.10.1] - 2026-11-21

- Depends on zrip-core 0.14.

## [0.10.0] - 2026-11-20

- Removed the unused `strategy::max_hash_log`.

- `fast::count_back` counts equal bytes from the right end on big-endian
  targets; it counted from the wrong end and extended matches over unequal
  bytes.
- LDM buckets of 256 entries (`LdmParams::bucket_size_log` 8, C zstd's
  maximum) cycle through all entries; the mask was computed in `u8` and
  overflowed.

- Level -3 hashes eight bytes at level -4's step (`min_match` 8,
  `target_length` 2) in a table of 2^14 entries, so it sits between -4 and -2
  in speed and ratio. The Fast finder gets a constant-table path for it.
- Level -7 grows its step every 8 bytes without a match (`search_strength`
  3), so it encodes faster than -6 on text, too.
- Fixed: a level -7 block written with the predefined sequence tables now
  drops the dictionary's tables, so a later literal-heavy block cannot
  repeat them. Such frames failed to decode since 0.9.6.

## [0.9.10] - 2026-11-20

- `strategy::MIN_LEVEL` and `strategy::MAX_LEVEL` name the lowest and
  highest compression levels.
- `PushEncoder::encode_uninit` writes into an uninitialized buffer.
- Literal sections under 64 bytes never build a fresh Huffman table, as in C
  zstd: they reuse the previous one or stay raw. Raw literals and raw blocks
  keep the decoder's table; a fresh table takes over only once its block is
  written compressed.
- `Options::window_log(0)` selects the level's default window, as in C
  zstd, instead of `WINDOW_LOG_MIN`.

## [0.9.9] - 2026-11-20

- The DFast probe loop breaks out with the match it found and handles it
  after the loop, with no closures. Built without `target-cpu=native`,
  levels 3 and 4 compress 17-52% faster; frames are byte-identical.

## [0.9.8] - 2026-11-19

- Fix a panic or corrupt frame when a block repeated a dictionary's sequence
  tables while its offset codes went past what they hold. FSE encode tables
  record their symbols, and repeat mode needs every code of the block.
- Levels 3 and 4 keep a dictionary in its own tables beside small input
  tables, for inputs up to 8 KiB, or 16 KiB when the prefilled tables are
  sized for other inputs, as C zstd's dfast does. A 64 B frame with a 16 KiB
  dictionary at level 3 takes 2 µs instead of 43.
- Dictionary frames follow C zstd: literals up to 1 KiB reuse the
  dictionary's Huffman table, raw literals win when headers make Huffman
  larger, blocks under 1000 sequences reuse valid dictionary sequence tables
  unless they code them over 20% past their entropy, and frames within their
  window are single-segment. 64 B frames shrink by 11-13%.
- `CompressContext::with_dict` builds its snapshot and attached tables on
  first use, from the dictionary bytes alone.
- Match tables go through `MatchTable`, which zeroes each at most once per
  frame. A reused `PushEncoder` or `FrameEncoder` clears only the slots a
  small frame wrote.
- One-shot frames reserve sequences for their input, streams grow their
  window by doubling up to a block, and a reused `PushEncoder` encodes
  one-pass frames with its own tables.

## [0.9.7] - 2026-11-19

- Sequence bitstreams go straight into the block output, reserved from an
  exact bound, and the literal buffer is sized to the block's literals, so
  one-shot calls zero and copy less.
- `CompressContext::new` allocates nothing; the first frame sizes the hash
  tables and zeroes them once. A context made per call for 1-16 KiB inputs
  runs 1.2-3x faster.

## [0.9.6] - 2026-11-18

- Add `push::PushEncoder` with `EndOp`: push-style (sans-io) encoding into
  caller buffers. A frame that ends before any of it was written is encoded
  in one pass, the same bytes as `compress`. Needs only `alloc`.
- `FrameEncoder` appends input straight into its window, grows it in large
  steps, and allocates hash tables on the first block. Silesia encodes
  1-30% faster.
- Retune Fast levels -6 to 2 for speed. Levels -6 to -4 hash and confirm 8
  bytes, -3 to -1 hash 7, most negative levels skip ahead faster in literal
  runs, and levels 1 and 2 use smaller hash tables. On medium-compressibility
  Silesia files, levels -4 to 2 encode 11-17% faster for 2.5-3.4% less ratio.
- Level -7 Huffman-codes literals in literal-heavy blocks over 32 KiB below
  6.09 bits per byte, instead of storing them raw.
- Use four Huffman literal streams from 256 literals, as C zstd does.

## [0.9.5] - 2026-11-12

- The `context` module and `CompressContext` need only the `alloc` feature,
  not `std`.

## [0.9.4] - 2026-11-02

- `Options::ldm(true)` without `window_log` uses a 128 MiB window, like
  `zstd --long`, so streams find repeats past the level's window. One-shot
  calls shrink it to the input. `FrameEncoder` frames always advertise
  128 MiB, even short ones. Set `window_log` for a smaller window.
- `compress_opts` with LDM sizes its hash table for the window after shrinking
  that to the input. A 1 KiB input at `window_log` 24 allocated the full 8 MiB
  table.
- `compress_opts` with `ldm(true)` and no `window_log` no longer panics in
  debug builds on inputs of 2 to 16 bytes.
- One-shot inputs just above a multiple of 4 GiB keep their level's window and
  hash tables. The input size was cut to 32 bits, so 4 GiB + 5 bytes got
  `window_log` 3 and 8-byte blocks.
- One-shot compression keeps finding matches past 4 GiB. Hash table positions
  wrapped there, so the rest of the input lost nearly all matches. Frames
  stayed valid. `FrameEncoder` was not affected.

## [0.9.3] - 2026-10-31

- Retune L-3, L-2, L-1, and L1 to search less in long literal runs. This
  speeds up low-compressibility inputs with slightly lower compression
  ratios.
- L1 and L2 use 7-byte hashes and faster search-step growth. Above 256 KiB,
  L3 grows its search step after each unsuccessful probe; smaller inputs
  keep the denser search. Low-compressibility inputs encode faster at the
  cost of compression ratio.
- L4 uses faster search-step growth above 256 KiB for better throughput
  on low-compressibility inputs. Smaller inputs keep the denser search.
- The 256 KiB cutoff for L3 and L4 counts the input, not a dictionary, and
  applies to one-shot calls and `CompressContext`. `FrameEncoder` does not
  know its input size and always uses the sparser search, so short streams
  at those levels can come out up to about 16% larger than one-shot frames.
- LDM no longer emits offsets past the window a frame advertises. This hit
  streaming with a repeat 1 to 2 windows back, and one-shot frames past
  128 MiB. Those frames could fail in `FrameDecoder` and C zstd.
- Blocks no longer exceed the frame's window. `FrameEncoder` at `window_log`
  below 17, and one-shot frames past 128 MiB at such a `window_log`, cut
  128 KiB blocks that C zstd rejected.
- Custom `LevelParams` with `hash_log` 14 and `min_match` 4 now honor
  `window_log`. They used a fixed 512 KiB distance, which could pass the
  advertised window.

## [0.9.2] - 2026-10-20

- Add `adaptive::AdaptiveLevel`, which walks the compression level toward
  ratio or speed from per-window `CompressStats` and a `Bottleneck`.
- Add `CompressContext::set_level` and `CompressContext::level`.
- Dictionaries at levels -7 to 2 use the regular fast match finder, so they
  no longer cost ratio. Inputs up to 8 KiB keep the dictionary in a
  separate table.
- Faster encoding of compressible blocks with Huffman literals. Output is
  unchanged.

## [0.9.1] - 2026-10-19

- Enforce `#![forbid(unsafe_code)]` for every build. The `paranoid` feature
  remains available and has no effect today.
- Faster match finding and block encoding, with reused sequence buffers
  and less work on incompressible input.
- Retune levels: L-7 uses raw literals and long matches for fast encoding
  and decoding; L-8 now runs as L-7. L-6 to L-4 encode faster. L3 and L4
  use a match finder modeled on C zstd's for better speed and ratios.

## [0.9.0] - 2026-09-25

- Add `CompressContext::compress_into` for caller-owned output slices.
- Fix a panic in `CompressContext` with a prepared dictionary when a
  snapshot-restoring input follows one that resized the hash tables.
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
- Update `zrip-core` to `0.11.0`.

## [0.8.7] - 2026-09-10

- Add `CompressContext::set_content_checksum` and `content_checksum` for
  reusable contexts. Checksums remain enabled by default.
- Keep `FrameEncoder` failed after write, flush, or finalization errors,
  including partial writes and `WouldBlock`.
- Update `zrip-core` to `0.10.1`.
