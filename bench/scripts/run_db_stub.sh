#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

# The canonical run context owns the artifact root and scratch; it keeps
# every value the caller already set.
. "$ROOT/tools/molt_shell_env.sh"
molt_init_shell_context "$ROOT"
eval "$(molt_run_context_env "$ROOT" --session-prefix db-stub --format posix)"

: "${TMPDIR:?Molt DX resolver did not set TMPDIR}"

exec python3 "$ROOT/tools/guarded_exec.py" --prefix MOLT_BENCH --cwd "$ROOT" -- \
  python3 "$ROOT/bench/scripts/run_db_stub.py" "$@"
