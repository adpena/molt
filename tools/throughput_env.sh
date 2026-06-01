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
      --prefer-external-artifacts
}

_daemon_socket_dir() {
  local socket_root="${MOLT_BACKEND_DAEMON_SOCKET_ROOT:-/tmp}"
  local root_hash
  root_hash="$(python3 -c 'import hashlib, sys; print(hashlib.sha256(sys.argv[1].encode()).hexdigest()[:12])' "$ROOT")"
  printf '%s\n' "$socket_root/molt-backend-$root_hash"
}

_choose_defaults() {
  CANONICAL_RUN_CONTEXT_EXPORTS="$(_run_context_exports)"
  eval "$CANONICAL_RUN_CONTEXT_EXPORTS"
  DEFAULT_MOLT_EXT_ROOT="$MOLT_EXT_ROOT"
  DEFAULT_CARGO_TARGET_DIR="$CARGO_TARGET_DIR"
  DEFAULT_MOLT_DIFF_CARGO_TARGET_DIR="$MOLT_DIFF_CARGO_TARGET_DIR"
  DEFAULT_MOLT_CACHE="$MOLT_CACHE"
  DEFAULT_MOLT_DIFF_ROOT="$MOLT_DIFF_ROOT"
  DEFAULT_MOLT_DIFF_TMPDIR="$MOLT_DIFF_TMPDIR"
  DEFAULT_UV_CACHE_DIR="$UV_CACHE_DIR"
  DEFAULT_TMPDIR="$TMPDIR"
  DEFAULT_MOLT_BACKEND_DAEMON_SOCKET_DIR="${MOLT_BACKEND_DAEMON_SOCKET_DIR:-$(_daemon_socket_dir)}"
  DEFAULT_SCCACHE_DIR="${SCCACHE_DIR:-$DEFAULT_MOLT_EXT_ROOT/.sccache}"
  DEFAULT_SCCACHE_SIZE="${SCCACHE_CACHE_SIZE:-10G}"
  DEFAULT_CACHE_MAX_GB="${MOLT_CACHE_MAX_GB:-30}"
}

_emit_exports() {
  printf '%s\n' "$CANONICAL_RUN_CONTEXT_EXPORTS"
  cat <<EOF
export MOLT_BACKEND_DAEMON_SOCKET_DIR="$DEFAULT_MOLT_BACKEND_DAEMON_SOCKET_DIR"
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
