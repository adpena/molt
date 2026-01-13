Checkpoint: 2026-01-13T18:52:08Z
Git: 0e079e6d2c617dcfee4afe59b798cbaf60debe03 (dirty)

Summary
- Added `tools/profile.py` to run repeatable CPU/alloc profiling (plus optional compiler cProfile).
- Documented the profiling harness in `LOGGING_AND_BENCHMARK_CONVENTIONS.md`.
- Scoped the wasm async fix but blocked by the `src/molt/stdlib/asyncio.py` lock.

Files touched (uncommitted)
- tools/profile.py
- LOGGING_AND_BENCHMARK_CONVENTIONS.md
- docs/AGENT_LOCKS.md
- docs/AGENT_MEMORY.md
- CHECKPOINT.md
- Other pre-existing local modifications not listed here (see git status).

Docs/spec updates needed?
- None.

Tests run
- None.

Benchmarks
- None.

Known gaps
- WASM async benchmarks are blocked by a compat error in `src/molt/stdlib/asyncio.py:48` (lock held by codex-98895).

CI
- Not run (local changes only).
