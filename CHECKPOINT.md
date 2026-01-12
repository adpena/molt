Checkpoint: 2026-01-12T01:57:14-0600
Git: 361bc13bcf2f8a101c01f3d6fbad3454c917eeb5 (dirty)

Summary
- Added async free-var closure capture (stored in future payload) and async decorator diff coverage.
- Routed closure-backed CALL_FUNC paths through call_bind via new `molt_function_closure_bits` (native + WASM).
- Updated wasm harness to support `func_new_closure` + `function_closure_bits` and to pass closure args.
- Updated README/STATUS for async closures and ASGI shim usage.

Files touched (uncommitted)
- CHECKPOINT.md
- README.md
- docs/spec/STATUS.md
- runtime/molt-backend/src/lib.rs
- runtime/molt-backend/src/wasm.rs
- runtime/molt-runtime/src/lib.rs
- src/molt/asgi.py
- src/molt/frontend/__init__.py
- tests/differential/basic/async_closure_decorators.py
- tests/differential/basic/django_calling_conventions.py
- tests/wasm_harness.py
- wit/molt-runtime.wit

Docs/spec updates needed?
- None.

Tests run
- `uv run --python 3.12 python3 tools/dev.py lint`
- `uv run --python 3.12 python3 tools/dev.py test`

Benchmarks
- Not run in this session.

Known gaps
- Codon baseline skips remain for async/channel/matrix_math/bytearray/memoryview/parse_msgpack/struct/sum_list_hints benches.
- Single-module WASM link + JS stub removal remains pending (see `docs/spec/STATUS.md`).
