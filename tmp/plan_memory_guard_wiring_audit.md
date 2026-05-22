# Memory Guard Wiring Audit Plan

## Design

Promote the default-on memory guard wiring contract from passive pytest token
checks into a reusable executable audit. The audit should be the single
repo-local source for:

- Python guard entrypoints that must import/use `tools.harness_memory_guard`.
- Legacy shell wrappers that must enter a guarded Python wrapper.
- Process sentinel coverage for every scanner-discovered guarded entrypoint.

The audit must be runnable directly, by `tools/dev.py lint`, and by
`molt validate --suite smoke`. It should fail loudly on missing tokens or stale
sentinel scanner coverage, and emit compact JSON when requested.

## Files

- Add `tools/check_memory_guard_wiring.py`.
- Reuse `tools/guarded_entrypoints.py` for scanner-derived sentinel tokens.
- Update `pyproject.toml` lint command refs.
- Update `src/molt/cli.py` smoke validation steps.
- Update focused tests under `tests/test_memory_guard_wiring.py`,
  `tests/tools/test_dev_py.py`, and `tests/cli/test_cli_setup_validate.py`.
- Update docs that describe canonical lint/validate guard surfaces if wording
  changes.

## Tests

Baseline already run before edits:

- `uv run --python 3.12 python3 -m pytest -q tests/test_memory_guard_wiring.py tests/tools/test_process_sentinel.py::test_guarded_entrypoints_are_repo_sentinel_tokens tests/cli/test_cli_setup_validate.py::test_cli_validate_check_json_reports_canonical_matrix tests/tools/test_dev_py.py::test_dev_py_lint_uses_documented_stdlib_intrinsic_gates`
  - `5 passed`

Planned proof after edits:

- Direct audit: `python3 tools/check_memory_guard_wiring.py`.
- Focused pytest for the audit and lint/validate planners.
- Ruff on touched Python files.
- `molt validate --check --json --suite smoke` to confirm the audit is part of
  canonical smoke validation.

## Risks

- A token-only audit can become ceremony. Keep it tied to existing executable
  guard mechanisms and the process sentinel scanner rather than adding
  unrelated style checks.
- Shell wrappers are intentionally static; only enforce the wrapper-entry
  contract, not shell internals.
- Do not expand the raw subprocess allowlist here; subprocess custody remains
  owned by `tools/check_subprocess_guard_coverage.py`.

## Exit Criteria

- New direct audit passes on the current tree.
- Lint includes both subprocess coverage and memory-guard wiring audits.
- Smoke validate includes both audits under `MOLT_TEST_SUITE`.
- Focused tests prove missing tokens/stale scanner coverage are surfaced.
- Working tree contains only owned changes and is ready for atomic commit.
