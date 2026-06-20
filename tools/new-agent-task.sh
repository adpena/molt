#!/usr/bin/env bash
set -euo pipefail

# new-agent-task.sh
# Creates a standard agent task directory under logs/agents/<task-slug>/,
# writes an initial progress report, and captures the canonical throughput env
# each parallel agent must source before build/test/bench work.

if [ $# -lt 1 ]; then
  echo "Usage: new-agent-task <task-name>"
  exit 1
fi

TASK="$1"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TASK_SLUG="$(printf '%s' "$TASK" \
  | tr '[:upper:]' '[:lower:]' \
  | sed -E 's/[^a-z0-9._-]+/-/g; s/^-+//; s/-+$//')"
if [ -z "$TASK_SLUG" ]; then
  echo "Task name must contain at least one ASCII letter, digit, dot, underscore, or hyphen" >&2
  exit 2
fi

TS="$(date -u +%Y%m%d-%H%M%S)"
SESSION_ID="${MOLT_SESSION_ID:-agent-${TASK_SLUG}-${TS}}"
BASE="$ROOT/logs/agents/$TASK_SLUG"

mkdir -p "$BASE/artifacts"

MOLT_SESSION_ID="$SESSION_ID" \
MOLT_SESSION_PREFIX="agent-${TASK_SLUG}" \
  "$ROOT/tools/throughput_env.sh" --print > "$BASE/env.sh"

set -a
# shellcheck disable=SC1090
. "$BASE/env.sh"
set +a

cat > "$BASE/report_${TS}.md" <<EOF
# Agent Progress Report

## Meta
- Task: $TASK
- Task Slug: $TASK_SLUG
- Report ID: $TS
- Env: $BASE/env.sh
- Agent:
- Repo:
- Branch/Commit:
- Session:
- Status: running | paused | blocked | done

## Summary
- Initialized task directory with canonical Molt throughput environment

## Outputs
- Artifacts:
  - $BASE/artifacts/
- Logs:
  - $BASE/progress.log
- Environment:
  - $BASE/env.sh

## Canonical Env
- MOLT_SESSION_ID: $MOLT_SESSION_ID
- MOLT_EXT_ROOT: $MOLT_EXT_ROOT
- CARGO_TARGET_DIR: $CARGO_TARGET_DIR
- MOLT_DIFF_CARGO_TARGET_DIR: $MOLT_DIFF_CARGO_TARGET_DIR
- MOLT_CACHE: $MOLT_CACHE
- MOLT_DIFF_ROOT: $MOLT_DIFF_ROOT
- MOLT_DIFF_TMPDIR: $MOLT_DIFF_TMPDIR
- TMPDIR: $TMPDIR
- MOLT_BACKEND_DAEMON_SOCKET_DIR: $MOLT_BACKEND_DAEMON_SOCKET_DIR
- SCCACHE_DIR: $SCCACHE_DIR

## Next Steps
1) source "$BASE/env.sh"
2) Fill Meta fields
3) Write plan
4) Start implementation

## Resume Instructions
- cd "$ROOT"
- source "$BASE/env.sh"
- tmux attach -t molt
EOF

touch "$BASE/progress.log"
printf '[%s] initialized task=%s session=%s env=%s\n' \
  "$TS" "$TASK_SLUG" "$MOLT_SESSION_ID" "$BASE/env.sh" >> "$BASE/progress.log"

echo "Created task scaffold at $BASE"
