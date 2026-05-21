#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

export MOLT_EXT_ROOT="${MOLT_EXT_ROOT:-$ROOT}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export MOLT_DIFF_CARGO_TARGET_DIR="${MOLT_DIFF_CARGO_TARGET_DIR:-$CARGO_TARGET_DIR}"
export MOLT_CACHE="${MOLT_CACHE:-$ROOT/.molt_cache}"
export MOLT_DIFF_ROOT="${MOLT_DIFF_ROOT:-$ROOT/tmp/diff}"
export MOLT_DIFF_TMPDIR="${MOLT_DIFF_TMPDIR:-$ROOT/tmp}"
export UV_CACHE_DIR="${UV_CACHE_DIR:-$ROOT/.uv-cache}"
export TMPDIR="${TMPDIR:-$ROOT/tmp}"

SAMPLES="${MOLT_BENCH_SAMPLES:-1}"
WARMUP="${MOLT_BENCH_WARMUP:-0}"

exec python3 "$ROOT/tools/bench.py" \
  --samples "$SAMPLES" \
  --warmup "$WARMUP" \
  "$@"
