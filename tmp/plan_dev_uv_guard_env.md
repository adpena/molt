# Dev UV Guard Env Plan

## Design

`tools/dev.py` already routes `uv run` commands through
`harness_memory_guard.guarded_completed_process`, but the `run_uv` path starts
from the caller environment instead of the repo canonical artifact/cache roots.
That leaves `tools/dev.py test`, `setup`, `doctor`, `update`, and `validate`
guarded for RSS/process cleanup while still able to inherit ad hoc target,
cache, diff, and temp roots.

Move canonical root installation to the common `run_uv` path before uv
normalization, while preserving caller-provided overrides. The guard remains
default-on and the existing UV_NO_SYNC version probe stays guarded.

## Files

- `tools/dev.py`
- `tests/tools/test_dev_py.py`
- `docs/DEVELOPER_GUIDE.md`

## Tests

- Baseline: focused existing `test_dev_py_uv_no_sync_normalization_uses_guarded_probe`.
- Add regression that `run_uv` passes canonical Molt roots into the guarded
  command environment.
- Add regression that explicit caller roots are preserved.
- Run focused `tests/tools/test_dev_py.py` slice.

## Risks

- Overwriting caller overrides would break deliberate external artifact roots;
  use `setdefault`-style canonicalization through `canonical_harness_env`.
- UV_NO_SYNC normalization must continue to probe with the same canonical env.

## Exit Criteria

- Every `run_uv`-backed dev command has `MOLT_EXT_ROOT`, `CARGO_TARGET_DIR`,
  `MOLT_DIFF_CARGO_TARGET_DIR`, `MOLT_CACHE`, `MOLT_DIFF_ROOT`,
  `MOLT_DIFF_TMPDIR`, `UV_CACHE_DIR`, `TMPDIR`, and `MOLT_SESSION_ID` before
  memory-guard execution.
- Focused regression passes.
- Worktree is staged atomically and clean after commit.
