#!/usr/bin/env bash
set -euo pipefail

jobs="${MIRI_JOBS:-6}"

cargo +nightly miri test -p zrip-encode count_match

filters=(
  fast_vec::tests::fast_extend_from_slice_all_sizes
  fast_vec::tests::wild_copy_match_offsets_1_8
  fast_vec::tests::wild_copy_match_offsets_9_16
  fast_vec::tests::wild_copy_match_offsets_17_32
  fast_vec::tests::wild_copy_match_offsets_33_64
  fast_vec::tests::wild_copy_match_single_offsets_1_8
  fast_vec::tests::wild_copy_match_single_offsets_9_16
  fast_vec::tests::wild_copy_match_single_offsets_17_32
  fast_vec::tests::wild_copy_match_single_offsets_33_64
  fast_vec::tests::wild_copy_match_single_offsets_65_96
  fast_vec::tests::wild_copy_match_single_offsets_97_128
)

printf '%s\0' "${filters[@]}" |
  xargs -0 -P "$jobs" -I {} \
    cargo +nightly miri test -p zrip-decode {} -- --exact
