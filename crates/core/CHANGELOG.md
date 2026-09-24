# Changelog

## [Unreleased]

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

## [0.10.1] - 2026-09-10

- Accept explicitly encoded dictionary ID zero without requiring a dictionary.
- Ignore the unused frame descriptor bit while retaining reserved-bit checks.
