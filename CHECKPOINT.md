Checkpoint: 2026-01-10 22:38:35 CST
Git: db1737e41048b87ef5b166cd356d3ac5954cedef

Summary
- Future allocation now uses a zeroed, direct header init in `molt_future_new` to avoid pointer round-trips.
- `molt_aiter`/`molt_anext` now use `attr_name_bits_from_bytes`, fixing async-for resolving `__anext__` as
  `__aiter__` ("object is not awaitable").
- Awaitable debug now includes class name when poll_fn is missing.

Files touched (uncommitted)
- CHECKPOINT.md

Tests run
- uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/async_for_else.py
- uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/async_for_iter.py
- uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/async_long_running.py

Known gaps
- Allowlisted module calls still reject keywords/star args; only Molt-defined callables accept CALL_BIND.
- async with multi-context and destructuring targets remain unsupported (see docs/spec/STATUS.md).
- BaseException hierarchy and typed matching remain partial (see docs/spec/0014_TYPE_COVERAGE_MATRIX.md).
- OPT-0007/0008 regressions still open (struct/descriptor/attr access).
- OPT-0009 partial: bench_str_split.py ~2x CPython, bench_str_join.py ~0.91x.
- Fuzz invocation needs a bounded run (e.g. max time) to be treated as a clean pass.
- bench_struct/bench_attr_access/bench_descriptor_property remain far below CPython; prioritize OPT-0007/0008 follow-through.
- Codon baseline skips asyncio/bytearray/memoryview/molt_buffer/molt_msgpack/struct-init benches.
