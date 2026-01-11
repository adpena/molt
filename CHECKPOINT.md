Checkpoint: 2026-01-10 21:32:22 CST
Git: 073b4e42c196067cc6db2ad1f6988baa1136e81f

Summary
- Added `molt_async_sleep_new` to allocate async sleep futures with the runtime poll function set.
- Native backend now routes `CALL_ASYNC` for `molt_async_sleep` through the new constructor, bypassing import
  `func_addr` and avoiding null poll_fn headers on Linux.
- Ran cargo fmt after CI failure; no functional changes beyond formatting.
- Adjusted call_async to use `first()` to satisfy clippy get_first in CI.

Files touched (uncommitted)
- CHECKPOINT.md

Tests run
- cargo test -p molt-runtime -p molt-backend
- cargo clippy -p molt-runtime -p molt-backend -- -D warnings

Known gaps
- Allowlisted module calls still reject keywords/star args; only Molt-defined callables accept CALL_BIND.
- async with multi-context and destructuring targets remain unsupported (see docs/spec/STATUS.md).
- BaseException hierarchy and typed matching remain partial (see docs/spec/0014_TYPE_COVERAGE_MATRIX.md).
- OPT-0007/0008 regressions still open (struct/descriptor/attr access).
- OPT-0009 partial: bench_str_split.py ~2x CPython, bench_str_join.py ~0.91x.
- Fuzz invocation needs a bounded run (e.g. max time) to be treated as a clean pass.
- bench_struct/bench_attr_access/bench_descriptor_property remain far below CPython; prioritize OPT-0007/0008 follow-through.
- Codon baseline skips asyncio/bytearray/memoryview/molt_buffer/molt_msgpack/struct-init benches.
