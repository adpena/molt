Checkpoint: 2026-01-12T08:53:06-06:00
Git: 578c3ea9a5c40dda938a0208ce6a565fda538d98 (dirty)

Summary
- Fixed wasm header offsets for poll/state after MoltHeader growth; generator iter hang resolved.
- Marked object headers as pointer-containing in native stores and runtime field setters to keep ref scanning correct.
- Re-ran native + wasm benches and refreshed README performance numbers.

Files touched (uncommitted)
- CHECKPOINT.md
- OPTIMIZATIONS_PLAN.md
- README.md
- bench/results/bench.json
- bench/results/bench_wasm.json
- runtime/molt-backend/src/lib.rs
- runtime/molt-backend/src/wasm.rs
- runtime/molt-runtime/src/lib.rs
- src/molt/frontend/__init__.py
- src/molt_json/__init__.py
- tests/wasm_harness.py
- wit/molt-runtime.wit

Docs/spec updates needed?
- None.

Tests run
- `uv run --python 3.12 python3 tools/dev.py lint`
- `uv run --python 3.12 python3 tools/dev.py test`
- `PYTHONPATH=src uv run --python 3.14 python3 -m molt.cli build --target wasm /tmp/molt_gen_debug.py`
- `PYTHONPATH=src uv run --python 3.14 python3 -m molt.cli build --target wasm tests/benchmarks/bench_generator_iter.py`
- `node run_wasm.js`
- `uv run --python 3.14 python3 tools/bench_wasm.py --samples 1 --json-out bench/results/bench_wasm.json`
- `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`

Benchmarks
- `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`
- `uv run --python 3.14 python3 tools/bench_wasm.py --samples 1 --json-out bench/results/bench_wasm.json`

Known gaps
- Codon baseline skips remain for async/channel/matrix_math/bytearray/memoryview/parse_msgpack/struct/sum_list_hints benches.
- Single-module WASM link + JS stub removal remains pending (see `docs/spec/STATUS.md`).
