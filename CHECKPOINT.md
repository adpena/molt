Checkpoint: 2026-01-11T18:43:54-0600
Git: f70c52bd80932247a23947b34222e0017fe93176 (dirty)

Summary
- Added instance `__getattr__`/`__setattr__` hooks, `**kwargs` mapping support, and dict `setdefault`/`update` bound methods.
- `list.extend` now consumes generic iterables via the iter protocol; added differential coverage.
- Updated STATUS/type matrix and refreshed README performance summary with new native + WASM bench results.

Files touched (uncommitted)
- CHECKPOINT.md
- README.md
- bench/results/bench.json
- bench/results/bench_wasm.json
- docs/spec/0014_TYPE_COVERAGE_MATRIX.md
- docs/spec/STATUS.md
- logs/clif_fib.txt
- logs/clif_sum_list.txt
- logs/ir_fib.txt
- runtime/molt-backend/src/lib.rs
- runtime/molt-backend/src/wasm.rs
- runtime/molt-runtime/src/lib.rs
- tests/differential/basic/attr_hooks.py
- tests/differential/basic/container_methods.py
- tests/differential/basic/kwargs_mapping.py

Docs/spec updates needed?
- None (STATUS/type matrix/README updated).

Tests run
- `cargo fmt`
- `uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/container_methods.py`
- `uv run --python 3.12 python3 tools/dev.py lint`
- `uv run --python 3.12 python3 tools/dev.py test`
- `cargo test`
- `cargo clippy -- -D warnings`
- `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`
- `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json`

Known gaps
- Codon baseline skips remain for async/channel/matrix_math/bytearray/memoryview/parse_msgpack/struct/sum_list_hints benches.
- WASM remains slower than native on nested-loop/struct benches; async/channel binaries are still the largest (80-142 KB).
- Single-module WASM link + JS stub removal remains pending (see `docs/spec/STATUS.md`).
