#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT"

EXPORTS="$ROOT/demo/molt_worker_app/molt_exports.json"
WORKER_BIN="${WORKER_BIN:-$ROOT/target/debug/molt-worker}"
if [ ! -x "$WORKER_BIN" ]; then
  cargo build -p molt-worker
  WORKER_BIN="$ROOT/target/debug/molt-worker"
fi

MOLT_WORKER_CMD="$WORKER_BIN --stdio --exports $EXPORTS --compiled-exports $EXPORTS"
export MOLT_WORKER_CMD
export MOLT_WIRE=msgpack
export PYTHONPATH="$ROOT/src:$ROOT/demo/django_app"
export DJANGO_SETTINGS_MODULE=demoapp.settings

# For now reuse compute/offload_table to simulate DB-heavy payloads.
python3 - <<'PY'
from django.test import Client
client = Client()
resp = client.get("/offload_table/?rows=20000")
print("offload_table status", resp.status_code)
print(resp.json())
PY
