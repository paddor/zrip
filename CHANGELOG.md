# Changelog

## [Unreleased]

## [0.1.1]

### Changed

- Aligned L1 and L2 compression parameters to C zstd 1.5.7 defaults
  (hash_log 17->14 at L1, 18->16 at L2), reducing hash table pressure on L1
  cache and improving encode throughput on all data.
- L1 encode throughput improved ~18% (193->228 MB/s geomean).
- L1 decode throughput improved ~18% (566->667 MB/s geomean).
- L3 DFast window_log increased from 20 to 21 to match C zstd.

### Added

- Mid-block match ratio bail-out in the Fast encoder: blocks with poor match
  rates are detected and emitted as raw blocks early, avoiding wasted work on
  incompressible data.
- Encoder pre-check in block_encoder that skips FSE/Huffman encoding when
  total match bytes cannot beat a raw block.

## [0.1.0]

- Initial release: Fast/DFast encode (levels -7..4), decode, streaming,
  dictionary compression (COVER/FastCOVER), SIMD-dispatched hot paths, no_std.
