#!/usr/bin/env bash
set -euo pipefail

# throughput_env.sh
# Throughput bootstrap using canonical artifact roots.
#
# Usage:
#   eval "$(tools/throughput_env.sh --print)"
#   tools/throughput_env.sh --apply

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
_resolve_root() {
  local candidate="${MOLT_EXT_ROOT:-$ROOT}"
  mkdir -p "$candidate"
  RESOLVED_ROOT="$(cd "$candidate" && pwd)"
}

_choose_defaults() {
  DEFAULT_MOLT_EXT_ROOT="$RESOLVED_ROOT"
  DEFAULT_CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$DEFAULT_MOLT_EXT_ROOT/target}"
  DEFAULT_MOLT_DIFF_CARGO_TARGET_DIR="$DEFAULT_CARGO_TARGET_DIR"
  DEFAULT_MOLT_CACHE="${MOLT_CACHE:-$DEFAULT_MOLT_EXT_ROOT/.molt_cache}"
  DEFAULT_MOLT_DIFF_ROOT="${MOLT_DIFF_ROOT:-$DEFAULT_MOLT_EXT_ROOT/tmp/diff}"
  DEFAULT_MOLT_DIFF_TMPDIR="${MOLT_DIFF_TMPDIR:-$DEFAULT_MOLT_EXT_ROOT/tmp}"
  DEFAULT_UV_CACHE_DIR="${UV_CACHE_DIR:-$DEFAULT_MOLT_EXT_ROOT/.uv-cache}"
  DEFAULT_TMPDIR="${TMPDIR:-$DEFAULT_MOLT_EXT_ROOT/tmp}"
  DEFAULT_SCCACHE_DIR="${SCCACHE_DIR:-$DEFAULT_MOLT_EXT_ROOT/.sccache}"
  DEFAULT_SCCACHE_SIZE="${SCCACHE_CACHE_SIZE:-10G}"
  DEFAULT_CACHE_MAX_GB="${MOLT_CACHE_MAX_GB:-30}"
}

_emit_exports() {
  cat <<EOF
export MOLT_EXT_ROOT="$DEFAULT_MOLT_EXT_ROOT"
export MOLT_CACHE="$DEFAULT_MOLT_CACHE"
export CARGO_TARGET_DIR="$DEFAULT_CARGO_TARGET_DIR"
export MOLT_DIFF_CARGO_TARGET_DIR="$DEFAULT_MOLT_DIFF_CARGO_TARGET_DIR"
export MOLT_DIFF_ROOT="$DEFAULT_MOLT_DIFF_ROOT"
export MOLT_DIFF_TMPDIR="$DEFAULT_MOLT_DIFF_TMPDIR"
export UV_CACHE_DIR="$DEFAULT_UV_CACHE_DIR"
export TMPDIR="$DEFAULT_TMPDIR"
export MOLT_USE_SCCACHE="${MOLT_USE_SCCACHE:-1}"
export MOLT_DIFF_ALLOW_RUSTC_WRAPPER="${MOLT_DIFF_ALLOW_RUSTC_WRAPPER:-1}"
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
export SCCACHE_DIR="$DEFAULT_SCCACHE_DIR"
export SCCACHE_CACHE_SIZE="$DEFAULT_SCCACHE_SIZE"
export MOLT_CACHE_MAX_GB="$DEFAULT_CACHE_MAX_GB"
export MOLT_CACHE_MAX_AGE_DAYS="${MOLT_CACHE_MAX_AGE_DAYS:-30}"
EOF
}

_apply() {
  eval "$(_emit_exports)"
  mkdir -p \
    "$MOLT_CACHE" \
    "$CARGO_TARGET_DIR" \
    "$MOLT_DIFF_ROOT" \
    "$MOLT_DIFF_TMPDIR" \
    "$UV_CACHE_DIR" \
    "$TMPDIR" \
    "$SCCACHE_DIR"

  if command -v sccache >/dev/null 2>&1; then
    SCCACHE_DIR="$SCCACHE_DIR" sccache --stop-server >/dev/null 2>&1 || true
    SCCACHE_DIR="$SCCACHE_DIR" SCCACHE_CACHE_SIZE="$SCCACHE_CACHE_SIZE" \
      sccache --start-server >/dev/null 2>&1 || true
  fi

  if [[ "${MOLT_CACHE_PRUNE:-1}" != "0" ]]; then
    PYTHONPATH=src UV_NO_SYNC=1 \
      python3 "$ROOT/tools/molt_cache_prune.py" \
      --cache-dir "$MOLT_CACHE" \
      --max-gb "$MOLT_CACHE_MAX_GB" \
      --max-age-days "$MOLT_CACHE_MAX_AGE_DAYS"
  fi

  echo "Configured throughput env:"
  echo "  MOLT_EXT_ROOT=$MOLT_EXT_ROOT"
  echo "  MOLT_CACHE=$MOLT_CACHE"
  echo "  CARGO_TARGET_DIR=$CARGO_TARGET_DIR"
  echo "  MOLT_DIFF_CARGO_TARGET_DIR=$MOLT_DIFF_CARGO_TARGET_DIR"
  echo "  MOLT_DIFF_ROOT=$MOLT_DIFF_ROOT"
  echo "  MOLT_DIFF_TMPDIR=$MOLT_DIFF_TMPDIR"
  echo "  UV_CACHE_DIR=$UV_CACHE_DIR"
  echo "  TMPDIR=$TMPDIR"
  echo "  SCCACHE_DIR=$SCCACHE_DIR"
  echo "  SCCACHE_CACHE_SIZE=$SCCACHE_CACHE_SIZE"
}

main() {
  _resolve_root
  _choose_defaults
  case "${1:---apply}" in
    --print)
      _emit_exports
      ;;
    --apply)
      _apply
      ;;
    *)
      echo "Usage: $0 [--print|--apply]" >&2
      exit 2
      ;;
  esac
}

main "$@"
