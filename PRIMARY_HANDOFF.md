# Primary Handoff

This file is the fast resume point for continuing Molt development on `Primary`.

The wrapper/build-contract work referenced here was audited complete on 2026-03-30. This handoff file is now the canonical resume point for that slice.

## Current State

- Branch: `main`
- Remote: `origin/main`
- This handoff file was added immediately after the repo was verified clean and synced on commit `b628f233`.
- No hidden local memory should be assumed. The portable state is the repo, the pushed commits, the plan docs, and canonical artifacts under `logs/`, `tmp/`, and `target/`.

## What Was Finished

- Wrapper artifact contract work is complete for `run`, cross-target `run`, and `deploy`.
- Build JSON now carries additive semantic fields for wrapper consumers:
  - `consumer_output`
  - `bundle_root`
  - target-specific `artifacts`
  - `messages`
- Native and non-native wrappers now consume the build contract instead of reconstructing output paths locally.
- Non-JSON wrapper flows now replay nested build warnings, success messages, diagnostics, and failure detail instead of silently discarding them.
- Luau artifact semantics were corrected.
- Cloudflare split-runtime deploy now uses emitted `bundle_root` and `wrangler_config`.

## Verified Commands

Run these exact commands first on `Primary` if you want to re-establish the validated state:

```bash
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp

python3 -m py_compile src/molt/cli.py tests/cli/test_cli_import_collection.py
./.venv/bin/ruff check src/molt/cli.py tests/cli/test_cli_import_collection.py
./.venv/bin/python -m pytest -q tests/cli/test_cli_import_collection.py
python3 -m molt.cli run --profile dev examples/hello.py
MOLT_DIFF_MEASURE_RSS=1 ./.venv/bin/python -u tests/molt_diff.py --build-profile dev --jobs 1 \
  tests/differential/stdlib/importlib_support_bootstrap.py \
  tests/differential/stdlib/importlib_import_module_helper_constant.py
python3 -m molt.cli build --build-profile dev --out-dir tmp/static_hello_build \
  --diagnostics \
  --diagnostics-file logs/importlib_runtime_support_20260327/hello_build_diagnostics.json \
  examples/hello.py
```

Validated outcomes before this file was added:
- `tests/cli/test_cli_import_collection.py`: `237 passed, 3 skipped`
- `python3 -m molt.cli run --profile dev examples/hello.py`: printed `42`
- Differential control cases passed with RSS enabled
- Static diagnostics build passed and wrote diagnostics under canonical roots

## Resume Here

Recommended next task order:

1. Unify `compare()` onto the shared build artifact contract.
2. Add Windows `.exe` contract tests for both `run` and `compare`.
3. Add wasm explicit `--output` and unlinked `consumer_output` tests.
4. Add Luau explicit `--output` coverage on direct `run`.
5. Harden Cloudflare negative-path validation:
   - missing `bundle_root`
   - missing `wrangler_config`
   - `wrangler_config` outside `bundle_root`
   - split-runtime explicit `--output` / `--out-dir`
6. Normalize successful JSON UX across cross-run and deploy.
7. Harden backend daemon UX:
   - readiness/backoff
   - stale-socket handling
   - quiet-by-default logs
   - debug-only detailed backend output

## Known Remaining Risks

- `compare()` still manually parses build JSON and still keys off `data.output` rather than the shared wrapper contract.
- Windows native `.exe` fallback exists but is not yet proven by dedicated tests.
- wasm and Luau explicit-output cases are not fully covered.
- Cloudflare deploy is covered on the happy path, but negative-path and containment checks are still missing.
- Backend daemon readiness and log noise still need hardening. Recent validation still surfaced stale-socket restart behavior and verbose backend log spill.

## Operating Notes For Primary

- Treat the repo as the memory system. Do not rely on thread-local context from this machine.
- Reuse canonical artifact roots only: `target/`, `logs/`, `tmp/`, `.molt_cache/`, `.uv-cache/`.
- Continue on `main` and push directly to `origin/main`.
- Do not trample partner-owned churn outside the files you intentionally touch.
- Prefer extending the shared artifact-contract layer over adding more wrapper-local heuristics.

## Canonical Detailed Handoff

For exact implementation notes, evidence, and the active roadmap, start with this handoff plus `src/molt/cli.py` and `tests/cli/test_cli_import_collection.py`, which now hold the canonical wrapper-contract behavior.
