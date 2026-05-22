# Runtime Safety Guard Env Plan

## Design

`tools/runtime_safety.py` already routes sanitizer, Miri, fuzz, and clippy
commands through `harness_memory_guard.guarded_completed_process`, but `_run`
passes either `os.environ.copy()` or a caller-provided env directly. That leaves
canonical Molt harness roots (`MOLT_EXT_ROOT`, `CARGO_TARGET_DIR`,
`MOLT_DIFF_CARGO_TARGET_DIR`, `MOLT_CACHE`, `MOLT_DIFF_ROOT`,
`MOLT_DIFF_TMPDIR`, `UV_CACHE_DIR`, `TMPDIR`, `MOLT_SESSION_ID`) dependent on
external shell setup for a Year-1 safety lane.

Make `_run` canonicalize the environment structurally via
`harness_memory_guard.canonical_harness_env(..., repo_root=ROOT)` before any
Cargo/sanitizer command is launched. Preserve caller overrides such as
`RUSTFLAGS`, `MIRIFLAGS`, `TMPDIR`, and `MOLT_SESSION_ID`; only fill missing
defaults.

## Files

- `tools/runtime_safety.py`
- `tests/test_runtime_safety.py`
- `tools/check_memory_guard_wiring.py`
- this plan artifact

## Tests

- Baseline: `uv run --python 3.12 python3 -m pytest tests/test_runtime_safety.py -q`
- Focused: `uv run --python 3.12 python3 -m pytest tests/test_runtime_safety.py tests/tools/test_check_memory_guard_wiring.py -q`
- Audit: `uv run --python 3.12 python3 tools/check_memory_guard_wiring.py`
- Audit: `uv run --python 3.12 python3 tools/check_subprocess_guard_coverage.py`

## Risks

- Sanitizer/Miri env setup must keep explicit toolchain variables intact.
- Miri's explicit or repo-local `TMPDIR` behavior must not change except by
  installing the same canonical defaults every other guarded harness uses.

## Exit Criteria

- Runtime safety commands always receive canonical Molt harness env defaults
  when callers omit them.
- Existing explicit env overrides remain preserved.
- Guard wiring audit tracks `tools/runtime_safety.py` as a canonical-env
  contract, not just a memory-guard token contract.
- Focused tests and guard audits pass.
