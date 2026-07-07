#!/usr/bin/env bash
set -uo pipefail

# Do not use `set -e`: run_logged records each phase result and lets the
# audit continue so the summary shows every failure found during the run.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SMOKE="${SMOKE:-0}"
CPU_COUNT="${CPU_COUNT:-6}"
MIRI_TEST_THREADS="${MIRI_TEST_THREADS:-1}"
if [ "$SMOKE" = "1" ]; then
  FUZZ_SECONDS_PER_TARGET="${FUZZ_SECONDS_PER_TARGET:-5}"
  FUZZ_TARGET_LIMIT="${FUZZ_TARGET_LIMIT:-1}"
  RUN_MIRI_UNSAFE="${RUN_MIRI_UNSAFE:-0}"
  RUN_FULL_MIRI="${RUN_FULL_MIRI:-0}"
else
  FUZZ_SECONDS_PER_TARGET="${FUZZ_SECONDS_PER_TARGET:-1800}"
  FUZZ_TARGET_LIMIT="${FUZZ_TARGET_LIMIT:-0}"
  RUN_MIRI_UNSAFE="${RUN_MIRI_UNSAFE:-1}"
  RUN_FULL_MIRI="${RUN_FULL_MIRI:-1}"
fi
FUZZ_MAX_LEN="${FUZZ_MAX_LEN:-65536}"
FUZZ_TIMEOUT="${FUZZ_TIMEOUT:-30}"
FUZZ_SANITIZER="${FUZZ_SANITIZER:-address}"
if [ -z "${FUZZ_JOBS_WHILE_MIRI+x}" ]; then
  if [ "$CPU_COUNT" -gt 1 ]; then
    FUZZ_JOBS_WHILE_MIRI="$((CPU_COUNT - 1))"
  else
    FUZZ_JOBS_WHILE_MIRI=1
  fi
fi
RUN_ID="${RUN_ID:-$(date +%Y%m%d-%H%M%S)}"
LOG_DIR="${LOG_DIR:-$ROOT/tmp/overnight-memory-audit/$RUN_ID}"
STATUS_LOG="$LOG_DIR/status.log"
SUMMARY_LOG="$LOG_DIR/summary.tsv"
MIRI_SUMMARY_LOG="$LOG_DIR/miri-summary.tsv"
TARGETS_LOG="$LOG_DIR/fuzz-targets.txt"
if [ -z "${MIRI_STRICT_FLAGS+x}" ]; then
  MIRI_STRICT_FLAGS="-Zmiri-symbolic-alignment-check"
  MIRI_STRICT_FLAGS+=" -Zmiri-retag-fields"
  MIRI_STRICT_FLAGS+=" -Zmiri-many-seeds=0..256"
fi

export CARGO_BUILD_JOBS="$CPU_COUNT"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"
if [ -z "${ASAN_OPTIONS+x}" ]; then
  ASAN_OPTIONS="detect_stack_use_after_return=1"
  ASAN_OPTIONS+=":strict_string_checks=1"
  ASAN_OPTIONS+=":detect_leaks=1"
  ASAN_OPTIONS+=":halt_on_error=1"
  ASAN_OPTIONS+=":abort_on_error=1"
  ASAN_OPTIONS+=":symbolize=1"
fi
export ASAN_OPTIONS
export LSAN_OPTIONS="${LSAN_OPTIONS:-print_suppressions=0}"

mkdir -p "$LOG_DIR/artifacts"
: >"$STATUS_LOG"
: >"$SUMMARY_LOG"
: >"$MIRI_SUMMARY_LOG"

stamp() {
  date -Iseconds
}

status() {
  local line
  line="$(stamp) $*"
  printf '%s\n' "$line"
  printf '%s\n' "$line" >>"$STATUS_LOG"
}

append_summary_header() {
  local path="$1"
  printf 'phase\tstatus\texit\tstart\tend\tseconds\tlog\n' >"$path"
}

append_summary_header "$SUMMARY_LOG"
append_summary_header "$MIRI_SUMMARY_LOG"

run_logged() {
  local summary="$1"
  local phase="$2"
  shift 2

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
    >>"$summary"
  status "DONE $phase status=$outcome exit=$rc seconds=$elapsed"
  return "$rc"
}

miri_unsafe_primitives() {
  MIRI_JOBS="$CPU_COUNT" scripts/miri_unsafe_primitives.sh
}

miri_full_alloc() {
  MIRIFLAGS="$MIRI_STRICT_FLAGS" \
    cargo +nightly miri test -j "$CPU_COUNT" \
      --no-default-features \
      --features alloc \
      -- \
      --no-capture \
      --test-threads="$MIRI_TEST_THREADS"
}

miri_decode_smoke() {
  cargo +nightly miri test -j "$CPU_COUNT" \
    -- \
    roundtrip_small_offset \
    roundtrip_rle_like \
    roundtrip_varied_literal \
    --no-capture \
    --test-threads="$MIRI_TEST_THREADS"
}

miri_dict_decode() {
  MIRIFLAGS="-Zmiri-disable-isolation" \
    cargo +nightly miri test -j "$CPU_COUNT" \
      -- \
      fuzz_corpus_dict_decode_miri \
      --ignored \
      --no-capture \
      --test-threads="$MIRI_TEST_THREADS"
}

fuzz_build() {
  local target

  if [ "$#" -eq 0 ]; then
    cargo +nightly fuzz build --sanitizer "$FUZZ_SANITIZER"
    return
  fi

  for target in "$@"; do
    cargo +nightly fuzz build --sanitizer "$FUZZ_SANITIZER" "$target"
  done
}

fuzz_target() {
  local target="$1"
  local jobs="$2"
  local artifact_dir="$LOG_DIR/artifacts/$target/"
  local worker_log_dir="$LOG_DIR/worker-logs/$target"
  mkdir -p "$artifact_dir" "$worker_log_dir"

  (
    cd "$worker_log_dir"
    cargo +nightly fuzz run \
      --fuzz-dir "$ROOT/fuzz" \
      --sanitizer "$FUZZ_SANITIZER" \
      "$target" \
      -- \
      -max_total_time="$FUZZ_SECONDS_PER_TARGET" \
      -jobs="$jobs" \
      -workers="$jobs" \
      -max_len="$FUZZ_MAX_LEN" \
      -timeout="$FUZZ_TIMEOUT" \
      -detect_leaks=1 \
      -print_final_stats=1 \
      -artifact_prefix="$artifact_dir"
  )
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

ordered_fuzz_targets() {
  local available=("$@")
  local preferred=(
    fuzz_corrupt_decompress
    fuzz_corrupt_streaming
    fuzz_corrupt_streaming_dict
    fuzz_corrupt_dict
    fuzz_corrupt_zrip_output
    fuzz_corrupt_bitflip
    fuzz_corrupt_splice
    fuzz_corrupt_truncate
    fuzz_corrupt_overwrite
    fuzz_roundtrip_frame
    fuzz_roundtrip_streaming
    fuzz_roundtrip_streaming_dict
    fuzz_roundtrip_dict
    fuzz_roundtrip_block
    fuzz_c_compress_zrip_decompress
    fuzz_zrip_compress_c_decompress
  )
  local target

  for target in "${preferred[@]}"; do
    if contains_target "$target" "${available[@]}"; then
      printf '%s\n' "$target"
    fi
  done

  for target in "${available[@]}"; do
    if ! contains_target "$target" "${preferred[@]}"; then
      printf '%s\n' "$target"
    fi
  done
}

run_miri_background_suite() {
  local failures=0

  run_logged "$MIRI_SUMMARY_LOG" miri_full_alloc miri_full_alloc \
    || failures=$((failures + 1))
  run_logged "$MIRI_SUMMARY_LOG" miri_decode_smoke miri_decode_smoke \
    || failures=$((failures + 1))
  run_logged "$MIRI_SUMMARY_LOG" miri_dict_decode miri_dict_decode \
    || failures=$((failures + 1))

  return "$failures"
}

fuzz_jobs_for_now() {
  local miri_pid="${1:-}"
  if [ -n "$miri_pid" ] && kill -0 "$miri_pid" 2>/dev/null; then
    printf '%s\n' "$FUZZ_JOBS_WHILE_MIRI"
  else
    printf '%s\n' "$CPU_COUNT"
  fi
}

main() {
  local failures=0
  local miri_pid=""
  local target jobs
  local available_targets=()
  local targets=()
  local fuzz_list_log="$LOG_DIR/fuzz-list.log"

  status "overnight memory audit start run_id=$RUN_ID"
  status "log_dir=$LOG_DIR"
  status "config cpu_count=$CPU_COUNT smoke=$SMOKE sanitizer=$FUZZ_SANITIZER"
  status "config run_miri_unsafe=$RUN_MIRI_UNSAFE"
  status "config run_full_miri=$RUN_FULL_MIRI"
  status "config fuzz_jobs_while_miri=$FUZZ_JOBS_WHILE_MIRI"
  status "config fuzz_seconds_per_target=$FUZZ_SECONDS_PER_TARGET"
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
  printf '%s\n' "${targets[@]}" >"$TARGETS_LOG"

  run_logged "$SUMMARY_LOG" fuzz_build fuzz_build "${targets[@]}" \
    || failures=$((failures + 1))

  if [ "$RUN_MIRI_UNSAFE" = "1" ]; then
    run_logged "$SUMMARY_LOG" miri_unsafe_primitives miri_unsafe_primitives \
      || failures=$((failures + 1))
  fi

  if [ "$RUN_FULL_MIRI" = "1" ]; then
    status "START miri_background_suite"
    run_miri_background_suite >"$LOG_DIR/miri-background.log" 2>&1 &
    miri_pid=$!
    printf '%s\n' "$miri_pid" >"$LOG_DIR/miri.pid"
    status "miri_background_suite pid=$miri_pid"
  fi

  for target in "${targets[@]}"; do
    jobs="$(fuzz_jobs_for_now "$miri_pid")"
    run_logged "$SUMMARY_LOG" "$target" fuzz_target "$target" "$jobs" \
      || failures=$((failures + 1))
  done

  if [ -n "$miri_pid" ]; then
    status "WAIT miri_background_suite pid=$miri_pid"
    if wait "$miri_pid"; then
      status "DONE miri_background_suite status=ok"
    else
      status "DONE miri_background_suite status=fail"
      failures=$((failures + 1))
    fi
    sed '1d' "$MIRI_SUMMARY_LOG" >>"$SUMMARY_LOG"
  fi

  if [ "$failures" -eq 0 ]; then
    status "overnight memory audit complete status=ok"
    return 0
  fi

  status "overnight memory audit complete status=fail failures=$failures"
  return 1
}

main "$@"
