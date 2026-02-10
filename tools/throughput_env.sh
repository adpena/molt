#!/usr/bin/env bash
set -euo pipefail

# throughput_env.sh
# External-volume-first build throughput bootstrap.
#
# Usage:
#   eval "$(tools/throughput_env.sh --print)"
#   tools/throughput_env.sh --apply

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXTERNAL_ROOT="/Volumes/APDataStore/Molt"

_choose_defaults() {
  if [[ -d "$EXTERNAL_ROOT" ]]; then
    DEFAULT_MOLT_CACHE="$EXTERNAL_ROOT/molt_cache"
    DEFAULT_SCCACHE_DIR="$EXTERNAL_ROOT/sccache"
    DEFAULT_SCCACHE_SIZE="20G"
    DEFAULT_CACHE_MAX_GB="200"
  else
    DEFAULT_MOLT_CACHE="${HOME}/Library/Caches/molt"
    DEFAULT_SCCACHE_DIR="${HOME}/Library/Caches/Mozilla.sccache"
    DEFAULT_SCCACHE_SIZE="10G"
    DEFAULT_CACHE_MAX_GB="30"
  fi
  # Keep Rust incremental artifacts on local APFS/ext4 for hard-link behavior.
  DEFAULT_CARGO_TARGET_DIR="${HOME}/.molt/throughput_target"
}

_emit_exports() {
  cat <<EOF
export MOLT_CACHE="${MOLT_CACHE:-$DEFAULT_MOLT_CACHE}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$DEFAULT_CARGO_TARGET_DIR}"
export MOLT_DIFF_CARGO_TARGET_DIR="${MOLT_DIFF_CARGO_TARGET_DIR:-$CARGO_TARGET_DIR}"
export MOLT_USE_SCCACHE="${MOLT_USE_SCCACHE:-1}"
export MOLT_DIFF_ALLOW_RUSTC_WRAPPER="${MOLT_DIFF_ALLOW_RUSTC_WRAPPER:-1}"
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
export SCCACHE_DIR="${SCCACHE_DIR:-$DEFAULT_SCCACHE_DIR}"
export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-$DEFAULT_SCCACHE_SIZE}"
export MOLT_CACHE_MAX_GB="${MOLT_CACHE_MAX_GB:-$DEFAULT_CACHE_MAX_GB}"
export MOLT_CACHE_MAX_AGE_DAYS="${MOLT_CACHE_MAX_AGE_DAYS:-30}"
EOF
}

_apply() {
  eval "$(_emit_exports)"
  mkdir -p "$MOLT_CACHE" "$CARGO_TARGET_DIR" "$SCCACHE_DIR"

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
  echo "  MOLT_CACHE=$MOLT_CACHE"
  echo "  CARGO_TARGET_DIR=$CARGO_TARGET_DIR"
  echo "  MOLT_DIFF_CARGO_TARGET_DIR=$MOLT_DIFF_CARGO_TARGET_DIR"
  echo "  SCCACHE_DIR=$SCCACHE_DIR"
  echo "  SCCACHE_CACHE_SIZE=$SCCACHE_CACHE_SIZE"
}

main() {
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
