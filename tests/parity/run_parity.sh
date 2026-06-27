#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

eval "$(
  python3 "$REPO_ROOT/tools/run_context_env.py" \
    --root "$REPO_ROOT" \
    --session-prefix "${MOLT_SESSION_PREFIX:-parity}" \
    --prefer-external-artifacts \
    --dx \
    --format posix
)"

: "${TMPDIR:?Molt DX resolver did not set TMPDIR}"

if [ "$#" -eq 0 ]; then
  set -- "$SCRIPT_DIR"
fi

exec python3 "$REPO_ROOT/tools/parity_gate.py" "$@"
