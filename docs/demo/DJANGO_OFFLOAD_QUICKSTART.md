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
If you want async Postgres-backed `db_query`, set:
```
export MOLT_WORKER_RUNTIME=async
export MOLT_DB_POSTGRES_DSN="postgres://user:pass@localhost:5432/dbname"
```

3. Start Django:
```
cd demo/django_app
uv run --python 3.12 python3 manage.py runserver
```

4. Hit endpoints:
- `http://127.0.0.1:8000/health/`
- `http://127.0.0.1:8000/baseline/?user_id=1` vs `/offload/?user_id=1`
- `http://127.0.0.1:8000/compute/?values=1,2,3&scale=2&offset=1` vs `/compute_offload/?...`
- `http://127.0.0.1:8000/offload_table/?rows=10000`
  (or `POST /offload_table/` with JSON `{"rows": 10000}` to override rows)

5. Perf harness:
```
bench/scripts/run_stack.sh
```
Artifacts land in `bench/results/` (k6 JSON + markdown summary).
Worker metrics land in `/tmp/molt_demo_metrics.jsonl` unless `MOLT_DEMO_METRICS_PATH` is set.
Set `MOLT_FAKE_DB_DELAY_MS` to simulate base DB latency,
`MOLT_FAKE_DB_DECODE_US_PER_ROW` to simulate per-row decode cost, and
`MOLT_FAKE_DB_CPU_ITERS` to simulate per-row CPU work.
Set `MOLT_DEMO_DB_PATH` to enable SQLite-backed reads for `/baseline` and `/offload`;
seed it with `uv run --python 3.12 python3 -m demoapp.db_seed --path "$MOLT_DEMO_DB_PATH"` (or let
`bench/scripts/run_stack.sh` seed automatically). The worker reads
`MOLT_DB_SQLITE_PATH` (defaults to `MOLT_DEMO_DB_PATH` in the bench script). Use
`MOLT_DB_SQLITE_READWRITE=1` to open the worker connection read-write (default is
read-only).
Set `MOLT_WORKER_RUNTIME=async` + `MOLT_DB_POSTGRES_DSN` to use Postgres for `db_query`.
Tune async pool behavior with `MOLT_DB_POSTGRES_QUERY_TIMEOUT_MS`,
`MOLT_DB_POSTGRES_MAX_WAIT_MS`, and `MOLT_DB_POSTGRES_MAX_CONNS`.
Process CPU/RSS summaries are captured by sampling the process table; the bench
runner uses `MOLT_DEMO_SERVER_PID`/`MOLT_DEMO_WORKER_PID` when available,
prefers listen-PIDs from `MOLT_SERVER_PORT` when `lsof` is present, and falls
back to command-name matching.
Set `MOLT_ACCEL_CLIENT_MODE=per_request` to spawn a worker client per request (default is `shared`).
Set `MOLT_ACCEL_POOL_SIZE` to use a pool of worker processes when `MOLT_ACCEL_CLIENT_MODE=shared`.
Set `MOLT_WORKER_THREADS` or `MOLT_WORKER_MAX_QUEUE` to override worker thread count or queue depth.
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
