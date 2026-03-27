# Wrapper Artifact Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `run` and `deploy` consume one canonical build artifact contract instead of reconstructing output paths from wrapper-local heuristics.

**Architecture:** Extend build JSON for native and non-native outputs with semantic artifact roles, then route wrapper commands through one parser/helper that consumes that contract. Preserve existing build semantics while removing wrapper-side filename guessing for native, wasm, Luau, Roblox deploy, and Cloudflare split-runtime deploy.

**Tech Stack:** Python 3.12+, `pytest`, Molt CLI build JSON contract

## Status

- Completed on `main` at commit `3fb765c2`
- Local `main` was clean and in sync with `origin/main` during handoff preparation
- Transition note: this document is the canonical handoff artifact for continuing the CLI/build-contract hardening work on `Primary`

## Outcome Summary

- `run`, cross-target `run`, and `deploy` now consume one canonical build artifact contract instead of reconstructing output paths locally.
- Build JSON now carries additive semantic fields for wrapper consumers:
  - `consumer_output`
  - `bundle_root`
  - target-specific `artifacts`
  - `messages` for success-path signal replay
- Native and non-native wrappers replay nested build warnings, success messages, diagnostics, and failure detail in non-JSON mode instead of silently swallowing signal.
- Luau artifacts are emitted with the correct semantic role instead of being folded through the Rust-transpile branch.
- Cloudflare split-runtime deploy now uses emitted `bundle_root` and `wrangler_config`.

## Verified Results

- `python3 -m py_compile src/molt/cli.py tests/cli/test_cli_import_collection.py`
- `./.venv/bin/ruff check src/molt/cli.py tests/cli/test_cli_import_collection.py`
- `./.venv/bin/python -m pytest -q tests/cli/test_cli_import_collection.py`
  - Result: `237 passed, 3 skipped`
- `python3 -m molt.cli run --profile dev examples/hello.py`
  - Result: nested build warning/success output surfaced, then program printed `42`
- `MOLT_DIFF_MEASURE_RSS=1 ./.venv/bin/python -u tests/molt_diff.py --build-profile dev --jobs 1 tests/differential/stdlib/importlib_support_bootstrap.py tests/differential/stdlib/importlib_import_module_helper_constant.py`
  - Result: both passed
  - Latest RSS entries:
    - `importlib_import_module_helper_constant.py`: build `374320 KB`, run `13616 KB`
    - `importlib_support_bootstrap.py`: build `379264 KB`, run `13424 KB`
- `python3 -m molt.cli build --build-profile dev --out-dir tmp/static_hello_build --diagnostics --diagnostics-file logs/importlib_runtime_support_20260327/hello_build_diagnostics.json examples/hello.py`
  - Result: passed
  - Output binary: `tmp/static_hello_build/hello_molt`
  - Diagnostics artifact: `tmp/static_hello_build/.molt_build/hello/logs/importlib_runtime_support_20260327/hello_build_diagnostics.json`

---

### Task 1: Write The Failing Wrapper Contract Tests

**Files:**
- Modify: `tests/cli/test_cli_import_collection.py`

- [x] **Step 1: Write the failing native run test**

Add a test that simulates a successful `molt build --json` returning a non-default native binary path and asserts `run_script()` executes that resolved binary path instead of recomputing `<output_base>_molt`.

- [x] **Step 2: Run the native run test to verify it fails**

Run: `pytest -q tests/cli/test_cli_import_collection.py -k 'run_script_uses_build_json_output'`
Expected: FAIL because `run_script()` currently reconstructs the binary path locally.

- [x] **Step 3: Write the failing cross-wasm test**

Add a test that simulates successful `build --json` with custom wasm output plus linked wasm metadata and asserts `_run_script_cross()` prefers the emitted runnable artifact.

- [x] **Step 4: Run the cross-wasm test to verify it fails**

Run: `pytest -q tests/cli/test_cli_import_collection.py -k 'run_script_cross_wasm_honors_build_json_output'`
Expected: FAIL because `_run_script_cross()` currently guesses `<output_base>.wasm` and sibling `*_linked.wasm`.

- [x] **Step 5: Write the failing deploy tests**

Add:
- Roblox deploy honoring emitted custom Luau output
- Cloudflare deploy honoring emitted bundle root / wrangler config

- [x] **Step 6: Run the deploy tests to verify they fail**

Run: `pytest -q tests/cli/test_cli_import_collection.py -k 'deploy_roblox_honors_build_json_output or deploy_cloudflare_uses_build_json_bundle_root'`
Expected: FAIL because `_deploy()` currently reconstructs Luau paths and runs `wrangler` from `project_root`.

### Task 2: Implement The Canonical Build Artifact Contract

**Files:**
- Modify: `src/molt/cli.py`

- [x] **Step 1: Add semantic artifact roles to build JSON**

Extend native and non-native build-success payloads with canonical fields for wrapper consumers, including at minimum:
- `consumer_output` for the executable/runnable artifact
- `bundle_root` when the deployable unit is a directory
- `artifacts` entries for target-specific roles like linked wasm, Luau output, Cloudflare `wrangler.toml`, `worker.js`, `app.wasm`, and `molt_runtime.wasm`

- [x] **Step 2: Add one wrapper build-result parser/helper**

Create a helper that:
- forces `molt build --json`
- parses the build payload
- resolves semantic paths into typed `Path` values
- returns structured errors when JSON is missing or artifact roles are absent

- [x] **Step 3: Route `run_script()` through the helper**

Consume emitted native `consumer_output` and use `_resolve_binary_output()` only as compatibility validation for the returned path.

- [x] **Step 4: Route `_run_script_cross()` through the helper**

Consume emitted wasm/Luau artifact roles instead of reconstructing filenames from `output_base`.

- [x] **Step 5: Route `_deploy()` through the helper**

Use emitted Luau output for Roblox and emitted bundle root / wrangler config for Cloudflare.

### Task 3: Verify The Contract End-To-End

**Files:**
- Modify: `tests/cli/test_cli_import_collection.py`
- Verify only: `tests/differential/stdlib/*`, `examples/hello.py`

- [x] **Step 1: Run the focused CLI wrapper tests**

Run: `pytest -q tests/cli/test_cli_import_collection.py -k 'build_json_output or bundle_root or generated_importer or prepare_entry_module_graph_marks'`
Expected: PASS

- [x] **Step 2: Run the broader CLI regression slice**

Run: `pytest -q tests/cli/test_cli_import_collection.py -k 'run_script or run_script_cross or deploy or generated_importer or augment_support_modules or prepare_entry_module_graph_marks'`
Expected: PASS

- [x] **Step 3: Run differential control cases with RSS enabled**

Run:
`MOLT_DIFF_MEASURE_RSS=1 python3 -u tests/molt_diff.py --build-profile dev --jobs 1 tests/differential/stdlib/importlib_support_bootstrap.py tests/differential/stdlib/importlib_import_module_helper_constant.py`
Expected: PASS

- [x] **Step 4: Run one static build diagnostics probe**

Run:
`python3 -m molt.cli build --build-profile dev --out-dir tmp/static_hello_build --diagnostics --diagnostics-file logs/importlib_runtime_support_20260327/hello_build_diagnostics.json examples/hello.py`
Expected: PASS

## Remaining Work / Next Recommended Task

The highest-leverage next slice on `Primary` is to extend this same contract one layer further instead of creating more wrapper-local logic:

1. Unify `compare()` onto the shared artifact contract.
   - Current drift: `compare()` still manually parses build JSON and still keys off `data.output` instead of `consumer_output`.
   - Recommended shape: factor the shared execution/parsing path below `_run_wrapper_build()` so `compare()` can reuse the contract without inheriting wrapper-specific success-message replay unless explicitly desired.

2. Expand cross-platform contract tests.
   - Add Windows `.exe` cases for both `run` and `compare`.
   - Add wasm explicit `--output` and unlinked `consumer_output` cases.
   - Add Luau explicit `--output` on direct `run`.

3. Harden Cloudflare negative-path validation.
   - Missing `bundle_root`
   - Missing `wrangler_config`
   - `wrangler_config` outside `bundle_root`
   - split-runtime explicit `--output` / `--out-dir`

4. Normalize successful JSON UX across cross-run and deploy.
   - Today native `run` has a cleaner structured success path than cross-target `run` / deploy.

5. Harden daemon UX and logging.
   - The latest verification still surfaced stale-socket restart and noisy backend log spill.
   - Next pass should focus on readiness/backoff, quiet-by-default daemon logs, and structured debug-only detail.
