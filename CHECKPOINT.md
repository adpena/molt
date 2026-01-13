Checkpoint: 2026-01-13T08:35:25Z
Git: 3ade558198cb5da0dba2dac3b8393823e2eb4fa5 (dirty)

Summary
- Renamed `molt_django_adapter` to `molt_db_adapter` and updated docs/tests to reflect framework-agnostic DB adapter naming.
- Added a feature-gated async pool primitive with cancellation token support in `molt-db` and documented it in DB specs/ROADMAP/STATUS.
- Added `tokio` dependency for the async pool and validated with `cargo test -p molt-db --features async`.

Files touched (uncommitted)
- CHECKPOINT.md
- Cargo.lock
- README.md
- ROADMAP.md
- bench/scripts/run_demo_bench.py
- bench/scripts/run_stack.sh
- demo/django_app/demoapp/db_seed.py
- demo/django_app/demoapp/views.py
- docs/AGENT_MEMORY.md
- docs/demo/DJANGO_OFFLOAD_QUICKSTART.md
- docs/spec/0700_MOLT_DB_LAYER_VISION.md
- docs/spec/0701_ASYNC_PG_POOL_AND_PROTOCOL.md
- docs/spec/0702_QUERY_BUILDER_AND_DJANGO_ADAPTER.md
- docs/spec/0910_REPRO_BENCH_VERTICAL_SLICE.md
- docs/spec/0911_MOLT_WORKER_V0_SPEC.md
- docs/spec/0913_DEMO_DJANGO_ENDPOINT_CONTRACT.md
- docs/spec/0914_BENCH_RUNNER_AND_RESULTS_FORMAT.md
- docs/spec/0915_MOLT_DB_IPC_CONTRACT.md
- docs/spec/STATUS.md
- references/docs.md
- runtime/molt-db/Cargo.toml
- runtime/molt-db/src/lib.rs
- runtime/molt-db/src/async_pool.rs
- runtime/molt-db/src/pool.rs
- runtime/molt-db/src/sqlite.rs
- runtime/molt-worker/Cargo.toml
- runtime/molt-worker/src/main.rs
- src/molt_db_adapter/__init__.py
- src/molt_db_adapter/contracts.py
- Other pre-existing local modifications not listed here (see git status).

Docs/spec updates needed?
- Updated docs/spec/0700_MOLT_DB_LAYER_VISION.md, docs/spec/0701_ASYNC_PG_POOL_AND_PROTOCOL.md, docs/spec/0702_QUERY_BUILDER_AND_DJANGO_ADAPTER.md, docs/spec/0915_MOLT_DB_IPC_CONTRACT.md, docs/spec/STATUS.md, README.md, and ROADMAP.md for adapter rename + async pool primitive.

Tests run
- cargo test -p molt-db --features async
- uv run --python 3.12 python3 -m pytest tests/test_molt_db_adapter_contracts.py tests/test_django_demo.py

Benchmarks
- None this turn.

Known gaps
- Cancellation propagation into real DB tasks still pending.
- SQLite connector support is native-only; wasm parity pending.
- DB IPC contract is defined, but worker-side `db_query` entrypoint is not implemented yet.
- Postgres driver integration and async worker runtime wiring remain pending; avoid blocking worker threads on DB I/O.
- Codon baseline skips remain for async/channel/matrix_math/bytearray/memoryview/parse_msgpack/struct/sum_list_hints benches.
- Single-module WASM link + JS stub removal remains pending (see docs/spec/STATUS.md).
- Offload demo throughput still trails baseline; need to iterate on IPC + serialization + DB mode perf.

CI
- Not run (local changes only).
