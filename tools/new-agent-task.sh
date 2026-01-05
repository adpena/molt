#!/usr/bin/env bash
set -euo pipefail

# new-agent-task.sh
# Creates a standard agent task directory under logs/agents/<task-name>/,
# writes an initial progress report, and creates progress.log + artifacts/.

if [ $# -lt 1 ]; then
  echo "Usage: new-agent-task <task-name>"
  exit 1
fi

TASK="$1"
BASE="logs/agents/$TASK"
TS="$(date -u +%Y%m%d-%H%M%S)"

mkdir -p "$BASE/artifacts"

cat > "$BASE/report_${TS}.md" <<EOF
# Agent Progress Report

## Meta
- Task: $TASK
- Report ID: $TS
- Agent:
- Repo:
- Branch/Commit:
- Session:
- Status: running | paused | blocked | done

## Summary
- Initialized task directory

## Outputs
- Artifacts:
  - $BASE/artifacts/
- Logs:
  - $BASE/progress.log

## Next Steps
1) Fill Meta fields
2) Write plan
3) Start implementation

## Resume Instructions
- tmux attach -t molt
EOF

touch "$BASE/progress.log"

echo "Created task scaffold at $BASE"
