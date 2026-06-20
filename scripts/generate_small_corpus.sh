#!/bin/bash
# Generate small-input corpus slices for benchmarking.
# Deterministic: head -c from existing corpus files.
set -euo pipefail

CORPUS="bench/corpus"
OUT="$CORPUS/small"
mkdir -p "$OUT"

SOURCES=(dickens.txt hdfs.json xml_collection.xml)
SIZES=(2048 8192 32768 131072)
LABELS=(2k 8k 32k 128k)

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
