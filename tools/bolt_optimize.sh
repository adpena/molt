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

echo "==> BOLT instrumenting ${BINARY}" >&2
llvm-bolt "$BINARY" -o "$INSTRUMENTED" -instrument \
    -instrumentation-file="$FDATA_PATH" \
    -instrumentation-file-append-pid

if [[ -z "$TRAINING" ]]; then
    echo "==> BOLT training: ${INSTRUMENTED}" >&2
    "$INSTRUMENTED"
else
    TRAINING_COMMAND="${TRAINING//\{binary\}/$INSTRUMENTED}"
    echo "==> BOLT training: ${TRAINING_COMMAND}" >&2
    bash -lc "$TRAINING_COMMAND"
fi

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
if (( ${#PROFILE_FRAGMENTS[@]} == 1 )); then
    FDATA_FOUND="${PROFILE_FRAGMENTS[0]}"
else
    if ! command -v merge-fdata >/dev/null 2>&1; then
        echo "BOLT: multiple profile fragments require merge-fdata" >&2
        exit 1
    fi
    FDATA_FOUND="$WORK_DIR/merged.fdata"
    merge-fdata "${PROFILE_FRAGMENTS[@]}" > "$FDATA_FOUND"
    if [[ ! -s "$FDATA_FOUND" ]]; then
        echo "BOLT: merge-fdata produced no merged profile data" >&2
        exit 1
    fi
fi

echo "==> BOLT optimizing ${BINARY}" >&2
llvm-bolt "$BINARY" -o "$BOLT_BINARY" \
    -data="$FDATA_FOUND" \
    -reorder-blocks=ext-tsp \
    -reorder-functions=hfsort \
    -split-functions \
    -split-all-cold \
    -dyno-stats

if [[ ! -f "$BOLT_BINARY" ]]; then
    echo "BOLT: optimizer did not produce ${BOLT_BINARY}" >&2
    exit 1
fi

SIZE_BEFORE="$(stat --format=%s "$BINARY")"
SIZE_AFTER="$(stat --format=%s "$BOLT_BINARY")"
echo "==> BOLT binary size: ${SIZE_BEFORE} -> ${SIZE_AFTER} bytes" >&2
