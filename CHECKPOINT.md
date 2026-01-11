Checkpoint: 2026-01-10 21:28:55 CST
Git: f4e38c1fc33ed4687d2f5a7850bff129d410fc8b

Summary
- Added `molt_async_sleep_new` to allocate async sleep futures with the runtime poll function set.
- Native backend now routes `CALL_ASYNC` for `molt_async_sleep` through the new constructor, bypassing import
  `func_addr` and avoiding null poll_fn headers on Linux.
- Rebuilt molt-backend/molt-runtime tests; no spec changes in this slice.

Files touched (uncommitted)
- CHECKPOINT.md

Tests run
- cargo test -p molt-runtime -p molt-backend

Known gaps
- Allowlisted module calls still reject keywords/star args; only Molt-defined callables accept CALL_BIND.
- async with multi-context and destructuring targets remain unsupported (see docs/spec/STATUS.md).
- BaseException hierarchy and typed matching remain partial (see docs/spec/0014_TYPE_COVERAGE_MATRIX.md).
- OPT-0007/0008 regressions still open (struct/descriptor/attr access).
- OPT-0009 partial: bench_str_split.py ~2x CPython, bench_str_join.py ~0.91x.
- Fuzz invocation needs a bounded run (e.g. max time) to be treated as a clean pass.
- bench_struct/bench_attr_access/bench_descriptor_property remain far below CPython; prioritize OPT-0007/0008 follow-through.
- Codon baseline skips asyncio/bytearray/memoryview/molt_buffer/molt_msgpack/struct-init benches.
