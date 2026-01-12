Checkpoint: 2026-01-12T02:33:10-0600
Git: ce0dc12b92695df1042dcab048a26e0b0c04605b (dirty)

Summary
- Fixed class layout version mismatch by setting runtime layout version during class construction.
- Added wasm/runtime plumbing for class_set_layout_version and updated wasm harness stubs.
- Re-ran native + wasm benches; bench_descriptor_property now 2.80x vs CPython.

Files touched (uncommitted)
- CHECKPOINT.md
- OPTIMIZATIONS_PLAN.md
- README.md
- ROADMAP.md
- bench/results/bench.json
- bench/results/bench_wasm.json
- runtime/molt-backend/src/lib.rs
- runtime/molt-backend/src/wasm.rs
- runtime/molt-runtime/src/lib.rs
- src/molt/frontend/__init__.py
- tests/wasm_harness.py
- wit/molt-runtime.wit

Docs/spec updates needed?
- None.

Tests run
- `uv run --python 3.12 python3 tools/dev.py lint`
- `uv run --python 3.12 python3 tools/dev.py test`
- `uv run --python 3.12 python3 tools/dev.py lint` (post-fix)

Benchmarks
- `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`
- `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`

Known gaps
- Codon baseline skips remain for async/channel/matrix_math/bytearray/memoryview/parse_msgpack/struct/sum_list_hints benches.
- Single-module WASM link + JS stub removal remains pending (see `docs/spec/STATUS.md`).
