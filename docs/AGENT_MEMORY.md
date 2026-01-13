# Agent Memory Log

This log is append-only. Record coordination notes, active scope, tests, and
benchmarks here to avoid collisions during parallel work.

## Entries
- 2026-01-12T23:54:58Z codex: created memory log; pending update after k6 install
  and demo bench rerun.
- 2026-01-13T00:04:53Z codex: added coordination guidance in `AGENTS.md` and
  `GEMINI.md` to require reading `docs/AGENT_LOCKS.md` + `docs/AGENT_MEMORY.md`;
  installed k6 via Homebrew; ran demo bench with a temp venv for Django but the
  offload run failed with 503s (worker unavailable; connection reset). Tests:
  `cargo test -p molt-worker` (pass), `uv run --python 3.12 python3 -m pytest
  tests/test_molt_accel_contracts.py tests/test_django_demo.py` (pass).
- 2026-01-13T00:16:34Z codex: added `client_mode` control (shared vs per_request)
  to `molt_offload`, plus shared-client caching and per-request cleanup; updated
  `docs/spec/0912_MOLT_ACCEL_DJANGO_DECORATOR_SPEC.md`. Added a worker preflight
  check + default `MOLT_ACCEL_CLIENT_MODE=shared` in
  `bench/scripts/run_stack.sh`. Fixed worker msgpack payload encoding by using
  `ByteBuf` in `ResponseEnvelope` to avoid list payloads. Manual probe to
  `/offload/` initially returned Django 500 (TypeError on msgpack decode); after
  rebuilding the worker it returned 200 with JSON payload. Tests: `cargo test -p
  molt-worker` (pass). Bench: `PATH=/tmp/molt_bench_venv/bin:$PATH bash
  bench/scripts/run_stack.sh` (pass; baseline/offload/offload_table errors 0%).
- 2026-01-13T00:24:22Z codex: added `demo` dependency group (Django+msgpack),
  updated `bench/scripts/run_stack.sh` to prefer `uv run --project` and run
  Django from repo root, and documented uv-based demo setup in
  `docs/demo/DJANGO_OFFLOAD_QUICKSTART.md`. Updated `uv.lock` via
  `uv lock --python 3.12` and synced demo deps with `uv sync --group demo
  --python 3.12`. Bench: `MOLT_ACCEL_CLIENT_MODE=per_request bash
  bench/scripts/run_stack.sh` (pass; baseline=105.2 req/s, offload=98.1 req/s,
  offload_table=46.8 req/s, errors 0%).
- 2026-01-13T00:27:26Z codex: added optional `uv sync` step to
  `bench/scripts/run_stack.sh` (disable with `MOLT_UV_SYNC=0`) and reran the
  per-request bench with the updated script. Bench:
  `MOLT_ACCEL_CLIENT_MODE=per_request bash bench/scripts/run_stack.sh`
  (pass; baseline=105.3 req/s, offload=98.7 req/s, offload_table=46.8 req/s,
  errors 0%).
- 2026-01-13T00:33:11Z codex: ran follow-up benches for shared and per_request
  modes. Shared: `MOLT_ACCEL_CLIENT_MODE=shared bash bench/scripts/run_stack.sh`
  (pass; baseline=105.3 req/s, offload=103.5 req/s, offload_table=51.9 req/s,
  errors 0%). Per-request with sync disabled: `MOLT_UV_SYNC=0
  MOLT_ACCEL_CLIENT_MODE=per_request bash bench/scripts/run_stack.sh` (pass;
  baseline=104.9 req/s, offload=98.3 req/s, offload_table=46.8 req/s, errors
  0%).
- 2026-01-13T01:04:29Z codex (git 05ff17b73ed99360af4081408e1c4da082500441):
  blocked on missing `references/docs.md` required by proceed workflow; no
  changes or tests run.
- 2026-01-13T01:07:47Z codex: added recursion-limit runtime exports + sys wiring, len(__len__) parity, and sum/min/max builtins; updated wasm/WIT bindings and type coverage/status docs; added differential tests for reductions, len, and recursion limits. Tests: not run (not requested).
- 2026-01-13T01:34:12Z codex: expanded bytes/bytearray encoding support to utf-16/utf-32 variants with error-handler parity for ascii/latin-1, added encoding differential coverage, and updated STATUS/0014 matrix. Tests: not run (not requested).
- 2026-01-13T01:36:19Z codex (git 05ff17b73ed99360af4081408e1c4da082500441):
  added `references/docs.md` with the Django demo spec list required by the
  proceed workflow. Tests: not run (doc-only).
- 2026-01-13T02:06:00Z codex (git 05ff17b73ed99360af4081408e1c4da082500441):
  added gunicorn/uvicorn keep-alive flags in `bench/scripts/run_stack.sh`,
  documented `MOLT_SERVER_KEEPALIVE`, and fixed k6 summary parsing by setting
  `K6_SUMMARY_TREND_STATS` + handling trend stats in `run_demo_bench.py`.
  Bench runs (K6_TARGET=5 K6_SLEEP_MS=50 due to local port exhaustion at higher
  targets): gunicorn shared (baseline=72.0 req/s, offload=70.8 req/s,
  offload_table=70.5 req/s), gunicorn per_request (baseline=71.7 req/s,
  offload=64.8 req/s, offload_table=65.1 req/s), uvicorn shared
  (baseline=69.1 req/s, offload=70.0 req/s, offload_table=69.5 req/s), uvicorn
  per_request (baseline=70.1 req/s, offload=62.8 req/s, offload_table=62.6
  req/s). Tests: `cargo test -p molt-worker` (pass), `uv run --python 3.12
  python3 -m pytest tests/test_molt_accel_contracts.py tests/test_django_demo.py`
  (pass).
- 2026-01-13T02:41:38Z codex (git 05ff17b73ed99360af4081408e1c4da082500441):
  switched gunicorn bench runs to `gthread` with default threads=2, set default
  server workers to min(4, CPU cores), and defaulted k6 log level to `error` in
  `run_demo_bench.py`. Bench runs (K6_TARGET=100, K6_LOG_LEVEL=error, default
  worker counts): gunicorn shared (baseline=5830.2 req/s, offload=4076.9 req/s,
  offload_table=4420.8 req/s), gunicorn per_request (baseline=1948.5 req/s,
  offload=1541.7 req/s, offload_table=1724.1 req/s), uvicorn shared
  (baseline=4178.8 req/s, offload=2795.9 req/s, offload_table=3045.4 req/s),
  uvicorn per_request (baseline=1990.4 req/s, offload=1498.1 req/s,
  offload_table=1710.6 req/s). Tests: not run (bench-only).
- 2026-01-13T03:01:17Z codex (git 05ff17b73ed99360af4081408e1c4da082500441):
  ran longer steady-state benches (K6_STEADY=60s, K6_TARGET=100,
  K6_LOG_LEVEL=error) to stabilize p99/p999. Gunicorn shared (baseline=1943.9
  req/s, offload=1532.1 req/s, offload_table=1714.2 req/s), gunicorn per_request
  (baseline=2105.1 req/s, offload=1536.7 req/s, offload_table=1711.3 req/s),
  uvicorn shared (baseline=2014.2 req/s, offload=1535.2 req/s, offload_table=1707.0
  req/s), uvicorn per_request (baseline=2031.9 req/s, offload=1527.9 req/s,
  offload_table=1664.8 req/s). Tests: not run (bench-only).
- 2026-01-13T01:42:19Z codex: added abs/divmod builtins with numeric semantics and __abs__ fallback, wired WIT/wasm/builtins, added differential coverage, and updated STATUS/0014 matrix. Tests: not run (not requested).
- 2026-01-13T01:53:27Z codex (git 05ff17b73ed99360af4081408e1c4da082500441; dirty): implemented `ascii`/`bin`/`oct`/`hex` builtins with `__index__` fallback, wired builtins module + WIT/wasm imports/table entries, added differential coverage, and updated STATUS/0014 type matrix. Tests: not run (not requested).
- 2026-01-13T03:19:55Z codex (git 05ff17b73ed99360af4081408e1c4da082500441; dirty): tightened chr/ord + dict update error parity, added recursion guard helpers across native/wasm call paths (sys recursionlimit now uses runtime), updated wasm harness + WIT, and refreshed differential coverage. Tests: `tools/dev.py lint`, `tools/dev.py test`, `cargo clippy -- -D warnings`, `cargo test`, `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`, `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`.
- 2026-01-13T03:27:32Z codex (git 2ca92962c4289eda3495783be30ca7c5b388a666; dirty): fixed CI demo k6 failures by installing k6 in `.github/workflows/ci.yml` and `.github/workflows/perf_demo.yml`. Tests: not run (workflow-only change).
- 2026-01-13T03:56:12Z codex (git 0ff8a23160685b65d10e8d28c361d5deef0ed41b; dirty): added `sorted` builtin (runtime + wasm imports/WIT + frontend allowlist + builtins module) and expanded ordering comparisons for str/bytes/bytearray/list/tuple, updated min/max to use general ordering, implemented lambda lowering with closures/defaults/varargs/kw-only, added differential tests for sorted/lambda, and updated STATUS/0014/ROADMAP. Noted unexpected local modifications in `bench/scripts/run_demo_bench.py`, `docs/demo/DJANGO_OFFLOAD_QUICKSTART.md`, and `docs/spec/0914_BENCH_RUNNER_AND_RESULTS_FORMAT.md` that were not part of this change set.
- 2026-01-13T04:11:17Z codex (git 0ff8a23160685b65d10e8d28c361d5deef0ed41b; dirty):
  improved demo bench CPU/RSS capture by sampling listen PIDs via `lsof` and
  recording `process_context`, added gunicorn pid file handling in
  `bench/scripts/run_stack.sh`, and updated
  `docs/demo/DJANGO_OFFLOAD_QUICKSTART.md` +
  `docs/spec/0914_BENCH_RUNNER_AND_RESULTS_FORMAT.md`. Bench:
  `K6_STEADY=60s MOLT_SERVER=gunicorn bench/scripts/run_stack.sh` (baseline=2041.8
  req/s p99=67.4ms p999=72.2ms; offload=1514.6 req/s p99=98.9ms p999=107.7ms;
  offload_table=1768.6 req/s p99=45.4ms p999=48.2ms; errors 0%). Process
  metrics captured for server/worker in `bench/results/demo_k6_20260113T041023.json`.
- 2026-01-13T04:27:25Z codex (git 0ff8a23160685b65d10e8d28c361d5deef0ed41b; dirty):
  ran steady-state benches for per_request and uvicorn modes.
  Per-request (gunicorn): `K6_STEADY=60s MOLT_SERVER=gunicorn MOLT_ACCEL_CLIENT_MODE=per_request bench/scripts/run_stack.sh`
  (baseline=2019.9 req/s p99=69.4ms p999=79.8ms; offload=1543.8 req/s p99=93.0ms p999=99.3ms;
  offload_table=1766.4 req/s p99=45.0ms p999=47.8ms; errors 0%). Artifact:
  `bench/results/demo_k6_20260113T042238.json`.
  Uvicorn shared: `K6_STEADY=60s MOLT_SERVER=uvicorn MOLT_ACCEL_CLIENT_MODE=shared bench/scripts/run_stack.sh`
  (baseline=2073.3 req/s p99=67.6ms p999=81.2ms; offload=1553.1 req/s p99=91.5ms p999=95.8ms;
  offload_table=1781.3 req/s p99=44.8ms p999=47.9ms; errors 0%). Artifact:
  `bench/results/demo_k6_20260113T042647.json`.
- 2026-01-13T05:05:27Z codex (git 0ff8a23160685b65d10e8d28c361d5deef0ed41b; dirty):
  ran steady-state benches for uvicorn per_request and gunicorn K6_TARGET sweep.
  Uvicorn per_request: `K6_STEADY=60s MOLT_SERVER=uvicorn MOLT_ACCEL_CLIENT_MODE=per_request bench/scripts/run_stack.sh`
  (baseline=2035.7 req/s p99=68.2ms p999=82.4ms; offload=1541.7 req/s p99=91.7ms p999=95.3ms;
  offload_table=1783.6 req/s p99=44.6ms p999=47.5ms; errors 0%). Artifact:
  `bench/results/demo_k6_20260113T045205.json`.
  Gunicorn shared sweep (K6_STEADY=60s):
  - K6_TARGET=50: baseline=2141.7 req/s p99=37.6ms p999=41.8ms; offload=1581.4 req/s p99=47.8ms p999=55.9ms;
    offload_table=1775.4 req/s p99=44.7ms p999=48.9ms. Artifact: `bench/results/demo_k6_20260113T045628.json`.
  - K6_TARGET=100: baseline=2038.4 req/s p99=69.4ms p999=73.8ms; offload=1544.0 req/s p99=92.8ms p999=110.2ms;
    offload_table=1732.2 req/s p99=84.8ms p999=101.1ms. Artifact: `bench/results/demo_k6_20260113T050033.json`.
  - K6_TARGET=200: baseline=1904.3 req/s p99=127.5ms p999=139.1ms; offload=1493.3 req/s p99=162.1ms p999=175.7ms;
    offload_table=1650.1 req/s p99=149.0ms p999=184.5ms. Artifact: `bench/results/demo_k6_20260113T050438.json`.
- 2026-01-13T05:22:41Z codex (git 0ff8a23160685b65d10e8d28c361d5deef0ed41b; dirty):
  fixed boxed-local function calls to apply defaults via guarded direct calls;
  added explicit unsupported errors for list/set/dict/generator comprehensions;
  adjusted list.sort differential to avoid list comprehension; updated README
  perf summary and STATUS limitations. Ran `tools/dev.py test`, `tools/dev.py lint`,
  `cargo test`, `cargo clippy -- -D warnings`, `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`,
  and `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`.
- 2026-01-13T05:37:58Z codex (git 519c2e510db7126ccb92c3205a1f5f90845c05eb; dirty):
  ran uvicorn per_request steady-state sweep (K6_STEADY=60s).
  - K6_TARGET=50: baseline=2094.8 req/s p99=39.5ms p999=43.9ms; offload=1574.0 req/s p99=48.4ms p999=52.2ms;
    offload_table=1760.8 req/s p99=45.3ms p999=48.2ms. Artifact: `bench/results/demo_k6_20260113T052829.json`.
  - K6_TARGET=100: baseline=1972.6 req/s p99=72.7ms p999=90.7ms; offload=1616.1 req/s p99=88.3ms p999=95.0ms;
    offload_table=1832.9 req/s p99=79.3ms p999=83.9ms. Artifact: `bench/results/demo_k6_20260113T053233.json`.
  - K6_TARGET=200: baseline=2001.1 req/s p99=119.6ms p999=122.9ms; offload=1569.6 req/s p99=154.0ms p999=183.5ms;
    offload_table=1752.3 req/s p99=138.8ms p999=144.0ms. Artifact: `bench/results/demo_k6_20260113T053637.json`.
- 2026-01-13T06:02:44Z codex (git 519c2e510db7126ccb92c3205a1f5f90845c05eb; dirty): generated sweep summary  comparing gunicorn shared vs uvicorn per_request K6_TARGET=50/100/200 (K6_STEADY=60s). No doc updates needed; no tests run.
- 2026-01-13T06:03:01Z codex (git 519c2e510db7126ccb92c3205a1f5f90845c05eb; dirty): generated sweep summary `bench/results/demo_k6_sweep_20260113.md` comparing gunicorn shared vs uvicorn per_request K6_TARGET=50/100/200 (K6_STEADY=60s). No doc updates needed; no tests run.
- 2026-01-13T06:07:42Z codex: starting proceed workflow for Django accel/offload demos; will inspect demo/molt_accel/molt-worker/bench scaffolding and update plan/spec/docs/tests as needed.
- 2026-01-13T06:21:31Z codex: added cancel-aware DB pool acquisition and worker integration, fake DB decode-cost env var parity in Django baseline, enriched demo bench metadata/payload bytes, updated specs/roadmap/quickstart. Tests: not run.
- 2026-01-13T06:27:27Z codex: ran tests/bench after Django offload updates. Tests: cargo test -p molt-db; cargo test -p molt-worker; uv run --python 3.12 python3 -m pytest tests/test_django_demo.py. Bench: bench/scripts/run_stack.sh (baseline=2042.6 req/s, offload=1532.1 req/s, offload_table=1808.6 req/s, errors 0%). Artifacts: bench/results/demo_k6_20260113T062712.json, bench/results/demo_k6_20260113T062712.md.
- 2026-01-13T06:36:13Z codex: starting optimization+functionality push for molt_accel/molt_offload concurrency and throughput; will focus on src/molt_accel, demo bench, and relevant specs.
- 2026-01-13T07:01:54Z codex (git 519c2e510db7126ccb92c3205a1f5f90845c05eb; dirty): fixed generator resume SSA by reloading index in async index loops, added lambda positional-only differential coverage, refactored molt-worker request handling to reduce arg counts + clippy, tightened molt_not_implemented return, and removed unused exception binding in molt_accel client. Tests: `tools/dev.py lint`, `tools/dev.py test`, `cargo test`, `cargo clippy -- -D warnings`. Bench: `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`, `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`.
- 2026-01-13T07:12:08Z codex: resuming proceed workflow to add molt_accel worker pooling + decorator env selection, worker env tuning for threads/queue, and update demo/spec/tests.
- 2026-01-13T07:18:32Z codex (git 3ade558198cb5da0dba2dac3b8393823e2eb4fa5; dirty): added MoltClientPool + shared pooling via MOLT_ACCEL_POOL_SIZE, worker env tuning (MOLT_WORKER_THREADS/MOLT_WORKER_MAX_QUEUE), updated bench metadata and demo/spec docs. Tests: `uv run --python 3.12 python3 -m pytest tests/test_molt_accel_client.py tests/test_molt_accel_decorator.py`, `cargo test -p molt-worker`. Bench: not run.
- 2026-01-13T07:23:36Z codex (git 3ade558198cb5da0dba2dac3b8393823e2eb4fa5; dirty): ran demo bench with pooling/tuning: `MOLT_ACCEL_POOL_SIZE=2 MOLT_WORKER_THREADS=8 bench/scripts/run_stack.sh`. Results: baseline=2051.4 req/s p99=67.9ms p999=71.7ms; offload=1568.9 req/s p99=93.4ms p999=114.0ms; offload_table=1821.6 req/s p99=43.2ms p999=45.3ms; errors 0%. Artifacts: `bench/results/demo_k6_20260113T072233.json`, `bench/results/demo_k6_20260113T072233.md`.
- 2026-01-13T07:59:02Z codex (git 3ade558198cb5da0dba2dac3b8393823e2eb4fa5; dirty): added microsecond worker metrics and bench aggregation, refactored `molt-db` into pool/sqlite modules, wired SQLite DB mode into molt-worker + Django baseline with seeding helper, and documented native-only DB support. Tests: `cargo test -p molt-db`, `cargo test -p molt-worker`, `uv run --python 3.12 python3 -m pytest tests/test_molt_accel_client.py tests/test_molt_accel_decorator.py tests/test_django_demo.py`. Bench sweeps (`MOLT_WORKER_THREADS=8`): pool=1 baseline=2151.1 req/s p99=63.5ms p999=69.0ms; offload=1570.3 req/s p99=88.3ms p999=104.2ms; offload_table=1812.4 req/s p99=43.5ms p999=48.5ms (artifact `bench/results/demo_k6_20260113T075249.json`). pool=2 baseline=2078.4 req/s p99=66.3ms p999=72.9ms; offload=1582.3 req/s p99=88.5ms p999=104.8ms; offload_table=1796.7 req/s p99=43.6ms p999=48.4ms (artifact `bench/results/demo_k6_20260113T075512.json`). pool=4 baseline=2060.9 req/s p99=68.5ms p999=76.2ms; offload=1411.7 req/s p99=105.9ms p999=111.6ms; offload_table=1573.8 req/s p99=51.4ms p999=71.8ms (artifact `bench/results/demo_k6_20260113T075721.json`).
- 2026-01-13T07:17:23Z codex (git 3ade558198cb5da0dba2dac3b8393823e2eb4fa5; dirty): fixed CI rustfmt failure via `cargo fmt` on `runtime/molt-worker/src/main.rs`, pushed follow-up commit, and confirmed CI green for run 20947959819. Note: working tree now has uncommitted `molt_accel` + related test changes (MoltClientPool, decorator wiring) awaiting user direction.
- 2026-01-13T08:13:45Z codex (git 3ade558198cb5da0dba2dac3b8393823e2eb4fa5; dirty): implemented iterator parity cluster (iter(callable, sentinel), map/filter/zip/reversed) across runtime/frontend/wasm harness; added builtin iterator differential coverage; updated type/status matrices; fixed clippy in molt-db/molt-worker; ran full lint/test/bench. Tests: `tools/dev.py lint`, `tools/dev.py test`, `cargo test`, `cargo clippy -- -D warnings`. Bench: `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`, `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`.
- 2026-01-13T08:19:09Z codex (git 3ade558198cb5da0dba2dac3b8393823e2eb4fa5; dirty): added DB IPC contract spec `docs/spec/0915_MOLT_DB_IPC_CONTRACT.md`, scaffolded `molt_django_adapter` payload builder + tests, and updated README/ROADMAP/STATUS plus DB specs for cross-framework adapter + async Postgres priority. Added SQLite baseline test in `tests/test_django_demo.py`. Tests: `uv run --python 3.12 python3 -m pytest tests/test_molt_django_adapter_contracts.py tests/test_django_demo.py`. Bench: `MOLT_DEMO_DB_PATH=$(mktemp ...) MOLT_WORKER_THREADS=8 bench/scripts/run_stack.sh` (baseline=2033.9 req/s p99=68.3ms p999=73.5ms; offload=1577.6 req/s p99=91.1ms p999=101.0ms; offload_table=1789.9 req/s p99=43.6ms p999=45.9ms; errors 0%). Artifacts: `bench/results/demo_k6_20260113T081809.json`, `bench/results/demo_k6_20260113T081809.md`.
- 2026-01-13T08:35:25Z codex (git 3ade558198cb5da0dba2dac3b8393823e2eb4fa5; dirty): renamed `molt_django_adapter` to `molt_db_adapter`, updated docs/README/STATUS/ROADMAP/0700/0701/0702/0915 and tests, and added a feature-gated async pool primitive with a built-in cancellation token in `runtime/molt-db/src/async_pool.rs` (tokio-backed). Tests: `cargo test -p molt-db --features async`, `uv run --python 3.12 python3 -m pytest tests/test_molt_db_adapter_contracts.py tests/test_django_demo.py`. Bench: not run.
- 2026-01-13T09:16:41Z codex (git 3ade558198cb5da0dba2dac3b8393823e2eb4fa5; dirty): fixed lambda default binding by routing dynamic calls through CALL_BIND and removed dead CALL_FUNC fallback; tightened bytes/bytearray + chr/ord parity in wasm harness/runtime; refactored molt-db Postgres TLS setup and async pool (Default CancelToken + no await-holding-lock); added tokio-postgres to molt-worker and silenced dead_code warnings for pending DB-query helpers; updated sqlparser Query wrapper for new fields. Tests: `tools/dev.py lint`, `tools/dev.py test`, `cargo test`, `cargo clippy -- -D warnings`. Bench: `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`, `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`.
- 2026-01-13T09:26:52Z codex (git f22551aa876c6e3eab7576db45a2523713f8a753; clean): ran `cargo fmt` to fix CI rustfmt failure, committed `style: cargo fmt`, pushed, and confirmed CI green (run 20951357217).
- 2026-01-13T09:48:50Z codex (git 6b56c62dd73429da27a98be1b3b967b95a649fad; dirty): implemented async Postgres connector + statement cache in `molt-db`, added sync/async runtime selection and `db_query` handling in `molt-worker` (SQLite + Postgres cancel-aware), added db_query unit tests, wired default exports, and updated README/ROADMAP/STATUS + DB/offload specs/quickstart. Tests: `cargo test -p molt-worker`.
- 2026-01-13T10:22:11Z codex (git 6b56c62dd73429da27a98be1b3b967b95a649fad; dirty): added Arrow IPC result encoding for db_query in molt-worker with type inference and db-specific metrics (db_alias/tag/row_count/bytes/result_format), expanded Postgres decoding to uuid/json/date/time string outputs, and enabled arrow_ipc passthrough in molt_accel codecs. Updated DB specs/STATUS/ROADMAP/metrics slide and added Arrow IPC + codec tests. Tests: `cargo test -p molt-worker`, `uv run --python 3.12 python3 -m pytest tests/test_molt_accel_client.py`.
- 2026-01-13T10:54:12Z codex (git 6b56c62dd73429da27a98be1b3b967b95a649fad; dirty): added in-place augmented assignment semantics for list/bytearray/set (name/attribute/subscript targets), tightened set in-place operators to require set-like RHS with CPython error strings, and standardized iterable TypeError messages; updated wasm harness to mirror iterable/in-place errors; added `tests/differential/basic/augassign_inplace.py`; refreshed type/status docs; fixed molt-worker clippy nits. Tests: `uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/augassign_inplace.py`, `uv run --python 3.12 python3 tools/dev.py lint`, `uv run --python 3.12 python3 tools/dev.py test`, `cargo test`, `cargo clippy -- -D warnings`, `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`, `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`.
- 2026-01-13T15:08:00Z codex (git c0c3950737f54457842738232b015497d0c89125; dirty): added structured Postgres array/range/interval decoding in `molt-worker` with Arrow IPC rejection for those types; added pg decode unit tests; added wasm guardrails in `molt-db` (target-specific deps + compile-time errors for sqlite/postgres on wasm); updated docs/spec/0700/0701/0915/STATUS, ROADMAP, and README. Tests: `cargo test -p molt-worker`. Bench: not run.
- 2026-01-13T15:22:43Z codex (git c0c3950737f54457842738232b015497d0c89125; dirty): implemented Postgres multirange decoding and array lower-bound preservation in `molt-worker`, refactored range parsing helper, and updated docs/spec/0700/0701/0915/STATUS plus ROADMAP/README to reflect multirange support and remaining Arrow IPC gaps. Tests: `cargo test -p molt-worker`. Bench: not run.
- 2026-01-13T15:52:19Z codex (git 0e079e6d2c617dcfee4afe59b798cbaf60debe03; dirty): added Arrow IPC complex-type support (arrays/ranges/intervals/multiranges) with struct/list encoding and a new unit test, hardened async Postgres prepare/decoding cancellation/timeout handling, and updated docs/spec/0700/0701/0915. Tests: `cargo test -p molt-worker`. Bench: not run.
- 2026-01-13T16:00:33Z codex (git 0e079e6d2c617dcfee4afe59b798cbaf60debe03; dirty): updated STATUS/ROADMAP/README and DB specs for Arrow IPC complex-type support, and added wasm DB parity TODOs in 0701/ROADMAP. Tests: not run (docs-only updates).
- 2026-01-13T16:09:23Z codex: starting proceed step 1 (wasm DB parity): define WIT DB host interface, add wasm shims/capability gates, update specs/STATUS/ROADMAP as needed; inspect `wit/molt-runtime.wit`, `runtime/molt-backend/src/wasm.rs`, `runtime/molt-runtime/src/lib.rs`, and `runtime/molt-db`.
- 2026-01-13T16:29:00Z codex: added WIT DB host intrinsics (`db_query`/`db_exec`), runtime capability gating + host hook shims, wasm harness/run_wasm DB stubs, and updated DB specs/STATUS/ROADMAP/README for wasm DB host interface. Tests: not run.
- 2026-01-13T15:36:31Z codex (git c0c3950737f54457842738232b015497d0c89125; dirty): added `__index__` parity for indexing/slicing with CPython overflow messages, extended set algebra/in-place ops to accept dict_keys/items, updated wasm harness dict views + index handling, and added differential coverage (`index_dunder.py`, `set_view_ops.py`, updated `augassign_inplace.py`). Updated STATUS/type coverage matrix and regenerated native/wasm bench JSON. Tests: `tools/dev.py lint`, `tools/dev.py test`, `cargo test`, `cargo clippy -- -D warnings` (fails in `runtime/molt-worker/src/main.rs` while-let loop). Bench: `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`, `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`.
- 2026-01-13T15:41:02Z codex (git 0e079e6b1d571bd6a5d21d2b889ea288c2e02240; dirty): documented async/IO/DB/WASM parity cluster plan in `ROADMAP.md` + STATUS, preparing to execute step 1 (async runtime core).
- 2026-01-13T15:42:17Z codex: starting slice assignment/deletion parity for list/bytearray/memoryview (runtime+wasm), __index__ handling, and differential+status updates.
- 2026-01-13T16:10:01Z codex: implemented list/bytearray slice assignment+deletion and memoryview delete errors in runtime, added del_index op wiring across frontend/backend/WIT, and extended wasm harness slice assignment handling + __index__ error parity; added differential coverage in `tests/differential/basic/slice_assign.py` and updated STATUS/0014 matrix. Tests: not run (not requested).
- 2026-01-13T16:15:18Z codex: extended wasm harness with minimal memoryview support (memoryview_new/tobytes, len, assignment path helpers) to keep slice assignment/deletion parity; updated CHECKPOINT summary. Tests: not run.
- 2026-01-13T16:41:41Z codex (git 0e079e6d2c617dcfee4afe59b798cbaf60debe03; dirty): implemented async Task/Future + contextvars shims, added await lowering to honor __await__ for stdlib awaitables, injected cancellation checks into molt_future_poll_fn, and added differential coverage for contextvars + task/future basics (gather stubbed). Tests: `uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/contextvars_basic.py`, `uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/asyncio_task_basic.py`, `uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/asyncio_future_basic.py`.
- 2026-01-13T16:44:28Z codex (git 0e079e6d2c617dcfee4afe59b798cbaf60debe03; dirty): adjusted await lowering to call __await__ only when present, removed asyncio.run ASYNC_BLOCK_ON special-case to allow loop tracking, restored create_task running-loop guard, and kept gather stubbed (removed gather differential). Tests: `uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/asyncio_shim.py`, `uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/asyncio_task_basic.py`, `uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/asyncio_future_basic.py`.
- 2026-01-13T16:51:28Z codex: implemented asyncio gather + fixed Future.__await__/exception base classes, added differential gather coverage, updated STATUS + stdlib matrix. Tests: not run (not requested).
- 2026-01-13T16:58:45Z codex: scope locked for wasm DB host adapter (run_wasm.js) + db_exec implementation in molt-worker (SQLite/Postgres) + molt_db_adapter exec payload helper, with spec/STATUS/ROADMAP/README updates and checkpoint refresh.
- 2026-01-13T16:59:08Z codex: updated AGENTS.md and GEMINI.md to require explicit AGENT_LOCKS writes before non-doc code/test work, clarify doc-sharing norms, and warn about lower-level vs higher-level overlap risks. Tests: not run.
- 2026-01-13T17:01:12Z codex: starting wasm slice read parity cluster (SLICE op for list/tuple/bytes/bytearray/str/memoryview) scoped to `tests/wasm_harness.py` and `run_wasm.js` after updating AGENT_LOCKS.
- 2026-01-13T17:06:16Z codex: pivoting to free-variable capture parity; will inspect compiler lowering and runtime closure/cell handling (frontend/runtime/backend) and add differential coverage as needed.
- 2026-01-13T17:10:12Z codex: implemented async free-var boxing by treating async locals as outer scope, loading/storing boxed cells via closure slots; restored async free-var test in `tests/differential/basic/asyncio_event_wait_for.py` and added `tests/differential/basic/free_vars_basic.py`. Tests: not run.
- 2026-01-13T17:18:23Z codex: cleared stale AGENT_LOCKS entries per new unique-lock policy; no active locks remain.
- 2026-01-13T17:20:35Z codex (git 0e079e6b1d571bd6a5d21d2b889ea288c2e02240; dirty): async core work: added cancellation check in `molt_anext_default_poll`, added `asyncio.Event` + simplified `gather`, restored `Future.__await__` to return coroutine; `asyncio.wait_for` left as NotImplemented due to backend verifier/exception stack issues (documented in STATUS/ROADMAP). Added `tests/differential/basic/asyncio_event_basic.py`; removed wait_for test attempt. Ran molt_diff for `asyncio_event_basic.py`, `asyncio_task_basic.py`, `asyncio_future_basic.py` (pass).
- 2026-01-13T17:21:25Z codex: implemented `db_exec` in molt-worker (SQLite/Postgres) with allow_write gating + metrics and added a SQLite roundtrip test; wired Node/WASI DB host adapter in `run_wasm.js` (stdio -> molt-worker, msgpack header streaming, cancel polling) plus `molt_header_size` export; added `build_db_exec_payload`, updated manifests/stubs/tests, and refreshed 0700/0701/0911/0915/STATUS/ROADMAP/README. Tests: not run (not requested).
- 2026-01-13T17:23:13Z codex-4698 (git 0e079e6b1d571bd6a5d21d2b889ea288c2e02240; dirty): starting async core follow-up: investigate safe asyncio.wait_for/timeouts + cancellation-aware Event cleanup; scope runtime cancel/await plumbing + asyncio shim + differential tests.
- 2026-01-13T17:44:49Z codex-4698 (git 0e079e6d2c617dcfee4afe59b798cbaf60debe03; dirty): implemented `asyncio.wait_for` timeouts via polling + timeout task (no cancellation of timed-out tasks yet); added differential coverage in `tests/differential/basic/asyncio_wait_for_basic.py`; updated `docs/spec/STATUS.md`, `docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md`, and `ROADMAP.md` to reflect wait_for support + cancellation gap. Tests: `uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/asyncio_wait_for_basic.py`.
- 2026-01-13T17:45:52Z codex-34257 (git 0e079e6d2c617dcfee4afe59b798cbaf60debe03; dirty): starting async follow-up to add cancellation-aware `asyncio.wait_for` timeouts + Event waiter cleanup in runtime/stdlib/tests.
- 2026-01-13T18:22:34Z codex-34257 (git 0e079e6d2c617dcfee4afe59b798cbaf60debe03; dirty): added Event waiter cleanup via task teardown map in `src/molt/stdlib/asyncio.py`; attempted wait_for cancellation but async exception propagation still aborts (wait_for restored to NotImplemented, docs updated). Runtime: broadened `molt_raise` to coerce non-exception objects; added per-task exception stack storage in scheduler/block_on and a cancellation underflow guard. Docs: STATUS/0015/ROADMAP updated to mark wait_for pending. Tests: `uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/asyncio_event_basic.py`, `uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/asyncio_task_basic.py`.
- 2026-01-13T17:21:54Z codex: starting scoping parity work (nonlocal declarations + closure capture across nested/class scopes) scoped to `src/molt/frontend/__init__.py` and docs/status/matrix; tests deferred due to existing lock on `tests/differential/basic/`.
- 2026-01-13T17:33:36Z codex-13713: starting Django accel/offload demo alignment; will unify payload builders (list_items/compute/offload_table), add JSON-body parsing for offload_table, enrich molt_accel metrics/cancel-checks, update demo/spec docs, and extend tests for contracts/metrics/demo parity.
- 2026-01-13T17:29:33Z codex: implemented nonlocal + closure store handling and outer-scope detection (async/generator + class methods), added scoping differential test `tests/differential/scoping/nonlocal_and_class_closure.py`, updated STATUS/0014 matrix. Tests: not run.
- 2026-01-13T17:37:58Z codex: cleared tests/differential/basic/ from codex-4698 lock, moved scoping test into basic, and released temporary lock.
- 2026-01-13T17:39:59Z codex-13713: unified demo payload builders in `molt_accel.contracts` (lenient list_items limit default, compute/offload_table JSON body parsing) and wired Django demo views; added auto cancel-check detection and payload/response byte metrics in molt_accel; aligned offload_table fallback response to `sample`; updated tests and docs (0912 spec + quickstart). Tests: `uv run --python 3.12 python3 -m pytest tests/test_molt_accel_contracts.py tests/test_molt_accel_client.py tests/test_django_demo.py`.
- 2026-01-13T17:41:39Z codex-13713: updated `docs/spec/STATUS.md` to reflect molt_accel auto cancel-check detection, payload/response byte metrics, and shared demo payload builders. Tests/bench: not run (doc-only).
- 2026-01-13T17:42:55Z codex-13713: synced README/ROADMAP offload demo notes with STATUS (auto cancel-check detection, payload/response byte metrics, shared demo payload builders). Tests/bench: not run (doc-only).
- 2026-01-13T17:45:46Z codex-13713: ran `bench/scripts/run_stack.sh` (baseline 2035.6 req/s p99=67.9ms p999=71.9ms; offload 1532.9 req/s p99=92.9ms p999=98.2ms; offload_table 1693.0 req/s p99=47.2ms p999=69.4ms; errors 0%). Artifacts: `bench/results/demo_k6_20260113T174533.json`, `bench/results/demo_k6_20260113T174533.md`.
- 2026-01-13T17:47:09Z codex-96239: tightened AGENT_LOCKS guidance in `AGENTS.md` and `GEMINI.md` to require proactive lock release and to use Codex CLI parent PID for unique lock names; cleared temporary locks. Tests: not run.
- 2026-01-13T17:48:50Z codex-96239: fixed non-generator function scoping setup by populating nonlocal/assigned/scope_assigned before visiting body so inner free-var resolution sees outer assignments; released `src/molt/frontend/__init__.py` lock. Tests: not run (tests/differential/basic locked by another agent).
- 2026-01-13T17:58:26Z codex-13713: ran `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json` and `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`; updated README performance summary. WASM bench failures: bench_async_await, bench_channel_throughput (compile error: local.set expected i64, call_indirect returned i32).
- 2026-01-13T18:00:18Z codex-96239: reinforced mindset note in `AGENTS.md` and `GEMINI.md` emphasizing low-level core + high-level abstractions and embracing complexity. Tests: not run.
- 2026-01-13T18:01:23Z codex-96239: strengthened the project ethos line in `AGENTS.md` and `GEMINI.md` to emphasize aspirational, genius-level rigor with gritty execution. Tests: not run.
- 2026-01-13T18:02:19Z codex-96239: strengthened ethos line in `AGENTS.md`/`GEMINI.md` to emphasize proactive ownership of complex tasks. Tests: not run.
- 2026-01-13T18:03:02Z codex-96239: escalated ethos line in `AGENTS.md`/`GEMINI.md` to "seek the hardest problems first" and "own complexity end-to-end". Tests: not run.
- 2026-01-13T18:27:07Z codex-98895: starting full lint/test/fuzz/bench sweep; focus on async exception propagation + asyncio.wait_for parity; locked runtime/asyncio/tests scope.
- 2026-01-13T18:30:12Z codex-7757: starting benchmark run (native+wasm bench JSON) and drafting profiling/optimization plan; will update bench/results, OPTIMIZATIONS_PLAN.md if needed, and CHECKPOINT.md.
- 2026-01-13T18:44:12Z codex-7757 (git 0e079e6d2c617dcfee4afe59b798cbaf60debe03; dirty): ran `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json` and `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`. WASM async benches failed to compile due to `src/molt/stdlib/asyncio.py:48` base-class ordering compat error. Added OPT-0004 profiling/optimization plan in `OPTIMIZATIONS_PLAN.md`.
- 2026-01-13T18:46:23Z codex-7757: starting Phase 0 profiling harness work under `tools/`/`bench/` plus hotspot triage; blocked on `src/molt/stdlib/asyncio.py` (locked by codex-98895) for wasm async compat fix.
- 2026-01-13T18:51:54Z codex-7757: added `tools/profile.py` profiling harness (CPU/alloc + optional compiler cProfile) and documented it in `LOGGING_AND_BENCHMARK_CONVENTIONS.md`. No tests or benches run; wasm async fix still blocked by lock.
- 2026-01-13T20:18:45Z codex-98895 (git 0e079e6d2c617dcfee4afe59b798cbaf60debe03; dirty): starting async exception propagation fix by isolating exception stacks per await (state_transition + runtime), re-enabling asyncio.wait_for/gather coverage, then running full lint/test/fuzz/bench before commit+push+CI.
- 2026-01-13T21:51:10Z codex-98895 (git 0e079e6d2c617dcfee4afe59b798cbaf60debe03; dirty): refreshed async exception propagation by isolating task exception stacks/last exceptions, added GIL guard for non-wasm scheduler/block_on polling, routed future polling through per-task stack helper, and re-enabled asyncio.wait_for/gather coverage; fixed backend IfFrame terminal handling, function symbol caching for nested scopes, and DbNamedParam serialization; updated fuzz target signature; refreshed README performance summary after new native+wasm bench runs. Tests: `uv run --python 3.12 python3 tools/dev.py lint`, `uv run --python 3.12 python3 tools/dev.py test`, `cargo test`, `cargo clippy -- -D warnings`. Fuzz: `cargo +nightly fuzz run string_ops -- -runs=100000` (warnings: function_casts_as_integer). Bench: `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`, `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`.
