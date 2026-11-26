# Changelog

## [Unreleased]

## [0.14.4] - 2026-11-26

- `finalize_dictionary` fills `dict_size`: the content takes what the
  header and tables leave, as in C zstd, not `dict_size - 256` bytes.
  `finalize_dictionary` and `select_segments` count sizes under the new
  `MIN_DICT_SIZE` (256) as `MIN_DICT_SIZE`.

## [0.14.3] - 2026-11-26

- `build_huffman_decode_tables_into` builds a Huffman decode table and, if
  asked, its 16-bit fast-loop rows in the same stores, without the second
  pass over the table that `build_row_table` makes.
- On x86 the 4-stream fast loop reads decode table entries again, not
  rows (`FAST_LOOP_ROWS`). `decode_4_streams_rows_into` and
  `decode_4_streams_pairs_rows_into` do not read `rows` there.

## [0.14.2] - 2026-11-25

- Huffman decode tables carry 16-bit rows, the code length in the low byte
  and the symbol in the high byte, as C zstd's `HUF_DEltX1`. The fast
  loops read one row per symbol.
- `Dictionary::from_bytes` rejects repeat offsets larger than the
  dictionary content, as C zstd does. `finalize_dictionary` picks the
  repeat offsets from the content it keeps and pads content shorter than 8
  bytes with zeros in front.

## [0.14.1] - 2026-11-24

- `push::Status::NeedsOutput` says more output may follow, not that it is
  pending: a push decoder returns it at a full buffer before it reads the
  next block header.

## [0.14.0] - 2026-11-21

- `fse::table_builder::build_decode_rows` spreads a distribution as C zstd
  does and hands each cell's row to a callback, so callers with their own
  row type build their table in one pass. `build_decode_table_into` uses
  it.
- `parse_fse_table_description` and `parse_fse_table_description_into` end
  the distribution at its last symbol instead of padding it with zeros to
  `max_symbol + 1`, so its length is C zstd's `maxSymbolValue + 1`.
- Huffman weight tables take symbols up to 255 within C zstd's workspace
  bound: 92 symbols at accuracy log 5, 12 at log 6. Decoded weights above
  12 fail.
- The 4-stream Huffman decoder runs its fast rounds in C zstd's bounds and
  interleaves the four streams' lookups.

## [0.13.0] - 2026-11-20

- Removed unused items: `hash::hash4` to `hash8`, `hash::PRIME32_2`, the
  `simd` module (`has_bmi2`), `fse::FseEncodeEntry`, `fse::FseTable` and
  `fse::FSE_SEQ_DECODE_ENTRY_ZERO`. `build_decode_table_into` and
  `parse_huffman_weights_into` drop their unused `symbol_next` scratch
  argument. The empty `dict::cover` module is hidden from the docs.

- `fastcover::select_segments` fills the dictionary from the end, best
  segment first, and cuts the last pick to fit, as C zstd's
  `COVER_buildDictionary`; it cut the best segment's end.
- `finalize_dictionary` takes the dictionary ID as C zstd's
  `ZDICT_finalizeDictionary` does: XXH64 of the content, mapped into
  `32768..2^31`. The repeat offsets are the three best distinct offsets,
  then the defaults they leave out.

- Huffman decode tables take 12 bits (`huffman::MAX_TABLE_LOG`), as C
  zstd's decoder does; `FAST_TABLE_LOG` (11) bounds the fast loops, and
  `MAX_BITS` (11) the encoder. Weights need at least two symbols of weight
  1, as in C zstd.
- 4-stream Huffman sections take 6 symbols and more, with an empty fourth
  stream at 6 and 9, as in C zstd.
- Frame headers with a window log over 31 (30 on 32-bit targets) fail with
  `WindowTooLarge`.

## [0.12.2] - 2026-11-20

- `finalize_dictionary` writes a Huffman table with a code for every byte,
  weighted by literal counts from the samples, instead of a placeholder that
  coded no literals, as C zstd does.

## [0.12.1] - 2026-11-19

- `HuffmanEncodeTable::from_decode_table` keeps tables with symbols past 128,
  such as a dictionary's. Their weights may go undescribed, so such tables
  are only reused.

## [0.12.0] - 2026-11-18

- Breaking: `HuffmanEncodeTable::encode_4_streams_into` no longer takes a
  stream buffer. It and `encode_single_stream_into` write to the start of
  `out` and return the encoded length; bytes past it are scratch.
- Faster Huffman encoding, as C zstd does it: left-aligned `u64` code
  entries, two bit containers, and four streams written straight into one
  buffer. `byte_histogram` loads 8 bytes at a time.
- Huffman tables build from the active symbols only, with a counting sort and
  a two-queue tree, and limit code lengths in place like `HUF_setMaxHeight`.
  Weight descriptions use stack buffers. Builds take about half the time.
- `prefers_pairs` picks two-symbol decoding only for literal sections of at
  least 24 KiB, where its larger table pays off.
- Add `fse::encode::build_encode_table`,
  `fse::table_builder::normalize_counts_into`, and
  `fse::table_builder::write_fse_table_description`, which work on caller
  buffers.
- Add `frame::find_frame_compressed_size`: the size of the first frame in a
  buffer, from its frame and block headers alone, like C zstd's
  `ZSTD_findFrameCompressedSize`.
- Add the `push` module with `Status` and `Progress`, shared by the push
  encoder and decoder.

## [0.11.1] - 2026-10-19

- Enforce `#![forbid(unsafe_code)]` for every build. The `paranoid` feature
  remains available and has no effect today.
- Faster Huffman decoding, including two-symbol tables for dense literal
  sections, and safe bitstream and XXH64 primitives.
- Add `ReverseBitReader::is_exhausted` to distinguish an exactly consumed
  stream from one read past its end.

## [0.11.0] - 2026-09-25

- Length-limit Huffman codes to `MAX_BITS` instead of returning `None` for
  deep trees.
- Write FSE-compressed Huffman weights, allowing tables for byte values above
  128. Fix `FseEncodeTable` transforms for "less than one" probabilities.
- Add `HuffmanEncodeTable::from_histogram`, `estimate_bits`, and
  `byte_histogram`.
- Encode Huffman streams four symbols per flush with a packed code table.
- Make the Huffman bit writer safe.
- FSE-compress Huffman weights with table log 5, as C zstd does, and flatten
  counts when the weight description would not fit.
- Huffman decode tables always hold `DECODE_TABLE_SIZE` entries. The
  4-stream decoder indexes them without bounds checks and runs a precomputed
  number of rounds without per-round checks.
- Breaking: `build_huffman_decode_table_into` no longer takes the unused
  `rank_count` and `rank_start` buffers.
- Reject more than 255 Huffman weights in `parse_huffman_weights`,
  `parse_huffman_weights_into`, `build_huffman_decode_table`, and
  `build_huffman_decode_table_into`.
- `parse_huffman_weights_into` decodes four FSE weights per refill.
  `build_huffman_decode_table_into` counts and sorts weights without
  branches.
- `build_decode_table_into` keeps symbol states in a stack array.
  `BitReader::read_bits` reads with one 8-byte load.
- `build_decode_table_into` uses C zstd's two-stage spread for tables up to
  512 states without "less than one" probabilities, and sets symbol states
  without branching on zero probabilities.
- `parse_fse_table_description_into` reads each field from a 64-bit window
  instead of calling `read_bits`, and counts runs of zero-probability
  repeat codes in one step.
- Decode the last symbols of a Huffman stream with one shift, lookup, and
  add each, instead of a refill and exact bit count per symbol.
- The 4-stream Huffman decoder's fast rounds keep each stream as a
  left-aligned container with a sentinel bit, as C zstd does: a symbol costs
  one shift, one lookup, and one shift, with no separate bit count, and the
  table index needs no mask.
- `build_huffman_decode_table_into` counts and sorts weights in four
  contiguous symbol partitions with separate counters, so consecutive
  symbols of equal weight no longer wait on each other's counter store.

## [0.10.1] - 2026-09-10

- Accept explicitly encoded dictionary ID zero without requiring a dictionary.
- Ignore the unused frame descriptor bit while retaining reserved-bit checks.
