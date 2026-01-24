Checkpoint: 2026-01-19T12:43:45Z
Git: acff6bae470a3112a63f421bba352a51184a0d5d (dirty)

Summary
- Added utf-8/ascii/latin-1 text encoding support and basic text-mode seek/tell cookies in file I/O.
- Updated wasm file import coverage (WIT + backend + harness stubs) and parity docs for remaining gaps.

Files touched (uncommitted)
- CHECKPOINT.md
- docs/AGENT_LOCKS.md
- docs/spec/STATUS.md
- docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md
- docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md
- docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md
- docs/spec/areas/wasm/0400_WASM_PORTABLE_ABI.md
- ROADMAP.md
- runtime/molt-backend/src/wasm.rs
- runtime/molt-runtime/src/lib.rs
- tests/wasm_harness.py
- wit/molt-runtime.wit
- Large pre-existing dirty tree remains; see `git status -sb` for full list.

Docs/spec updates needed?
- None this turn (STATUS/0014/0015/0023/0400/ROADMAP updated).

Tests
- `cargo test -p molt-runtime`
- `uv run --python 3.12 ./tools/dev.py test`

Benchmarks
- Not run.

Profiling
- None.

Known gaps
- Exception hierarchy mapping still uses Exception/BaseException fallback (no full CPython hierarchy).
- `__traceback__` remains tuple-only; full traceback objects pending.
- `str(bytes, encoding, errors)` decoding not implemented (NotImplementedError).
- `print(file=None)` uses host stdout if `sys` is not initialized.
- File I/O gaps: broader codecs + full error handlers (utf-8/ascii/latin-1 only), partial text-mode seek/tell cookies, detach/reconfigure, Windows fileno/isatty parity.
- WASM host hooks for remaining file methods (detach/reconfigure) and parity coverage pending.
- WASM `str_from_obj` does not call `__str__` for non-primitive objects.
- Backend panic for classes defining `__next__` without `__iter__` (see ROADMAP TODO).
- `sys.argv` decoding still uses lossy UTF-8/UTF-16 until filesystem-encoding + surrogateescape parity lands.
- Pointer registry lock contention optimization still pending (see OPT-0003).

CI
- Last green: https://github.com/adpena/molt/actions/runs/21060145271.
