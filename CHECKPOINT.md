Checkpoint: 2026-01-10 21:51:11 CST
Git: 9260c9c4794e1808203b3559c34a93f6e5808035

Summary
- Added `molt_future_new` to centralize future header initialization (poll_fn/state) with debug logging when
  `MOLT_DEBUG_AWAITABLE` is set.
- `molt_async_sleep_new` now reuses `molt_future_new`; backend `alloc_future`/`call_async` also use it to avoid
  direct header stores.
- Still investigating Linux async-for awaitable failures; this change should clarify whether poll_fn is zero at
  allocation time.

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
