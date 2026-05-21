#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

export MOLT_EXT_ROOT="${MOLT_EXT_ROOT:-$ROOT}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export MOLT_DIFF_CARGO_TARGET_DIR="${MOLT_DIFF_CARGO_TARGET_DIR:-$CARGO_TARGET_DIR}"
export MOLT_CACHE="${MOLT_CACHE:-$ROOT/.molt_cache}"
export MOLT_DIFF_ROOT="${MOLT_DIFF_ROOT:-$ROOT/tmp/diff}"
export MOLT_DIFF_TMPDIR="${MOLT_DIFF_TMPDIR:-$ROOT/tmp}"
export UV_CACHE_DIR="${UV_CACHE_DIR:-$ROOT/.uv-cache}"
export TMPDIR="${TMPDIR:-$ROOT/tmp}"

exec python3 "$ROOT/tools/guarded_exec.py" --prefix MOLT_BENCH --cwd "$ROOT" -- \
  python3 "$ROOT/bench/scripts/run_db_stub.py" "$@"
