#!/bin/bash
# Generate small-input corpus slices for benchmarking.
# Deterministic: head -c from existing corpus files.
set -euo pipefail

CORPUS="bench/corpus"
OUT="$CORPUS/small"
mkdir -p "$OUT"

SOURCES=(dickens.txt hdfs.json xml_collection.xml)
SILESIA_SOURCES=(x-ray)
SIZES=(512 1024 2048 4096 8192 16384 32768 65536 131072)
LABELS=(512 1k 2k 4k 8k 16k 32k 64k 128k)

for src in "${SOURCES[@]}"; do
    path="$CORPUS/$src"
    if [[ ! -f "$path" ]]; then
        echo "skip $src (not found)" >&2
        continue
    fi
    base="${src%.*}"
    for i in "${!SIZES[@]}"; do
        sz="${SIZES[$i]}"
        label="${LABELS[$i]}"
        out="$OUT/${base}_${label}"
        head -c "$sz" "$path" > "$out"
        actual=$(wc -c < "$out")
        echo "$out: $actual bytes"
    done
done

for src in "${SILESIA_SOURCES[@]}"; do
    path="$CORPUS/silesia/$src"
    if [[ ! -f "$path" ]]; then
        echo "skip $src (not found)" >&2
        continue
    fi
    base="${src}"
    for i in "${!SIZES[@]}"; do
        sz="${SIZES[$i]}"
        label="${LABELS[$i]}"
        out="$OUT/${base}_${label}"
        head -c "$sz" "$path" > "$out"
        actual=$(wc -c < "$out")
        echo "$out: $actual bytes"
    done
done
