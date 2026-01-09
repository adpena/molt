Checkpoint: 2026-01-09 15:49:16 CST
Git: 39fc8997b0f28df2fc4f1292a6333679f2932edf

Summary
- Spilled async-with exception values across awaits to fix backend dominance errors and keep suppression behavior aligned with CPython.
- Simplified async-with calls to use CALL_FUNC so bound-method handling stays in the backend and async IR stays linear.
- Verified async-with suppression and try/except parity after the spill fix.
- Full lint/test pass completed across supported Python versions plus the differential suite.

Files touched (uncommitted)
- AGENTS.md
- CHECKPOINT.md
- GEMINI.md
- README.md
- ROADMAP.md
- docs/spec/0014_TYPE_COVERAGE_MATRIX.md
- docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md
- docs/spec/0020_RUNTIME_SAFETY_INVARIANTS.md
- docs/spec/0100_MOLT_IR.md
- docs/spec/0300_TASKS_AND_CHANNELS.md
- docs/spec/0704_TRANSACTIONS_AND_CANCELLATION.md
- docs/spec/0900_HTTP_SERVER_RUNTIME.md
- docs/spec/0901_WEB_FRAMEWORK_AND_ROUTING.md
- docs/spec/STATUS.md
- runtime/molt-backend/src/lib.rs
- runtime/molt-backend/src/wasm.rs
- runtime/molt-runtime/src/lib.rs
- src/molt/__init__.py
- src/molt/_intrinsics.pyi
- src/molt/capabilities.py
- src/molt/cli.py
- src/molt/compat.py
- src/molt/concurrency.py
- src/molt/frontend/__init__.py
- src/molt/net.py
- src/molt/shims.py
- src/molt/stdlib/asyncio.py
- src/molt/stdlib/builtins.py
- src/molt/stdlib/copy.py
- src/molt/stdlib/inspect.py
- src/molt/stdlib/os.py
- src/molt/stdlib/pprint.py
- src/molt/stdlib/string.py
- src/molt/stdlib/traceback.py
- src/molt/stdlib/typing.py
- tests/molt_diff.py
- tests/test_magic_concurrency.py
- tests/wasm_harness.py
- wit/molt-runtime.wit
- docs/spec/0016_ARGS_KWARGS.md
- docs/spec/0966_EXTERNAL_INSPIRATIONS_CODON_PY2WASM_TRIO_GO_OPENMP.md
- runtime/molt-backend/tests/
- tests/differential/basic/args_kwargs.py
- tests/differential/basic/async_anext_default_future.py
- tests/differential/basic/async_anext_future.py
- tests/differential/basic/async_cancellation_token.py
- tests/differential/basic/async_for_else.py
- tests/differential/basic/async_long_running.py
- tests/differential/basic/async_with_basic.py
- tests/differential/basic/async_with_instance_callable.py
- tests/differential/basic/async_with_suppress.py
- tests/differential/basic/asyncio_sleep_result.py
- tests/differential/basic/container_methods.py
- tests/differential/basic/enumerate_basic.py
- tests/differential/basic/hashability.py
- tests/differential/basic/loop_break_continue.py
- tests/differential/basic/stdlib_allowlist_calls.py
- tests/differential/basic/iter_methods.py
- tools/bench_wasm.py

Tests run
- uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/iter_methods.py
- uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/async_with_suppress.py
- uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic/try_except.py
- uv run --python 3.12 python3 tools/dev.py lint
- uv run --python 3.12 python3 tools/dev.py test

Known gaps
- Allowlisted module calls still reject keywords/star args; only Molt-defined callables accept CALL_BIND.
- async with multi-context and destructuring targets remain unsupported (see docs/spec/STATUS.md).
- BaseException hierarchy and typed matching remain partial (see docs/spec/0014_TYPE_COVERAGE_MATRIX.md).
- OPT-0007/0008 regressions still open (struct/descriptor/attr access).
- OPT-0009 still open: bench_str_split.py 0.27x and bench_str_join.py 0.52x vs CPython.
- Fuzz invocation needs a bounded run (e.g. max time) to be treated as a clean pass.
- bench_struct/bench_attr_access/bench_descriptor_property remain far below CPython; prioritize OPT-0007/0008 follow-through.
- Codon baseline skips asyncio/bytearray/memoryview/molt_buffer/molt_msgpack/struct-init benches.
