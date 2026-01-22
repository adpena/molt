# Agent Locks

This file coordinates parallel agent work to prevent file collisions.

## How to use
- Claim a file or directory by adding a line: `<agent-id> -> <path>`.
- Keep claims narrow and time-bound.
- Remove your claim when finished.

## Active locks
codex-68675 -> tests/differential/planned/
codex-68675 -> tests/differential/INDEX.md
