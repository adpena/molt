#!/usr/bin/env bash
set -euo pipefail

# throughput_env.sh
# Throughput bootstrap using canonical artifact roots.
#
# Usage:
#   eval "$(tools/throughput_env.sh --print)"
#   tools/throughput_env.sh --apply

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
_run_context_exports() {
  PYTHONPATH="$ROOT/src${PYTHONPATH:+:$PYTHONPATH}" \
    python3 "$ROOT/tools/run_context_env.py" \
      --root "$ROOT" \
      --session-prefix "${MOLT_SESSION_PREFIX:-throughput}" \
      --prefer-external-artifacts \
      --dx \
      --format posix
}

_choose_defaults() {
  CANONICAL_RUN_CONTEXT_EXPORTS="$(_run_context_exports)"
  eval "$CANONICAL_RUN_CONTEXT_EXPORTS"
}

_emit_exports() {
  printf '%s\n' "$CANONICAL_RUN_CONTEXT_EXPORTS"
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
    "$MOLT_BACKEND_DAEMON_SOCKET_DIR" \
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
  echo "  MOLT_BACKEND_DAEMON_SOCKET_DIR=$MOLT_BACKEND_DAEMON_SOCKET_DIR"
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
