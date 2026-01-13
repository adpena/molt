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
