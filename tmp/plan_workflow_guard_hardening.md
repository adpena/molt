## Workflow Guard Hardening Plan

### Design

The local Molt harnesses already route known benchmark, test, compliance, and
dev subprocesses through `tools.harness_memory_guard` or
`tools/guarded_exec.py`. The remaining gap is hosted workflow commands that run
memory-intensive build/test/proof/security commands directly. Those commands
should use the same `guarded_exec.py` boundary so CI/release failures report
timeouts, RSS pressure, and orphan cleanup consistently with local runs.

### Files

- `.github/workflows/nightly.yml`
- `.github/workflows/release.yml`
- `.github/workflows/formal.yml`
- `.github/workflows/security_hardening.yml`
- `tests/test_ci_workflow_topology.py`

### Tests

- `python3 tools/check_memory_guard_wiring.py`
- `python3 tools/check_subprocess_guard_coverage.py`
- `python3 -m pytest tests/test_ci_workflow_topology.py -q`

### Risks

- Release runs on Windows as well as Unix. Use the selected `$PYTHON_BIN` for
  release workflow guard invocations so the wrapper works across the release
  matrix.
- Formal Lean verification previously used `working-directory`; use
  `guarded_exec.py --cwd formal/lean` from the repo root instead of relying on
  relative wrapper paths.

### Exit Criteria

- Hosted workflow memory-intensive build/test/proof/security commands enter
  `tools/guarded_exec.py` by default.
- Workflow topology tests fail if those guarded commands are replaced by raw
  `cargo build`, `cargo deny`, `cargo audit`, `pip-audit`, `lake build`, or
  formal check invocations.
- Focused guard/topology tests pass under canonical env and the worktree is
  clean after commit/push.
