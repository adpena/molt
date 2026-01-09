Checkpoint: 2026-01-09 16:22:18 CST
Git: 657885a6a64f5c0f13ccbf71d1604f2873f3f7af

Summary
- Fixed rustfmt/clippy regressions in runtime/backend after the async-with spill updates.
- Added safety docs for cancellation/callargs externs, tightened thread-local initialization, and cleaned minor clippy nits.
- Confirmed clippy passes locally and CI green across rust/wasm/python jobs.

Files touched (uncommitted)
- None

Tests run
- uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/iter_methods.py
- uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/async_with_suppress.py
- uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/try_except.py
- uv run --python 3.12 python3 tools/dev.py lint
- uv run --python 3.12 python3 tools/dev.py test
- cargo clippy -- -D warnings
- cargo fmt

Known gaps
- Allowlisted module calls still reject keywords/star args; only Molt-defined callables accept CALL_BIND.
- async with multi-context and destructuring targets remain unsupported (see docs/spec/STATUS.md).
- BaseException hierarchy and typed matching remain partial (see docs/spec/0014_TYPE_COVERAGE_MATRIX.md).
- OPT-0007/0008 regressions still open (struct/descriptor/attr access).
- OPT-0009 still open: bench_str_split.py 0.27x and bench_str_join.py 0.52x vs CPython.
- Fuzz invocation needs a bounded run (e.g. max time) to be treated as a clean pass.
- bench_struct/bench_attr_access/bench_descriptor_property remain far below CPython; prioritize OPT-0007/0008 follow-through.
- Codon baseline skips asyncio/bytearray/memoryview/molt_buffer/molt_msgpack/struct-init benches.
