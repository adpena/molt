Checkpoint: 2026-01-10 20:53:34 CST
Git: cb2f53a650d1360c67d44e2ebd75aa7b4c1fe4d2

Summary
- Async for awaitable handling now reuses `_emit_await_value` to align with the standard await path and avoid bespoke state-transition caching.
- Simplified async-for lowering by removing the custom awaitable slot/state transition block while retaining StopAsyncIteration handling.
- Local differential async-for slices now pass after the refactor.
- Docs/tests unchanged this turn; no additional updates needed.

Files touched (uncommitted)
- CHECKPOINT.md

Tests run
- uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/async_for_else.py
- uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/async_for_iter.py
- uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/async_long_running.py
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
