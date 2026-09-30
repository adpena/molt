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

3. Seed the demo SQLite DB:
```
cd demo/django_app
uv run --python 3.12 python3 -m demoapp.db_seed --path db.sqlite3
```

4. Start Django:
```
cd demo/django_app
uv run --python 3.12 python3 manage.py runserver
```

5. Hit endpoints:
- `http://127.0.0.1:8000/health/`
- `http://127.0.0.1:8000/baseline/?user_id=1` vs `/offload/?user_id=1`
- `http://127.0.0.1:8000/compute/?values=1,2,3&scale=2&offset=1` vs `/compute_offload/?...`
- `http://127.0.0.1:8000/offload_table/?rows=10000`
  (or `POST /offload_table/` with JSON `{"rows": 10000}` to override rows)

6. Perf harness:
```
bench/scripts/run_stack.sh
```
Each invocation first binds its own run directory,
`bench/results/demo-runs/<run-id>/`, and prints the run id and path. Binding
happens before environment setup, the worker build, preflight, or server
readiness, so a failed startup can only fail its own run. The directory is
created exclusively: a random UUID locally, or exactly `MOLT_DEMO_RUN_DIR` when
that is set to a new `bench/results/demo-runs/<run-id>` path (Nightly CI uses
`perf-demo-<run id>-<attempt>`). An existing directory is refused, and earlier
runs are never modified or deleted.

The directory's `run.json` records the run id and the Git source identity:
HEAD, a dirty flag, and SHA-256 digests of `git status` and the tracked diff.
No source text is stored, and the contents of untracked files are not hashed.
A Git checkout is required. The bench captures the identity again when it
finishes and fails the run if Git became unavailable or the identity changed,
for example because `uv sync` or `cargo build` rewrote a lockfile, a tracked
file was edited, or a file was added or removed during the run.

Everything the run produces stays in its directory: the per-scenario k6
summaries and output logs, one `demo_k6_<timestamp>.json` composite bound to the
run id (with the worker binary's SHA-256 when the stack built or found it), a
markdown summary, and worker metrics in `molt_demo_metrics.jsonl`.
`MOLT_DEMO_METRICS_PATH` may name another metrics file, but that file must not
exist yet. Server and worker logs are shared by all runs: `logs/molt_django.log`
and `logs/molt_worker.log` hold the latest run's output. The stack waits for
`/health/` before load generation and drains its service process groups on
success, failure, or interruption.

Check a run with
`python bench/scripts/run_demo_bench.py --check-regressions bench/results/demo-runs/<run-id>`.
The checker reads only that directory. It requires `run.json`, at most one
composite bound to the same run id, and a source identity that did not change.
The budgets: all three scenarios, finite p95 latency, completed requests, and
error rates below 1%; p95 must stay below 1000 ms for baseline/offload and
1500 ms for offload_table. A run that stopped early is judged by the summaries
it kept, and it still fails: missing scenarios and a missing composite are
failures. Passing `bench/results` or `bench/results/demo-runs` fails. Passing
a composite JSON file checks only that historical artifact; the result is not
tied to any run, and CI never uses this mode.

Nightly CI sets `MOLT_DEMO_RUN_DIR` for each job attempt and checks exactly
that directory even when the stack fails. It uploads the directory together
with the server/worker logs and guard diagnostics.

These budgets are a development signal from one `ubuntu-latest` cell (the
Python in `.python-version` and a `dev-fast` worker). They are absolute, never
compare against CPython, and are not release or acceptance evidence. For the
canonical CPython-relative gate, see
[Running Benchmarks](../BENCHMARKING.md#running-benchmarks).

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
Set `MOLT_ACCEL_RETRY_ON_TIMEOUT=1` to opt into timeout retries for idempotent calls.
Set `MOLT_ACCEL_RETRY_ON_BUSY=1` to retry `Busy` responses for idempotent calls.
Set `MOLT_ACCEL_RETRY_BACKOFF_MS=10` and `MOLT_ACCEL_RETRY_BACKOFF_MAX_MS=80` for exponential retry backoff.
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
