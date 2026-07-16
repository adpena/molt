#!/usr/bin/env bash
# Linux ELF BOLT post-link optimization for Molt release binaries.
# Usage: tools/bolt_optimize.sh <binary> [training command containing {binary}]

set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
    echo "Usage: $0 <binary> [training command containing {binary}]" >&2
    exit 2
fi

BINARY="$1"
TRAINING="${2:-}"
BOLT_BINARY="${BINARY}.bolt"

if [[ ! -f "$BINARY" ]]; then
    echo "BOLT: binary not found: ${BINARY}" >&2
    exit 1
fi
if [[ "$(uname -s)" != "Linux" ]]; then
    echo "BOLT: only Linux ELF binaries are supported" >&2
    exit 1
fi
if ! command -v llvm-bolt >/dev/null 2>&1; then
    echo "BOLT: llvm-bolt not found; install the Molt LLVM toolchain or llvm-bolt" >&2
    exit 1
fi
if [[ -n "$TRAINING" && "$TRAINING" != *"{binary}"* ]]; then
    echo "BOLT: custom training command must contain the {binary} placeholder" >&2
    exit 2
fi

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/molt-bolt.XXXXXXXX")"
trap 'rm -rf -- "$WORK_DIR"' EXIT
INSTRUMENTED="$WORK_DIR/instrumented"
FDATA_PATH="$WORK_DIR/profile.fdata"
TELEMETRY_JSON="${MOLT_BOLT_TELEMETRY_JSON:-}"

now_ns() {
    date +%s%N
}

echo "==> BOLT instrumenting ${BINARY}" >&2
INSTRUMENT_START_NS="$(now_ns)"
llvm-bolt "$BINARY" -o "$INSTRUMENTED" -instrument \
    -instrumentation-file="$FDATA_PATH" \
    -instrumentation-file-append-pid
INSTRUMENT_WALL_NS="$(( $(now_ns) - INSTRUMENT_START_NS ))"

TRAIN_START_NS="$(now_ns)"
if [[ -z "$TRAINING" ]]; then
    echo "==> BOLT training: ${INSTRUMENTED}" >&2
    "$INSTRUMENTED"
else
    printf -v INSTRUMENTED_QUOTED '%q' "$INSTRUMENTED"
    TRAINING_COMMAND="${TRAINING//\{binary\}/$INSTRUMENTED_QUOTED}"
    echo "==> BOLT training: ${TRAINING_COMMAND}" >&2
    bash -lc "$TRAINING_COMMAND"
fi
TRAIN_WALL_NS="$(( $(now_ns) - TRAIN_START_NS ))"

PROFILE_FRAGMENTS=()
for profile in "$FDATA_PATH" "$FDATA_PATH".*; do
    if [[ -s "$profile" ]]; then
        PROFILE_FRAGMENTS+=("$profile")
    fi
done
if (( ${#PROFILE_FRAGMENTS[@]} == 0 )); then
    echo "BOLT: training produced no profile data" >&2
    exit 1
fi
PROFILE_FRAGMENT_BYTES=0
for profile in "${PROFILE_FRAGMENTS[@]}"; do
    PROFILE_FRAGMENT_BYTES="$(( PROFILE_FRAGMENT_BYTES + $(stat --format=%s "$profile") ))"
done
MERGE_WALL_NS=0
if (( ${#PROFILE_FRAGMENTS[@]} == 1 )); then
    FDATA_FOUND="${PROFILE_FRAGMENTS[0]}"
else
    if ! command -v merge-fdata >/dev/null 2>&1; then
        echo "BOLT: multiple profile fragments require merge-fdata" >&2
        exit 1
    fi
    FDATA_FOUND="$WORK_DIR/merged.fdata"
    MERGE_START_NS="$(now_ns)"
    merge-fdata "${PROFILE_FRAGMENTS[@]}" > "$FDATA_FOUND"
    MERGE_WALL_NS="$(( $(now_ns) - MERGE_START_NS ))"
    if [[ ! -s "$FDATA_FOUND" ]]; then
        echo "BOLT: merge-fdata produced no merged profile data" >&2
        exit 1
    fi
fi

echo "==> BOLT optimizing ${BINARY}" >&2
OPTIMIZE_START_NS="$(now_ns)"
llvm-bolt "$BINARY" -o "$BOLT_BINARY" \
    -data="$FDATA_FOUND" \
    -reorder-blocks=ext-tsp \
    -reorder-functions=hfsort \
    -split-functions \
    -split-all-cold \
    -dyno-stats
OPTIMIZE_WALL_NS="$(( $(now_ns) - OPTIMIZE_START_NS ))"

if [[ ! -f "$BOLT_BINARY" ]]; then
    echo "BOLT: optimizer did not produce ${BOLT_BINARY}" >&2
    exit 1
fi

SIZE_BEFORE="$(stat --format=%s "$BINARY")"
SIZE_AFTER="$(stat --format=%s "$BOLT_BINARY")"
echo "==> BOLT binary size: ${SIZE_BEFORE} -> ${SIZE_AFTER} bytes" >&2

if [[ -n "$TELEMETRY_JSON" ]]; then
    TELEMETRY_DIR="$(dirname -- "$TELEMETRY_JSON")"
    mkdir -p -- "$TELEMETRY_DIR"
    TELEMETRY_TMP="${TELEMETRY_JSON}.tmp.$$"
    printf '{"schema_version":1,"instrument_wall_ns":%s,"train_wall_ns":%s,"merge_wall_ns":%s,"optimize_wall_ns":%s,"profile_fragment_count":%s,"profile_fragment_bytes":%s,"input_size_bytes":%s,"optimized_size_bytes":%s}\n' \
        "$INSTRUMENT_WALL_NS" \
        "$TRAIN_WALL_NS" \
        "$MERGE_WALL_NS" \
        "$OPTIMIZE_WALL_NS" \
        "${#PROFILE_FRAGMENTS[@]}" \
        "$PROFILE_FRAGMENT_BYTES" \
        "$SIZE_BEFORE" \
        "$SIZE_AFTER" > "$TELEMETRY_TMP"
    mv -f -- "$TELEMETRY_TMP" "$TELEMETRY_JSON"
fi
