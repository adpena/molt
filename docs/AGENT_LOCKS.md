# Agent Locks

This file coordinates parallel agent work to prevent file collisions.

## How to use
- Claim a file or directory by adding a line: `<agent-id> -> <path>`.
- Keep claims narrow and time-bound.
- Remove your claim when finished.

## Active locks
- codex -> runtime/molt-runtime/, src/molt/frontend/, tests/differential/basic/, tests/wasm_harness.py, docs/spec/0014_TYPE_COVERAGE_MATRIX.md, docs/spec/STATUS.md, ROADMAP.md
