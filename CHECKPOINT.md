Checkpoint: 2026-01-08 12:00:11 CST
Git: 1dd3104 chore: rustfmt

Summary
- Restored single-byte split to the count+split path and cached join element pointers for a faster copy loop.
- Fixed wasm user function type allocation after adding guarded field ops to resolve the async protocol failure.
- Re-ran arm64 benchmarks and refreshed README + OPT-0009 notes with updated perf numbers.
- Ran lint/test across 3.12/3.13/3.14 plus molt-runtime cargo tests; reformatted the frontend IR file.
- Applied cargo fmt fixes after CI rustfmt failure.

Files touched (uncommitted)
- .gitignore
- AGENTS.md
- CHECKPOINT.md
- GEMINI.md
- OPTIMIZATIONS_PLAN.md
- README.md
- bench/results/bench.json
- docs/spec/0014_TYPE_COVERAGE_MATRIX.md
- runtime/molt-backend/src/lib.rs
- runtime/molt-backend/src/wasm.rs
- runtime/molt-runtime/src/lib.rs
- src/molt/frontend/__init__.py
- tests/differential/basic/descriptor_precedence.py
- tests/differential/basic/class_mutation_init_deopt.py
- tests/wasm_harness.py
- tools/bench.py
- wit/molt-runtime.wit

Tests run
- uv run --python /opt/homebrew/bin/python3.14 python3 tools/bench.py --json-out bench/results/bench.json
- uv run --python 3.12 python3 tools/dev.py lint
- uv run --python 3.12 python3 tools/dev.py test
- cargo test -p molt-runtime

Known gaps
- BaseException hierarchy and typed matching remain partial (see `docs/spec/0014_TYPE_COVERAGE_MATRIX.md`).
- OPT-0007/0008 regressions still open (struct/descriptor/attr access).
- OPT-0009 still open: `bench_str_split.py` 0.27x and `bench_str_join.py` 0.52x vs CPython.
- Fuzz invocation needs a bounded run (e.g. max time) to be treated as a clean pass.
- bench_struct/bench_attr_access/bench_descriptor_property remain far below CPython; prioritize OPT-0007/0008 follow-through.
- Codon baseline skips asyncio/bytearray/memoryview/molt_buffer/molt_msgpack/struct-init benches.

Pending changes
- .gitignore
- AGENTS.md
- CHECKPOINT.md
- GEMINI.md
- OPTIMIZATIONS_PLAN.md
- README.md
- bench/results/bench.json
- docs/spec/0014_TYPE_COVERAGE_MATRIX.md
- runtime/molt-backend/src/lib.rs
- runtime/molt-backend/src/wasm.rs
- runtime/molt-runtime/src/lib.rs
- src/molt/frontend/__init__.py
- tests/differential/basic/descriptor_precedence.py
- tests/differential/basic/class_mutation_init_deopt.py
- tests/wasm_harness.py
- tools/bench.py
- wit/molt-runtime.wit
