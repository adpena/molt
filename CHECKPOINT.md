Checkpoint: 2026-01-24T20:24:18Z
Git: af2e937884b15112580c183ec29dd13c72d40da8 (dirty)

Summary
- Hardened async sleep zero-delay behavior to yield once without depending on monotonic deadlines.
- Added async trace instrumentation hooks for scheduler/sleep/awaiter paths.

Files touched (uncommitted)
- CHECKPOINT.md
- docs/AGENT_LOCKS.md
- runtime/molt-runtime/src/async_rt/generators.rs
- runtime/molt-runtime/src/async_rt/mod.rs
- runtime/molt-runtime/src/async_rt/scheduler.rs
- Large pre-existing dirty tree remains; see `git status -sb` for full list.

Docs/spec updates needed?
- None this turn (STATUS/0014/0015/0023/0400/ROADMAP updated).

Tests
- `uv sync --python 3.12`
- `uv run --python 3.12 python3 -m molt.cli run --compiled tests/benchmarks/bench_async_await.py` (timed out after 180s; likely compile + bench)

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
