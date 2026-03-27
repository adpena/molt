# Wrapper Artifact Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `run` and `deploy` consume one canonical build artifact contract instead of reconstructing output paths from wrapper-local heuristics.

**Architecture:** Extend build JSON for native and non-native outputs with semantic artifact roles, then route wrapper commands through one parser/helper that consumes that contract. Preserve existing build semantics while removing wrapper-side filename guessing for native, wasm, Luau, Roblox deploy, and Cloudflare split-runtime deploy.

**Tech Stack:** Python 3.12+, `pytest`, Molt CLI build JSON contract

---

### Task 1: Write The Failing Wrapper Contract Tests

**Files:**
- Modify: `tests/cli/test_cli_import_collection.py`

- [ ] **Step 1: Write the failing native run test**

Add a test that simulates a successful `molt build --json` returning a non-default native binary path and asserts `run_script()` executes that resolved binary path instead of recomputing `<output_base>_molt`.

- [ ] **Step 2: Run the native run test to verify it fails**

Run: `pytest -q tests/cli/test_cli_import_collection.py -k 'run_script_uses_build_json_output'`
Expected: FAIL because `run_script()` currently reconstructs the binary path locally.

- [ ] **Step 3: Write the failing cross-wasm test**

Add a test that simulates successful `build --json` with custom wasm output plus linked wasm metadata and asserts `_run_script_cross()` prefers the emitted runnable artifact.

- [ ] **Step 4: Run the cross-wasm test to verify it fails**

Run: `pytest -q tests/cli/test_cli_import_collection.py -k 'run_script_cross_wasm_honors_build_json_output'`
Expected: FAIL because `_run_script_cross()` currently guesses `<output_base>.wasm` and sibling `*_linked.wasm`.

- [ ] **Step 5: Write the failing deploy tests**

Add:
- Roblox deploy honoring emitted custom Luau output
- Cloudflare deploy honoring emitted bundle root / wrangler config

- [ ] **Step 6: Run the deploy tests to verify they fail**

Run: `pytest -q tests/cli/test_cli_import_collection.py -k 'deploy_roblox_honors_build_json_output or deploy_cloudflare_uses_build_json_bundle_root'`
Expected: FAIL because `_deploy()` currently reconstructs Luau paths and runs `wrangler` from `project_root`.

### Task 2: Implement The Canonical Build Artifact Contract

**Files:**
- Modify: `src/molt/cli.py`

- [ ] **Step 1: Add semantic artifact roles to build JSON**

Extend native and non-native build-success payloads with canonical fields for wrapper consumers, including at minimum:
- `consumer_output` for the executable/runnable artifact
- `bundle_root` when the deployable unit is a directory
- `artifacts` entries for target-specific roles like linked wasm, Luau output, Cloudflare `wrangler.toml`, `worker.js`, `app.wasm`, and `molt_runtime.wasm`

- [ ] **Step 2: Add one wrapper build-result parser/helper**

Create a helper that:
- forces `molt build --json`
- parses the build payload
- resolves semantic paths into typed `Path` values
- returns structured errors when JSON is missing or artifact roles are absent

- [ ] **Step 3: Route `run_script()` through the helper**

Consume emitted native `consumer_output` and use `_resolve_binary_output()` only as compatibility validation for the returned path.

- [ ] **Step 4: Route `_run_script_cross()` through the helper**

Consume emitted wasm/Luau artifact roles instead of reconstructing filenames from `output_base`.

- [ ] **Step 5: Route `_deploy()` through the helper**

Use emitted Luau output for Roblox and emitted bundle root / wrangler config for Cloudflare.

### Task 3: Verify The Contract End-To-End

**Files:**
- Modify: `tests/cli/test_cli_import_collection.py`
- Verify only: `tests/differential/stdlib/*`, `examples/hello.py`

- [ ] **Step 1: Run the focused CLI wrapper tests**

Run: `pytest -q tests/cli/test_cli_import_collection.py -k 'build_json_output or bundle_root or generated_importer or prepare_entry_module_graph_marks'`
Expected: PASS

- [ ] **Step 2: Run the broader CLI regression slice**

Run: `pytest -q tests/cli/test_cli_import_collection.py -k 'run_script or run_script_cross or deploy or generated_importer or augment_support_modules or prepare_entry_module_graph_marks'`
Expected: PASS

- [ ] **Step 3: Run differential control cases with RSS enabled**

Run:
`MOLT_DIFF_MEASURE_RSS=1 python3 -u tests/molt_diff.py --build-profile dev --jobs 1 tests/differential/stdlib/importlib_support_bootstrap.py tests/differential/stdlib/importlib_import_module_helper_constant.py`
Expected: PASS

- [ ] **Step 4: Run one static build diagnostics probe**

Run:
`python3 -m molt.cli build --build-profile dev --out-dir tmp/static_hello_build --diagnostics --diagnostics-file logs/importlib_runtime_support_20260327/hello_build_diagnostics.json examples/hello.py`
Expected: PASS

