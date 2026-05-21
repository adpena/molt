#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

export MOLT_EXT_ROOT="${MOLT_EXT_ROOT:-$REPO_ROOT}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
export MOLT_DIFF_CARGO_TARGET_DIR="${MOLT_DIFF_CARGO_TARGET_DIR:-$CARGO_TARGET_DIR}"
export MOLT_CACHE="${MOLT_CACHE:-$REPO_ROOT/.molt_cache}"
export MOLT_DIFF_ROOT="${MOLT_DIFF_ROOT:-$REPO_ROOT/tmp/diff}"
export MOLT_DIFF_TMPDIR="${MOLT_DIFF_TMPDIR:-$REPO_ROOT/tmp}"
export UV_CACHE_DIR="${UV_CACHE_DIR:-$REPO_ROOT/.uv-cache}"
export TMPDIR="${TMPDIR:-$REPO_ROOT/tmp}"

if [ "$#" -eq 0 ]; then
  set -- "$SCRIPT_DIR"
fi

exec python3 "$REPO_ROOT/tools/parity_gate.py" "$@"
