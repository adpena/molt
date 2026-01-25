# Agent Locks

This file coordinates parallel agent work to prevent file collisions.

## How to use
- Claim a file or directory by adding a line: `<agent-id> -> <path>`.
- Keep claims narrow and time-bound.
- Remove your claim when finished.

## Active locks
codex-79872 -> run_wasm.js
codex-79872 -> runtime/molt-backend/
codex-79872 -> runtime/molt-runtime/src/async_rt/
codex-79872 -> src/molt/cli.py
codex-79872 -> runtime/molt-runtime/src/builtins/
codex-79872 -> runtime/molt-runtime/src/state/runtime_state.rs
codex-79872 -> runtime/molt-runtime/src/object/mod.rs
codex-79872 -> tests/wasm_harness.py
codex-79872 -> src/molt/stdlib/asyncio.py
codex-79872 -> src/molt/shims_cpython.py
codex-79872 -> tests/differential/basic/
codex-79872 -> src/molt/concurrency.py
