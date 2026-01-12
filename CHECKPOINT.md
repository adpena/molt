Checkpoint: 2026-01-11T22:13:08-0600
Git: 2b9cfdad616446bd5619b0a392c7429ada06c801 (dirty)

Summary
- Added `__molt_layout_size__` metadata on classes and wired class-object call paths in the runtime/wasm harness so `cls(...)` works via `__init__`.
- Implemented `is_function_obj` in the wasm harness to unblock `call_func` imports.
- Adjusted `functools.update_wrapper` to use `setattr` for `__wrapped__` to satisfy `ty`.

Files touched (uncommitted)
- CHECKPOINT.md
- docs/spec/0014_TYPE_COVERAGE_MATRIX.md
- docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md
- docs/spec/STATUS.md
- runtime/molt-backend/src/lib.rs
- runtime/molt-backend/src/wasm.rs
- runtime/molt-runtime/src/lib.rs
- src/molt/frontend/__init__.py
- src/molt/stdlib/__init__.py
- src/molt/type_facts.py
- tests/differential/basic/getattribute_basic.py
- tests/differential/basic/iter_methods.py
- tests/differential/basic/stdlib_allowlist_calls.py
- tests/wasm_harness.py
- wit/molt-runtime.wit
- logs/clif_fib.txt
- logs/clif_functools.txt
- logs/clif_lru.txt
- logs/clif_lru_factory.txt
- logs/clif_sum_list.txt
- logs/clif_wrapper.txt
- logs/ir_fib.txt
- src/molt/stdlib/functools.py
- src/molt/stdlib/itertools.py

Docs/spec updates needed?
- None.

Tests run
- `uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic`
- `uv run --python 3.12 python3 tools/dev.py lint`
- `uv run --python 3.12 python3 tools/dev.py test`

Known gaps
- Codon baseline skips remain for async/channel/matrix_math/bytearray/memoryview/parse_msgpack/struct/sum_list_hints benches.
- WASM remains slower than native on nested-loop/struct benches; async/channel binaries are still the largest (80-142 KB).
- Single-module WASM link + JS stub removal remains pending (see `docs/spec/STATUS.md`).
