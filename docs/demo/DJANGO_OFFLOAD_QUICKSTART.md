# Django Offload Quickstart

1. Install deps:
```
uv sync --group demo --python 3.12
uv sync --group dev --python 3.12  # optional: tests/lint
```

2. Build/run worker (compiled exports):
```
cargo run -p molt-worker -- --stdio --exports demo/molt_worker_app/molt_exports.json --compiled-exports demo/molt_worker_app/molt_exports.json
export MOLT_WORKER_CMD="target/debug/molt-worker --stdio --exports demo/molt_worker_app/molt_exports.json --compiled-exports demo/molt_worker_app/molt_exports.json"
```

3. Start Django:
```
cd demo/django_app
python3 manage.py runserver
```

4. Hit endpoints:
- `http://127.0.0.1:8000/health/`
- `http://127.0.0.1:8000/baseline/?user_id=1` vs `/offload/?user_id=1`
- `http://127.0.0.1:8000/compute/?values=1,2,3&scale=2&offset=1` vs `/compute_offload/?...`
- `http://127.0.0.1:8000/offload_table/?rows=10000`

5. Perf harness:
```
python3 bench/scripts/run_stack.sh
```
Artifacts land in `bench/results/` (k6 JSON + markdown summary).
Worker metrics land in `/tmp/molt_demo_metrics.jsonl` unless `MOLT_DEMO_METRICS_PATH` is set.
Process CPU/RSS summaries are captured when the bench script can read the server/worker PIDs.
Set `MOLT_ACCEL_CLIENT_MODE=per_request` to spawn a worker client per request (default is `shared`).
Set `MOLT_UV_SYNC=0` to skip the automatic `uv sync --group demo` step in the bench script.
Set `MOLT_SERVER=gunicorn|uvicorn|django` to choose the server (default `auto` prefers gunicorn, then uvicorn).
Set `MOLT_SERVER_THREADS=2` to control gunicorn threads (defaults to 2 and uses the `gthread` worker class).
Set `MOLT_SERVER_WORKERS` to override server worker count (defaults to min(4, CPU cores)).
Set `MOLT_SERVER_KEEPALIVE=15` to control keep-alive seconds for gunicorn/uvicorn.
Use `K6_TARGET`, `K6_SLEEP_MS`, `K6_WARMUP`, `K6_STEADY`, and `K6_COOLDOWN` to tune load.

Troubleshooting:
- 503: worker not reachable (check `MOLT_WORKER_CMD` or worker logs).
- 400: payload builder mismatch or invalid query params.
- Codec mismatch: ensure manifest `codec_in/out` matches decorator `codec`.
