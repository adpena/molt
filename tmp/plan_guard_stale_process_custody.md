# Guard Stale Process Custody Plan

## Design

- Extend the shared process sample contract with optional elapsed-process age from `ps etimes`.
- Carry that age through repo-scoped process groups and sentinel violations.
- Add conservative stale-orphan classification: only repo-scoped Molt groups whose external parent is init/launchd and whose age exceeds the configured threshold are eligible.
- Make shared guarded subprocesses run a default stale-orphan preflight before starting their command, with shorter policy for orphaned pytest-style groups.
- Emit operator-facing stderr plus JSONL custody events with reason, age, killed time, process group, and next action.

## Files

- `tools/memory_guard.py`
- `tools/process_sentinel.py`
- `tools/harness_memory_guard.py`
- `tests/test_memory_guard_tool.py`
- `tests/tools/test_process_sentinel.py`
- `tests/test_harness_memory_guard.py`
- `docs/OPERATIONS.md`
- `docs/DEVELOPER_GUIDE.md`

## Tests

- Focused pytest for memory guard parsing and harness sentinel behavior.
- Focused pytest for process sentinel stale-orphan classification.
- `python3 tools/dev.py validate --check --json --suite smoke --summary-out tmp/guard-stale-validate-plan.json`.
- Ruff on touched Python files.

## Exit Criteria

- Existing guarded processes remain default-on and adaptive.
- Stale cleanup is conservative, configurable, and produces useful evidence.
- No direct host-CPython fallback or test special casing.
