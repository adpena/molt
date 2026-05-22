Design:
- Add an AST-based subprocess guard coverage audit so dev/test/bench raw
  subprocess launches cannot drift silently away from the shared memory guard.
- Treat existing raw subprocess calls as an explicit structural contract:
  guard internals, OS/process metadata probes, interactive Popen paths with
  guard context/process-group custody, and CPython subprocess differential
  inputs are allowed; new unclassified calls fail.
- Wire the audit into the DX lint/validate surfaces so the contract is checked
  by normal developer workflows.

Files:
- tools/check_subprocess_guard_coverage.py
- tests/tools/test_subprocess_guard_coverage.py
- pyproject.toml
- src/molt/cli.py
- tests/cli/test_cli_setup_validate.py
- docs/DEVELOPER_GUIDE.md
- docs/OPERATIONS.md

Tests:
- Focused pytest for the new audit and validate/Dev planner regressions.
- Ruff check/format on touched Python.
- tools/dev.py validate --check --json --suite smoke to prove the audit appears
  in the default validation plan with guard budgets.

Exit criteria:
- The current repository audit is green.
- A synthetic raw subprocess call fails the audit.
- Stale allowlist entries fail the audit.
- Smoke validation lists the audit under the default guarded command surface.
