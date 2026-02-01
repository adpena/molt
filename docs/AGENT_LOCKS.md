# Agent Locks

This file coordinates parallel agent work to prevent file collisions.

## How to use
- Claim a file or directory by adding a line: `<agent-id> -> <path>`.
- Keep claims narrow and time-bound.
- Remove your claim when finished.

## Active locks
codex-26850 -> tools/bench.py
codex-26850 -> tools/bench_wasm.py
codex-26850 -> docs/benchmarks
codex-26850 -> README.md
codex-26850 -> docs/BENCHMARKING.md
codex-26850 -> tests/benchmarks
codex-26850 -> src/molt/shims_runtime.py
codex-26850 -> src/molt/stdlib/dataclasses.py
codex-26850 -> runtime/molt-runtime/src/builtins/attr.rs
codex-26850 -> runtime/molt-runtime/src/builtins/attributes.rs
codex-26850 -> runtime/molt-runtime/src/async_rt
codex-26850 -> runtime/molt-runtime/src/lib.rs
codex-26850 -> runtime/molt-runtime/src/state/runtime_state.rs
codex-26850 -> runtime/molt-backend/src/wasm.rs
codex-26850 -> tools/wasm_link.py
codex-26850 -> src/molt/cli.py
codex-26850 -> runtime/molt-wasm-host
codex-26850 -> src/molt/stdlib/contextlib.py
codex-26850 -> src/molt/frontend/__init__.py
codex-26850 -> runtime/molt-runtime/src/builtins/context.rs
codex-26850 -> runtime/molt-runtime/src/concurrency
codex-26850 -> runtime/molt-runtime/src/state
codex-26850 -> runtime/molt-runtime/src/async_rt
codex-26850 -> runtime/molt-runtime/src/builtins
codex-26850 -> runtime/molt-runtime/src/object
codex-26850 -> runtime/molt-runtime/src/call
codex-26850 -> src/molt/stdlib/threading.py
codex-26850 -> src/molt/stdlib
codex-26850 -> src/molt/shims.py
codex-26850 -> src/molt/shims_cpython.py
codex-26850 -> src/molt/_intrinsics.pyi
codex-26850 -> src/molt/frontend
codex-26850 -> runtime/molt-backend/src/lib.rs
codex-26850 -> runtime/molt-backend/src/wasm.rs
codex-26850 -> runtime/molt-runtime/src/concurrency/isolates.rs
codex-26850 -> docs/spec/areas/core/0000-vision.md
codex-26850 -> docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md
codex-26850 -> docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md
codex-26850 -> docs/AGENTS.md
codex-26850 -> tests/differential
codex-26850 -> tests/molt_diff.py
codex-26850 -> tools
codex-26850 -> docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md
codex-26850 -> docs/spec/STATUS.md
codex-26850 -> tests/differential/INDEX.md
