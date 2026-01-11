Checkpoint: 2026-01-11 00:00:55 CST
Git: b6fad8b9fc8d713bb1b8d99b5f6a562f241596dc

Summary
- Added profiling counters for attr lookup + layout guard hits/fails, exposed in profile dumps.
- Frontend now records default arg metadata for symbol-known callables and reuses it during call lowering,
  plus propagates function hints through assignments.
- Added guard-assumption loop split for getattr/setattr and exact-class init fast paths to reduce per-iteration
  guard branching.
- Backend adds int-tag fast paths for ADD/SUB/MUL/LT/EQ before runtime fallback.
- Updated bench harness Codon invocation for macOS x86_64 and refreshed README perf summary after native/WASM runs.

Files touched (uncommitted)
- README.md
- bench/results/bench.json
- runtime/molt-backend/src/lib.rs
- runtime/molt-runtime/src/lib.rs
- src/molt/frontend/__init__.py
- tools/bench.py

Docs/spec updates needed?
- None for this change set (perf counters, lowering tweaks, bench docs only).

Tests run
- uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic
- uv run --python 3.12 python3 tools/dev.py lint
- uv run --python 3.12 python3 tools/dev.py test
- uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json
- uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json

Known gaps
- Layout guard overhead remains high; bench_struct/bench_attr_access/bench_fib still below CPython.
- WASM bench timings unavailable (molt_wasm_ok false); sizes only.
- Codon baseline skips remain for async/channel/matrix_math/bytearray/memoryview/parse_msgpack/struct benches.
