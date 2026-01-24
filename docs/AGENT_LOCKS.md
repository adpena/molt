# Agent Locks

This file coordinates parallel agent work to prevent file collisions.

## How to use
- Claim a file or directory by adding a line: `<agent-id> -> <path>`.
- Keep claims narrow and time-bound.
- Remove your claim when finished.

## Active locks
codex-26366 -> runtime/molt-runtime/src/async_rt/; runtime/molt-runtime/src/concurrency/; runtime/molt-runtime/src/lib.rs; runtime/molt-runtime/src/object/; runtime/molt-runtime/src/state/; runtime/molt-backend/src/wasm.rs; runtime/molt-backend/src/lib.rs; src/molt/frontend/__init__.py; src/molt/stdlib/asyncio.py; tests/wasm_harness.py
