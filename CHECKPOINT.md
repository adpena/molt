Checkpoint: 2026-01-10 21:08:48 CST
Git: 41b3b7e5a2f0c6b7e2a3f39c2b77c62aa8a4d2a5

Summary
- Added awaitable debug logging behind `MOLT_DEBUG_AWAITABLE` to surface non-awaitable details during CI.
- Diff harness now enables `MOLT_DEBUG_AWAITABLE` for compiled binaries to surface async-for failures in CI logs.
- Re-ran native + wasm benches for the commit.
- Docs/tests unchanged beyond the debug toggle; no spec updates needed.

Files touched (uncommitted)
- CHECKPOINT.md

Tests run
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
