Checkpoint: 2026-01-13T06:28:01Z
Git: 519c2e510db7126ccb92c3205a1f5f90845c05eb (dirty)

Summary
- Added cancel-aware DB pool acquisition and wired it into molt-worker request handling, mapping pool waits to Cancelled/Timeout.
- Added fake DB decode-cost simulation via MOLT_FAKE_DB_DECODE_US_PER_ROW with baseline parity in Django views.
- Extended demo bench artifacts with machine/tool metadata and payload size metrics; updated demo/spec docs and roadmap.

Files touched (uncommitted)
- CHECKPOINT.md
- ROADMAP.md
- bench/scripts/run_demo_bench.py
- demo/django_app/demoapp/views.py
- docs/AGENT_MEMORY.md
- docs/demo/DJANGO_OFFLOAD_QUICKSTART.md
- docs/spec/0911_MOLT_WORKER_V0_SPEC.md
- docs/spec/0913_DEMO_DJANGO_ENDPOINT_CONTRACT.md
- docs/spec/0914_BENCH_RUNNER_AND_RESULTS_FORMAT.md
- docs/spec/STATUS.md
- runtime/molt-db/src/lib.rs
- runtime/molt-worker/src/main.rs
- Other pre-existing local modifications not listed here (see git status).

Docs/spec updates needed?
- Updated docs/spec/0911_MOLT_WORKER_V0_SPEC.md, docs/spec/0913_DEMO_DJANGO_ENDPOINT_CONTRACT.md, docs/spec/0914_BENCH_RUNNER_AND_RESULTS_FORMAT.md, docs/spec/STATUS.md, and ROADMAP.md for cancellation + bench metadata.

Tests run
- cargo test -p molt-db
- cargo test -p molt-worker
- uv run --python 3.12 python3 -m pytest tests/test_django_demo.py

Benchmarks
- bench/scripts/run_stack.sh
  - baseline: 2042.6 req/s, p50=44.0ms p95=65.4ms p99=70.5ms p999=82.7ms, errors=0.00%
  - offload: 1532.1 req/s, p50=59.5ms p95=86.2ms p99=93.9ms p999=111.1ms, errors=0.00%
  - offload_table: 1808.6 req/s, p50=25.2ms p95=29.6ms p99=43.5ms p999=46.2ms, errors=0.00%
  - artifacts: bench/results/demo_k6_20260113T062712.json, bench/results/demo_k6_20260113T062712.md

Known gaps
- Cancellation propagation into real DB tasks still pending.
- Codon baseline skips remain for async/channel/matrix_math/bytearray/memoryview/parse_msgpack/struct/sum_list_hints benches.
- Single-module WASM link + JS stub removal remains pending (see docs/spec/STATUS.md).

CI
- Not run (local changes only).
