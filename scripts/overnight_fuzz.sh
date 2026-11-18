#!/usr/bin/env bash
set -uo pipefail

# Do not use `set -e`: run_logged records each phase result and lets the
# run continue so the summary shows every failure found.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SMOKE="${SMOKE:-0}"
CPU_COUNT="${CPU_COUNT:-6}"
if [ "$SMOKE" = "1" ]; then
  FUZZ_SECONDS_PER_TARGET="${FUZZ_SECONDS_PER_TARGET:-5}"
  FUZZ_TARGET_LIMIT="${FUZZ_TARGET_LIMIT:-1}"
else
  FUZZ_TARGET_LIMIT="${FUZZ_TARGET_LIMIT:-0}"
fi
# Targets that feed the decoder arbitrary bytes keep finding new paths for
# hours. Round-trip targets feed it valid frames and level off in minutes.
# FUZZ_SECONDS_PER_TARGET sets both.
FUZZ_SECONDS_LONG="${FUZZ_SECONDS_LONG:-${FUZZ_SECONDS_PER_TARGET:-1200}}"
FUZZ_SECONDS_SHORT="${FUZZ_SECONDS_SHORT:-${FUZZ_SECONDS_PER_TARGET:-180}}"
# FUZZ_SHARD=i/n runs every n-th target starting at the i-th, to split one
# run across n hosts.
FUZZ_SHARD="${FUZZ_SHARD:-1/1}"
SHARD_INDEX="${FUZZ_SHARD%/*}"
SHARD_COUNT="${FUZZ_SHARD#*/}"
if ! [[ "$SHARD_INDEX" =~ ^[0-9]+$ && "$SHARD_COUNT" =~ ^[0-9]+$ ]] ||
  [ "$SHARD_INDEX" -lt 1 ] || [ "$SHARD_INDEX" -gt "$SHARD_COUNT" ]; then
  printf 'FUZZ_SHARD must be i/n with 1 <= i <= n, got %s\n' "$FUZZ_SHARD" >&2
  exit 2
fi
FUZZ_MAX_LEN="${FUZZ_MAX_LEN:-65536}"
FUZZ_TIMEOUT="${FUZZ_TIMEOUT:-30}"
RUN_ID="${RUN_ID:-$(date +%Y%m%d-%H%M%S)}"
LOG_DIR="${LOG_DIR:-$ROOT/tmp/overnight-fuzz/$RUN_ID}"
STATUS_LOG="$LOG_DIR/status.log"
SUMMARY_LOG="$LOG_DIR/summary.tsv"
TARGETS_LOG="$LOG_DIR/fuzz-targets.txt"

export CARGO_BUILD_JOBS="$CPU_COUNT"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

mkdir -p "$LOG_DIR/artifacts"
: >"$STATUS_LOG"

stamp() {
  date -Iseconds
}

status() {
  local line
  line="$(stamp) $*"
  printf '%s\n' "$line"
  printf '%s\n' "$line" >>"$STATUS_LOG"
}

printf 'phase\tstatus\texit\tstart\tend\tseconds\tlog\n' >"$SUMMARY_LOG"

run_logged() {
  local phase="$1"
  shift

  local log="$LOG_DIR/$phase.log"
  local start_iso end_iso start_epoch end_epoch elapsed rc outcome
  start_iso="$(stamp)"
  start_epoch="$(date +%s)"
  status "START $phase log=$log"

  {
    printf 'phase: %s\n' "$phase"
    printf 'start: %s\n' "$start_iso"
    printf 'command:'
    printf ' %q' "$@"
    printf '\n\n'
    "$@"
  } >"$log" 2>&1
  rc=$?

  end_iso="$(stamp)"
  end_epoch="$(date +%s)"
  elapsed=$((end_epoch - start_epoch))
  if [ "$rc" -eq 0 ]; then
    outcome="ok"
  else
    outcome="fail"
  fi

  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$phase" "$outcome" "$rc" "$start_iso" "$end_iso" "$elapsed" "$log" \
    >>"$SUMMARY_LOG"
  status "DONE $phase status=$outcome exit=$rc seconds=$elapsed"
  return "$rc"
}

fuzz_build() {
  local target

  if [ "$#" -eq 0 ]; then
    cargo +nightly fuzz build --sanitizer none
    return
  fi

  for target in "$@"; do
    cargo +nightly fuzz build --sanitizer none "$target"
  done
}

fuzz_target() {
  local target="$1"
  local artifact_dir="$LOG_DIR/artifacts/$target/"
  local worker_log_dir="$LOG_DIR/worker-logs/$target"
  mkdir -p "$artifact_dir" "$worker_log_dir"

  (
    cd "$worker_log_dir"
    cargo +nightly fuzz run \
      --fuzz-dir "$ROOT/fuzz" \
      --sanitizer none \
      "$target" \
      -- \
      -max_total_time="$(target_seconds "$target")" \
      -jobs="$CPU_COUNT" \
      -workers="$CPU_COUNT" \
      -max_len="$FUZZ_MAX_LEN" \
      -timeout="$FUZZ_TIMEOUT" \
      -print_final_stats=1 \
      -artifact_prefix="$artifact_dir"
  )
}

SHORT_TARGETS=(
  zrip_fuzz_roundtrip_frame
  zrip_fuzz_roundtrip_streaming
  zrip_fuzz_roundtrip_streaming_dict
  zrip_fuzz_roundtrip_dict
  zrip_fuzz_roundtrip_block
  zrip_fuzz_c_compress_zrip_decompress
  zrip_fuzz_zrip_compress_c_decompress
  zrip_fuzz_push_roundtrip
)

target_seconds() {
  if contains_target "$1" "${SHORT_TARGETS[@]}"; then
    printf '%s\n' "$FUZZ_SECONDS_SHORT"
  else
    printf '%s\n' "$FUZZ_SECONDS_LONG"
  fi
}

contains_target() {
  local needle="$1"
  shift
  local target
  for target in "$@"; do
    if [ "$target" = "$needle" ]; then
      return 0
    fi
  done
  return 1
}

# Long targets first, corrupt ones leading, then short targets.
ordered_fuzz_targets() {
  local available=("$@")
  local preferred=(
    zrip_fuzz_corrupt_decompress
    zrip_fuzz_corrupt_streaming
    zrip_fuzz_corrupt_streaming_dict
    zrip_fuzz_corrupt_dict
    zrip_fuzz_corrupt_zrip_output
    zrip_fuzz_corrupt_bitflip
    zrip_fuzz_corrupt_splice
    zrip_fuzz_corrupt_truncate
    zrip_fuzz_corrupt_overwrite
  )
  local target

  for target in "${preferred[@]}"; do
    if contains_target "$target" "${available[@]}"; then
      printf '%s\n' "$target"
    fi
  done

  for target in "${available[@]}"; do
    if ! contains_target "$target" "${preferred[@]}" "${SHORT_TARGETS[@]}"; then
      printf '%s\n' "$target"
    fi
  done

  for target in "${SHORT_TARGETS[@]}"; do
    if contains_target "$target" "${available[@]}"; then
      printf '%s\n' "$target"
    fi
  done
}

main() {
  local failures=0
  local target
  local available_targets=()
  local targets=()
  local fuzz_list_log="$LOG_DIR/fuzz-list.log"

  status "overnight fuzz start run_id=$RUN_ID"
  status "log_dir=$LOG_DIR"
  status "config cpu_count=$CPU_COUNT smoke=$SMOKE"
  status "config fuzz_seconds_long=$FUZZ_SECONDS_LONG fuzz_seconds_short=$FUZZ_SECONDS_SHORT"
  status "config shard=$FUZZ_SHARD"
  status "config fuzz_max_len=$FUZZ_MAX_LEN"

  if ! cargo +nightly fuzz list >"$fuzz_list_log" 2>&1; then
    status "cargo fuzz list failed log=$fuzz_list_log"
    return 1
  fi
  mapfile -t available_targets <"$fuzz_list_log"
  if [ "${#available_targets[@]}" -eq 0 ]; then
    status "cargo fuzz list returned no targets log=$fuzz_list_log"
    return 1
  fi
  mapfile -t targets < <(ordered_fuzz_targets "${available_targets[@]}")
  if [ "$FUZZ_TARGET_LIMIT" -gt 0 ] &&
    [ "${#targets[@]}" -gt "$FUZZ_TARGET_LIMIT" ]; then
  targets=("${targets[@]:0:$FUZZ_TARGET_LIMIT}")
fi
if [ "$SHARD_COUNT" -gt 1 ]; then
  local shard=()
  local i
  for i in "${!targets[@]}"; do
    if [ $((i % SHARD_COUNT)) -eq $((SHARD_INDEX - 1)) ]; then
      shard+=("${targets[$i]}")
    fi
  done
  targets=("${shard[@]}")
fi
if [ "${#targets[@]}" -eq 0 ]; then
  status "no fuzz targets selected"
  return 1
fi
  printf '%s\n' "${targets[@]}" >"$TARGETS_LOG"

  run_logged fuzz_build fuzz_build "${targets[@]}" \
    || failures=$((failures + 1))

  for target in "${targets[@]}"; do
    run_logged "$target" fuzz_target "$target" \
      || failures=$((failures + 1))
  done

  if [ "$failures" -eq 0 ]; then
    status "overnight fuzz complete status=ok"
    return 0
  fi

  status "overnight fuzz complete status=fail failures=$failures"
  return 1
}

main "$@"
