Checkpoint: 2026-01-09 18:37:04 CST
Git: 72763a5f86dec91200943c986f8fe932ab45a215

Summary
- Split shims into runtime vs CPython, added builtin function objects + missing sentinel, and wired wasm harness imports for new builtins.
- Fixed matmul loop lowering to handle boxed locals safely and restored buffer2d matmul fast path.
- Updated stdlib/type coverage docs, STATUS/README/ROADMAP, and refreshed benchmark artifacts.

Files touched (uncommitted)
- None

Tests run
- uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json
- uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json
- uv run --python 3.12 python3 tools/dev.py lint
- uv run --python 3.12 python3 tools/dev.py test
- CI: main workflow run 20869703735 (success)

Known gaps
- Allowlisted module calls still reject keywords/star args; only Molt-defined callables accept CALL_BIND.
- async with multi-context and destructuring targets remain unsupported (see docs/spec/STATUS.md).
- BaseException hierarchy and typed matching remain partial (see docs/spec/0014_TYPE_COVERAGE_MATRIX.md).
- OPT-0007/0008 regressions still open (struct/descriptor/attr access).
- OPT-0009 still open: bench_str_split.py and bench_str_join.py remain below CPython.
- Fuzz invocation needs a bounded run (e.g. max time) to be treated as a clean pass.
- bench_struct/bench_attr_access/bench_descriptor_property remain far below CPython; prioritize OPT-0007/0008 follow-through.
- Codon baseline skips asyncio/bytearray/memoryview/molt_buffer/molt_msgpack/struct-init benches.
