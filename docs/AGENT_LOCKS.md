# Agent Locks

This file coordinates parallel agent work to prevent file collisions.

## How to use
- Claim a file or directory by adding a line: `<agent-id> -> <path>`.
- Keep claims narrow and time-bound.
- Remove your claim when finished.

## Active locks
codex-79872 -> runtime/molt-runtime/src/async_rt/
codex-79872 -> runtime/molt-runtime/src/state/
codex-79872 -> runtime/molt-backend/
codex-79872 -> wit/molt-runtime.wit
codex-79872 -> src/molt/frontend/__init__.py
codex-79872 -> src/molt/stdlib/asyncio.py
codex-79872 -> src/molt/stdlib/contextvars.py
codex-79872 -> src/molt/stdlib/weakref.py
codex-79872 -> tests/differential/basic/
