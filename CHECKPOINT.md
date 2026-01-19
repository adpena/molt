Checkpoint: 2026-01-19T03:32:44Z
Git: ddb9f5feaa72a0263742f336c24f28e1764ac788 (dirty)

Summary
- Added wasm harness `open_builtin` import guard that raises capability errors instead of failing instantiation.
- Fixed `string_ops` fuzz target to pass the `molt_string_from_bytes` out pointer correctly after ABI change.
- Ran full formalize pipeline (lint/tests/clippy, native+WASM benches, bench report update, fuzz, Miri).

Files touched (uncommitted)
- CHECKPOINT.md
- docs/AGENT_LOCKS.md
- docs/AGENT_MEMORY.md
- docs/benchmarks/bench_summary.md
- README.md
- bench/results/bench.json
- bench/results/bench_wasm.json
- runtime/molt-runtime/fuzz/fuzz_targets/string_ops.rs
- tests/wasm_harness.py
- Large pre-existing dirty tree remains; see `git status -sb` for full list.

Docs/spec updates needed?
- None this turn (STATUS/0014/ROADMAP updated).

Tests
- `uv run --python 3.12 ./tools/dev.py lint`
- `./tools/dev.py test`
- `cargo clippy -- -D warnings`
- `uv run --python 3.14 python3 tools/runtime_safety.py fuzz --target string_ops --runs 10000`
- `uv run --python 3.14 python3 tools/runtime_safety.py miri`

Benchmarks
- `uv run --python 3.14 python3 tools/bench.py --smoke --json-out logs/bench_smoke_native.json`
- `uv run --python 3.14 python3 tools/bench_wasm.py --smoke --require-linked --json-out logs/bench_smoke_wasm.json`
- `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`
- `uv run --python 3.14 python3 tools/bench_wasm.py --require-linked --json-out bench/results/bench_wasm.json`
- `uv run --python 3.14 python3 tools/bench_report.py --update-readme`

Profiling
- None.

Known gaps
- Exception hierarchy mapping still uses Exception/BaseException fallback (no full CPython hierarchy).
- `__traceback__` remains tuple-only; full traceback objects pending.
- `str(bytes, encoding, errors)` decoding not implemented (NotImplementedError).
- `print(file=None)` uses host stdout if `sys` is not initialized.
- File I/O gaps: non-UTF-8 encodings/errors, text-mode seek/tell cookie semantics, readinto/writelines/detach/reconfigure, Windows fileno/isatty parity.
- WASM host hooks missing for full `open()` + file method parity.
- WASM `str_from_obj` does not call `__str__` for non-primitive objects.
- Backend panic for classes defining `__next__` without `__iter__` (see ROADMAP TODO).
- `sys.argv` decoding still uses lossy UTF-8/UTF-16 until filesystem-encoding + surrogateescape parity lands.
- Pointer registry lock contention optimization still pending (see OPT-0003).

CI
- Last green: https://github.com/adpena/molt/actions/runs/21060145271.
