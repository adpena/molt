Checkpoint: 2026-01-12T00:02:59-0600
Git: 417041656f8e0a5f1e0484bf9ea5e8e6949829c6 (dirty)

Summary
- Offset WASM table indices to avoid runtime table collisions; enable import/growable table for runtime wasm builds.
- Drop unused `callargs_push_pos` results in WASM `call_func` fallback paths to fix validation stack mismatches.
- WASM benches now run cleanly (descriptor property + async/channel).

Files touched (uncommitted)
- CHECKPOINT.md
- bench/results/bench.json
- bench/results/bench_wasm.json
- docs/spec/0014_TYPE_COVERAGE_MATRIX.md
- docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md
- docs/spec/STATUS.md
- runtime/molt-backend/src/lib.rs
- runtime/molt-backend/src/wasm.rs
- src/molt/stdlib/__init__.py
- src/molt/type_facts.py
- tests/differential/basic/getattribute_basic.py
- tests/differential/basic/iter_methods.py
- tests/differential/basic/stdlib_allowlist_calls.py
- tools/bench_wasm.py
- wit/molt-runtime.wit
- logs/clif_fib.txt
- logs/clif_functools.txt
- logs/clif_lru.txt
- logs/clif_lru_factory.txt
- logs/clif_sum_list.txt
- logs/clif_wrapper.txt
- logs/ir_fib.txt
- src/molt/stdlib/itertools.py

Docs/spec updates needed?
- None.

Tests run
- `uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic`
- `uv run --python 3.12 python3 tools/dev.py lint`
- `uv run --python 3.12 python3 tools/dev.py test`
- `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`
- `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`

Known gaps
- Codon baseline skips remain for async/channel/matrix_math/bytearray/memoryview/parse_msgpack/struct/sum_list_hints benches.
- Single-module WASM link + JS stub removal remains pending (see `docs/spec/STATUS.md`).
