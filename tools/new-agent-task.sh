#!/usr/bin/env bash
set -euo pipefail

if [ $# -lt 1 ]; then
  echo "Usage: new-agent-task <task-name> [agent-coordination-init-args...]"
  exit 1
fi

if [ -n "${PYTHON:-}" ]; then
  PYTHON_CMD=("$PYTHON")
elif command -v uv >/dev/null 2>&1; then
  exec uv run --python 3.12 python tools/agent_coordination.py init "$@"
elif command -v python >/dev/null 2>&1; then
  PYTHON_CMD=(python)
elif command -v py >/dev/null 2>&1; then
  PYTHON_CMD=(py -3.12)
elif command -v python3 >/dev/null 2>&1; then
  PYTHON3_PATH="$(command -v python3)"
  NORMALIZED_PYTHON3_PATH="$(printf '%s' "$PYTHON3_PATH" | tr '[:upper:]\\' '[:lower:]/')"
  case "$NORMALIZED_PYTHON3_PATH" in
    *"/microsoft/windowsapps/"*)
      echo "Refusing WindowsApps python3 alias; install Python or use uv." >&2
      exit 2
      ;;
  esac
  PYTHON_CMD=(python3)
else
  echo "No usable Python launcher found; install uv or set PYTHON." >&2
  exit 2
fi
exec "${PYTHON_CMD[@]}" tools/agent_coordination.py init "$@"
