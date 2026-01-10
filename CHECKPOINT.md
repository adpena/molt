Checkpoint: 2026-01-10 09:04:12 CST
Git: b7be1285a7bfcd736d3ff3690c3dd3f39839751a

Summary
- Removed handle-table/raw-object registry; pointer tags now store canonical 48-bit pointers and `molt_handle_resolve` just unboxes.
- `molt_alloc` returns boxed object bits; native + wasm backends unbox for pointer-only ops (field access, guards, attrs, async poll/sleep).
- Updated runtime/ABI docs + C stubs to reflect boxed alloc + handle_resolve usage.
- Wasm backend: only synthesize `self_param`/`self` locals for `*_poll` functions to avoid clobbering arity-1 non-poll args (fixes `__aiter__` returning 0 in wasm).
- Native backend: `alloc_future` now inc-ref payload args to prevent nondeterministic async awaitable failures in long-running loops.
- Docs/tests unchanged this turn; no additional updates needed.

Files touched (uncommitted)
- CHECKPOINT.md
- OPTIMIZATIONS_PLAN.md
- bench/results/bench.json
- docs/spec/0003-runtime.md
- docs/spec/0020_RUNTIME_SAFETY_INVARIANTS.md
- docs/spec/0400_WASM_PORTABLE_ABI.md
- main_stub.c
- runtime/molt-backend/src/lib.rs
- runtime/molt-backend/src/wasm.rs
- runtime/molt-obj-model/src/handle_table.rs
- runtime/molt-obj-model/src/lib.rs
- runtime/molt-runtime/src/lib.rs
- src/molt/cli.py
- src/molt/frontend/__init__.py
- tests/wasm_harness.py

Tests run
- uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/async_long_running.py (x10)
- uv run --python 3.12 python3 tools/dev.py lint
- uv run --python 3.12 python3 tools/dev.py test
- uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json
- uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json

Known gaps
- Allowlisted module calls still reject keywords/star args; only Molt-defined callables accept CALL_BIND.
- async with multi-context and destructuring targets remain unsupported (see docs/spec/STATUS.md).
- BaseException hierarchy and typed matching remain partial (see docs/spec/0014_TYPE_COVERAGE_MATRIX.md).
- OPT-0007/0008 regressions still open (struct/descriptor/attr access).
- OPT-0009 partial: bench_str_split.py ~2x CPython, bench_str_join.py ~0.91x.
- Fuzz invocation needs a bounded run (e.g. max time) to be treated as a clean pass.
- bench_struct/bench_attr_access/bench_descriptor_property remain far below CPython; prioritize OPT-0007/0008 follow-through.
- Codon baseline skips asyncio/bytearray/memoryview/molt_buffer/molt_msgpack/struct-init benches.
