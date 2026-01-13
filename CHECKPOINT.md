Checkpoint: 2026-01-13T00:23:17Z
Git: 05ff17b73ed99360af4081408e1c4da082500441 (dirty)

Summary
- Implemented bytes/bytearray constructor parity (int counts, iterable-of-ints, str+encoding) and added backend/runtime support.
- Fixed dict/dict.update sequence element error parity and tuple/dict arg-count TypeErrors.
- Added differential tests for constructor errors and updated STATUS/type coverage matrix; narrowed AGENT_LOCKS.

Files touched (uncommitted)
- .github/workflows/ci.yml
- AGENTS.md
- CHECKPOINT.md
- GEMINI.md
- README.md
- ROADMAP.md
- docs/AGENT_LOCKS.md
- docs/spec/0014_TYPE_COVERAGE_MATRIX.md
- docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md
- docs/spec/0016_ARGS_KWARGS.md
- docs/spec/0910_REPRO_BENCH_VERTICAL_SLICE.md
- docs/spec/0911_MOLT_WORKER_V0_SPEC.md
- docs/spec/0912_MOLT_ACCEL_DJANGO_DECORATOR_SPEC.md
- docs/spec/0913_DEMO_DJANGO_ENDPOINT_CONTRACT.md
- docs/spec/STATUS.md
- pyproject.toml
- runtime/molt-backend/src/lib.rs
- runtime/molt-backend/src/wasm.rs
- runtime/molt-runtime/src/lib.rs
- runtime/molt-worker/src/main.rs
- src/molt/cli.py
- src/molt/frontend/__init__.py
- src/molt/stdlib/collections/__init__.py
- src/molt/stdlib/functools.py
- src/molt_accel/client.py
- src/molt_accel/contracts.py
- src/molt_accel/decorator.py
- src/molt_accel/framing.py
- tests/differential/basic/collections_basic.py
- tests/fixtures/molt_worker_stub.py
- tests/molt_diff.py
- tests/test_molt_accel_client.py
- tests/test_molt_accel_contracts.py
- uv.lock
- wit/molt-runtime.wit
- .github/workflows/perf_demo.yml
- bench/k6/
- bench/scripts/
- demo/
- docs/AGENT_MEMORY.md
- docs/demo/DJANGO_OFFLOAD_QUICKSTART.md
- proceed.skill
- runtime/molt-worker/src/diagnostics.rs
- src/molt_accel/default_exports.json
- src/molt_accel/py.typed
- tests/differential/basic/args_kwargs_eval_order.py
- tests/differential/basic/async_hang_probe.py
- tests/differential/basic/bytes_constructors.py
- tests/differential/basic/dict_constructor_errors.py
- tests/differential/basic/functools_partial_lru.py
- tests/differential/basic/operator_getters.py
- tests/differential/basic/stdlib_class_kwargs.py
- tests/test_django_demo.py

Docs/spec updates needed?
- Updated `docs/spec/STATUS.md` and `docs/spec/0014_TYPE_COVERAGE_MATRIX.md` for constructor/error parity.

Tests run
- Not run (not run in this turn).

Benchmarks
- Not run (test-only).

Known gaps
- Codon baseline skips remain for async/channel/matrix_math/bytearray/memoryview/parse_msgpack/struct/sum_list_hints benches.
- Single-module WASM link + JS stub removal remains pending (see `docs/spec/STATUS.md`).

CI
- Not run (local changes only).
