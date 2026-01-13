#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT"
export MOLT_REPO_ROOT="$ROOT"

# Use uv for reproducible demo deps when available.
if command -v uv >/dev/null 2>&1; then
  UV_GROUP="${MOLT_UV_GROUP:-demo}"
  UV_PYTHON="${MOLT_UV_PYTHON:-3.12}"
  RUN_PY=(uv run --project "$ROOT" --group "$UV_GROUP" --python "$UV_PYTHON" python3)
  if [[ "${MOLT_UV_SYNC:-1}" != "0" ]]; then
    uv sync --group "$UV_GROUP" --python "$UV_PYTHON"
  fi
else
  RUN_PY=(python3)
fi

SERVER="${MOLT_SERVER:-auto}"
SERVER_PORT="${MOLT_SERVER_PORT:-8000}"
if [[ -n "${MOLT_SERVER_WORKERS:-}" ]]; then
  SERVER_WORKERS="${MOLT_SERVER_WORKERS}"
else
  CPU_COUNT=2
  if command -v getconf >/dev/null 2>&1; then
    CPU_COUNT="$(getconf _NPROCESSORS_ONLN || echo 2)"
  elif command -v nproc >/dev/null 2>&1; then
    CPU_COUNT="$(nproc || echo 2)"
  fi
  if [[ "$CPU_COUNT" =~ ^[0-9]+$ ]] && (( CPU_COUNT > 0 )); then
    if (( CPU_COUNT > 4 )); then
      SERVER_WORKERS=4
    else
      SERVER_WORKERS="$CPU_COUNT"
    fi
  else
    SERVER_WORKERS=2
  fi
fi
SERVER_THREADS="${MOLT_SERVER_THREADS:-2}"
SERVER_KEEPALIVE="${MOLT_SERVER_KEEPALIVE:-15}"
SERVER_PID_FILE="/tmp/molt_gunicorn.pid"

if [[ "$SERVER" == "auto" ]]; then
  if "${RUN_PY[@]}" - <<'PY'
import importlib.util
import sys

sys.exit(0 if importlib.util.find_spec("gunicorn") else 1)
PY
  then
    SERVER="gunicorn"
  elif "${RUN_PY[@]}" - <<'PY'
import importlib.util
import sys

sys.exit(0 if importlib.util.find_spec("uvicorn") else 1)
PY
  then
    SERVER="uvicorn"
  else
    SERVER="django"
  fi
fi
export MOLT_SERVER="$SERVER"

# Build worker if needed
if ! command -v molt-worker >/dev/null 2>&1; then
  cargo build -p molt-worker
  WORKER_BIN="$ROOT/target/debug/molt-worker"
else
  WORKER_BIN="$(command -v molt-worker)"
fi

EXPORTS="$ROOT/demo/molt_worker_app/molt_exports.json"
WORKER_CMD="$WORKER_BIN --stdio --exports $EXPORTS --compiled-exports $EXPORTS"

# Start worker
$WORKER_CMD > /tmp/molt_worker.log 2>&1 &
WORKER_PID=$!
trap 'kill $WORKER_PID 2>/dev/null || true' EXIT
export MOLT_DEMO_WORKER_PID="$WORKER_PID"

export MOLT_WORKER_CMD="$WORKER_CMD"
export MOLT_ACCEL_CLIENT_MODE="${MOLT_ACCEL_CLIENT_MODE:-shared}"
export DJANGO_SETTINGS_MODULE=demoapp.settings
export PYTHONPATH=$ROOT/src:$ROOT/demo/django_app
METRICS_PATH="${MOLT_DEMO_METRICS_PATH:-/tmp/molt_demo_metrics.jsonl}"
rm -f "$METRICS_PATH"
export MOLT_DEMO_METRICS_PATH="$METRICS_PATH"

# Preflight: ensure the worker command is functional before k6 runs.
"${RUN_PY[@]}" - <<'PY'
import os
import shlex
import sys
from pathlib import Path

root = Path(os.environ["MOLT_REPO_ROOT"]) if "MOLT_REPO_ROOT" in os.environ else None
if root is None:
    root = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(root / "src"))

from molt_accel.client import MoltClient

cmd = shlex.split(os.environ["MOLT_WORKER_CMD"])
wire = os.environ.get("MOLT_WORKER_WIRE") or os.environ.get("MOLT_WIRE")
client = MoltClient(worker_cmd=cmd, wire=wire)
try:
    client.ping(timeout_ms=250)
except Exception as exc:
    print(f"Worker preflight failed: {exc}", file=sys.stderr)
    sys.exit(1)
finally:
    client.close()
PY

# Start Django (server mode)
cd "$ROOT"
if [[ "$SERVER" == "gunicorn" ]]; then
  rm -f "$SERVER_PID_FILE"
fi
case "$SERVER" in
  django)
    SERVER_CMD=("${RUN_PY[@]}" demo/django_app/manage.py runserver "$SERVER_PORT")
    ;;
  gunicorn)
    SERVER_CMD=(
      "${RUN_PY[@]}" -m gunicorn demoapp.wsgi:application
      --bind "127.0.0.1:${SERVER_PORT}"
      --workers "$SERVER_WORKERS"
      --worker-class gthread
      --threads "$SERVER_THREADS"
      --keep-alive "$SERVER_KEEPALIVE"
      --pid "$SERVER_PID_FILE"
      --log-level warning
    )
    ;;
  uvicorn)
    SERVER_CMD=(
      "${RUN_PY[@]}" -m uvicorn demoapp.asgi:application
      --host 127.0.0.1
      --port "$SERVER_PORT"
      --workers "$SERVER_WORKERS"
      --timeout-keep-alive "$SERVER_KEEPALIVE"
      --log-level warning
    )
    ;;
  *)
    echo "Unknown MOLT_SERVER '$SERVER' (expected django|gunicorn|uvicorn|auto)" >&2
    exit 1
    ;;
esac

"${SERVER_CMD[@]}" > /tmp/molt_django.log 2>&1 &
DJ_PID=$!
trap 'kill $WORKER_PID $DJ_PID 2>/dev/null || true' EXIT
export MOLT_DEMO_SERVER_PID="$DJ_PID"
if [[ "$SERVER" == "gunicorn" ]]; then
  for _ in {1..50}; do
    if [[ -s "$SERVER_PID_FILE" ]]; then
      export MOLT_DEMO_SERVER_PID="$(cat "$SERVER_PID_FILE")"
      break
    fi
    sleep 0.1
  done
fi
sleep 2

cd "$ROOT"
"${RUN_PY[@]}" bench/scripts/run_demo_bench.py

kill $DJ_PID $WORKER_PID 2>/dev/null || true
trap - EXIT
