#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

eval "$(
  python3 "$ROOT/tools/run_context_env.py" \
    --root "$ROOT" \
    --session-prefix "${MOLT_SESSION_PREFIX:-bench}" \
    --prefer-external-artifacts \
    --dx \
    --format posix
)"

: "${TMPDIR:?Molt DX resolver did not set TMPDIR}"

SAMPLES="${MOLT_BENCH_SAMPLES:-1}"
WARMUP="${MOLT_BENCH_WARMUP:-0}"

exec python3 "$ROOT/tools/guarded_exec.py" --prefix MOLT_BENCH --cwd "$ROOT" -- \
  python3 "$ROOT/tools/bench.py" \
  --samples "$SAMPLES" \
  --warmup "$WARMUP" \
  "$@"
