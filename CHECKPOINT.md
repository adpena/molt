Checkpoint: 2026-01-12T11:52:21-06:00
Git: 03ce5c067746463a332358c32f529dcc2739a3b1 (dirty)

Summary
- Added BaseException root + exception trace capture (function-name tuples) and non-string exception messages via str() lowering/runtime conversion.
- Extended wasm harness imports for new dict helpers (clear/copy/popitem/update_kwstar) to restore WASM parity tests.
- Hardened collections.Counter mapping detection for Molt shims and fixed typing-only casts.

Files touched (uncommitted)
- CHECKPOINT.md
- ROADMAP.md
- docs/spec/0014_TYPE_COVERAGE_MATRIX.md
- docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md
- docs/spec/STATUS.md
- runtime/molt-backend/src/lib.rs
- runtime/molt-backend/src/wasm.rs
- runtime/molt-runtime/src/lib.rs
- src/molt/frontend/__init__.py
- src/molt/stdlib/__init__.py
- src/molt/stdlib/collections/__init__.py
- src/molt/stdlib/operator.py
- tests/differential/basic/collections_basic.py
- tests/differential/basic/exception_message.py
- tests/differential/basic/list_dict.py
- tests/differential/basic/mro_inconsistent.py
- tests/differential/basic/stdlib_allowlist_calls.py
- tests/wasm_harness.py
- wit/molt-runtime.wit

Docs/spec updates needed?
- None.

Tests run
- `uv run --python 3.12 python3 tools/dev.py lint`
- `uv run --python 3.12 python3 tools/dev.py test`

Benchmarks
- Not run (changes only).

Known gaps
- Codon baseline skips remain for async/channel/matrix_math/bytearray/memoryview/parse_msgpack/struct/sum_list_hints benches.
- Single-module WASM link + JS stub removal remains pending (see `docs/spec/STATUS.md`).

CI
- Not run (local changes only).
