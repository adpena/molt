# Agent Locks

This file coordinates parallel agent work to prevent file collisions.

## How to use
- Claim a file or directory by adding a line: `<agent-id> -> <path>`.
- Keep claims narrow and time-bound.
- Remove your claim when finished.

## Active locks
codex-19854 -> tests/differential/
codex-19854 -> src/molt/stdlib/collections/__init__.py
codex-19854 -> src/molt/stdlib/keyword.py
codex-19854 -> src/molt/stdlib/sys.py
codex-19854 -> src/molt/stdlib/asyncio.py
codex-19854 -> src/molt/stdlib/builtins.py
codex-19854 -> src/molt/_intrinsics.pyi
codex-19854 -> src/molt/frontend/__init__.py
codex-19854 -> src/molt/cli.py
codex-19854 -> AGENTS.md
codex-19854 -> src/molt/shims_cpython.py
codex-19854 -> runtime/molt-runtime/src/object/ops.rs
codex-19854 -> runtime/molt-runtime/src/state/runtime_state.rs
codex-19854 -> runtime/molt-runtime/src/builtins/methods.rs
codex-19854 -> runtime/molt-runtime/src/state/cache.rs
codex-19854 -> runtime/molt-runtime/Cargo.toml
codex-19854 -> Cargo.lock
codex-19854 -> docs/spec/STATUS.md
codex-19854 -> docs/ROADMAP.md
codex-19854 -> docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md
codex-19854 -> wit/molt-runtime.wit
