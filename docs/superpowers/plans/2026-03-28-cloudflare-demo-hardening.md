# Cloudflare Demo Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Cloudflare demo artifact contract compatible with the latest toolchains, harden and fuzz all variable-bearing endpoints, and gate production deploys on live verification against the real Cloudflare worker.

**Architecture:** Update the Cloudflare split-runtime artifact contract in `molt build`, add deterministic local and production verification layers that consume that contract, then harden the demo endpoint surface until CPython, local Cloudflare execution, and production live deployment agree. Keep the deploy flow evidence-first: build artifact metadata, local validation results, deploy logs, and live endpoint sweep output all live under canonical artifact roots.

**Tech Stack:** Python CLI, pytest, Wrangler 4+, Cloudflare Workers module-worker contract, split-runtime WASM bundle generation, curl/HTTP verification tooling.

---

### Task 1: Lock the Cloudflare artifact contract to the latest Wrangler model

**Files:**
- Modify: `src/molt/cli.py`
- Modify: `tests/test_wasm_split_runtime.py`
- Modify: `tests/cli/test_cli_import_collection.py`
- Modify: `examples/cloudflare-demo/README.md`
- Modify: `docs/cli-reference.md`

- [ ] **Step 1: Write failing artifact-contract tests**

Add tests covering:
- split-runtime build emits latest-toolchain Cloudflare config instead of legacy `[wasm_modules]`;
- emitted worker/config metadata includes enough information for local dev and deploy consumers;
- Cloudflare deploy still consumes `bundle_root` and `wrangler_config` from build JSON.

- [ ] **Step 2: Run the targeted tests to verify they fail**

Run:
```bash
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
python3 -m pytest -q tests/test_wasm_split_runtime.py tests/cli/test_cli_import_collection.py -k 'cloudflare or split_runtime'
```

Expected: FAIL on legacy Cloudflare config/output assumptions.

- [ ] **Step 3: Implement the artifact-contract update in `src/molt/cli.py`**

Make the minimal build output changes required to:
- emit current-toolchain Cloudflare config for module workers;
- keep `bundle_root` and `wrangler_config` canonical in build JSON;
- expose any additional metadata needed by verification tooling without duplicating path logic elsewhere.

- [ ] **Step 4: Update split-runtime output tests and docs**

Adjust tests and docs so they assert the new contract, not the legacy one.

- [ ] **Step 5: Re-run the targeted tests**

Run:
```bash
python3 -m pytest -q tests/test_wasm_split_runtime.py tests/cli/test_cli_import_collection.py -k 'cloudflare or split_runtime'
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/molt/cli.py tests/test_wasm_split_runtime.py tests/cli/test_cli_import_collection.py examples/cloudflare-demo/README.md docs/cli-reference.md
git commit -m "fix: update cloudflare split-runtime artifact contract"
```

### Task 2: Add local Cloudflare bundle validation on current toolchains

**Files:**
- Create: `tools/cloudflare_demo_verify.py`
- Modify: `tests/test_wasm_split_runtime.py`
- Modify: `tests/cli/test_cli_import_collection.py`
- Modify: `docs/OPERATIONS.md`

- [ ] **Step 1: Write failing tests for local bundle validation plumbing**

Add tests that require:
- a validation helper to consume generated `bundle_root`;
- deterministic log/result output under `logs/` and `tmp/`;
- explicit failure when Wrangler/local validation rejects the generated bundle.

- [ ] **Step 2: Run the targeted tests to verify they fail**

Run:
```bash
python3 -m pytest -q tests/test_wasm_split_runtime.py tests/cli/test_cli_import_collection.py -k 'cloudflare and validate'
```

Expected: FAIL because no local validation tool/contract exists yet.

- [ ] **Step 3: Implement `tools/cloudflare_demo_verify.py` local validation mode**

Implement a helper that:
- accepts a build artifact root;
- validates the generated Cloudflare config/layout;
- launches local validation with the latest Wrangler/toolchain contract;
- writes machine-readable and human-readable results under canonical roots.

- [ ] **Step 4: Re-run the targeted tests**

Run:
```bash
python3 -m pytest -q tests/test_wasm_split_runtime.py tests/cli/test_cli_import_collection.py -k 'cloudflare and validate'
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tools/cloudflare_demo_verify.py tests/test_wasm_split_runtime.py tests/cli/test_cli_import_collection.py docs/OPERATIONS.md
git commit -m "test: add local cloudflare bundle validation tooling"
```

### Task 3: Encode the documented endpoint matrix as executable tests

**Files:**
- Create: `tests/cloudflare/test_demo_endpoints.py`
- Modify: `examples/cloudflare-demo/src/app.py`
- Modify: `examples/cloudflare-demo/README.md`

- [ ] **Step 1: Write failing endpoint matrix tests against the current documented surface**

Cover at least:
- `/`
- `/fib/N`
- `/primes/N`
- `/diamond/N`
- `/mandelbrot`
- `/sort?...`
- `/fizzbuzz/N`
- `/pi/N`
- `/generate/N`
- `/bench`
- `/sql`
- `/demo`

Assert status/body sentinels/content-type expectations at the app-contract level.

- [ ] **Step 2: Run the endpoint tests against direct source execution**

Run:
```bash
python3 -m pytest -q tests/cloudflare/test_demo_endpoints.py
```

Expected: FAIL initially until the fixture/contract is complete.

- [ ] **Step 3: Implement the minimal app/test harness changes**

Keep changes tightly scoped to:
- documented route behavior;
- deterministic output shape;
- clear rejection behavior for unsupported/malformed inputs.

- [ ] **Step 4: Re-run the endpoint tests**

Run:
```bash
python3 -m pytest -q tests/cloudflare/test_demo_endpoints.py
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tests/cloudflare/test_demo_endpoints.py examples/cloudflare-demo/src/app.py examples/cloudflare-demo/README.md
git commit -m "test: encode cloudflare demo endpoint contract"
```

### Task 4: Harden numeric and query-driven endpoints with adversarial tests

**Files:**
- Create: `tests/cloudflare/test_demo_endpoint_fuzz.py`
- Modify: `examples/cloudflare-demo/src/app.py`
- Modify: `docs/SECURITY.md`

- [ ] **Step 1: Write failing adversarial input tests for variable-bearing endpoints**

Cover:
- path-number overflow/underflow/bad-type cases for `/fib`, `/primes`, `/diamond`, `/fizzbuzz`, `/pi`, `/generate`;
- malformed/oversized query input for `/sort` and `/sql`;
- delimiter abuse, empty segments, repeated separators, truncation, and bad percent-decoding at parser boundaries;
- assertions for bounded, deterministic, crash-free behavior.

- [ ] **Step 2: Run the fuzz-oriented endpoint tests to verify they fail**

Run:
```bash
python3 -m pytest -q tests/cloudflare/test_demo_endpoint_fuzz.py
```

Expected: FAIL on current un-hardened edge cases.

- [ ] **Step 3: Implement the minimal hardening in `examples/cloudflare-demo/src/app.py`**

Add:
- explicit validation and clamping where appropriate;
- fail-closed handling for malformed parser inputs;
- output-integrity protections so rejected inputs never produce partial/malformed responses.

- [ ] **Step 4: Re-run the fuzz-oriented tests**

Run:
```bash
python3 -m pytest -q tests/cloudflare/test_demo_endpoint_fuzz.py
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tests/cloudflare/test_demo_endpoint_fuzz.py examples/cloudflare-demo/src/app.py docs/SECURITY.md
git commit -m "fix: harden cloudflare demo variable input endpoints"
```

### Task 5: Fix malformed output and route/runtime drift in generated execution

**Files:**
- Modify: `src/molt/cli.py`
- Modify: `tests/cloudflare/test_demo_endpoints.py`
- Modify: `tests/cloudflare/test_demo_endpoint_fuzz.py`
- Modify: `tests/test_wasm_split_runtime.py`

- [ ] **Step 1: Write failing tests for generated-bundle output integrity**

Add coverage that fails on:
- NUL-prefixed text output;
- stale-route mismatches between source expectations and generated execution;
- runtime error placeholders such as plain `Error`/partial output in supported routes.

- [ ] **Step 2: Run the targeted tests to verify they fail**

Run:
```bash
python3 -m pytest -q tests/cloudflare/test_demo_endpoints.py tests/cloudflare/test_demo_endpoint_fuzz.py tests/test_wasm_split_runtime.py -k 'output or generated or cloudflare'
```

Expected: FAIL with current generated behavior mismatches.

- [ ] **Step 3: Implement the minimal generator/runtime fixes**

Focus on:
- generated worker output handling;
- route argument propagation;
- response/body normalization;
- removing corruption paths rather than masking them.

- [ ] **Step 4: Re-run the targeted tests**

Run:
```bash
python3 -m pytest -q tests/cloudflare/test_demo_endpoints.py tests/cloudflare/test_demo_endpoint_fuzz.py tests/test_wasm_split_runtime.py -k 'output or generated or cloudflare'
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/molt/cli.py tests/cloudflare/test_demo_endpoints.py tests/cloudflare/test_demo_endpoint_fuzz.py tests/test_wasm_split_runtime.py
git commit -m "fix: harden generated cloudflare demo runtime output"
```

### Task 6: Add production deploy verification against the real Cloudflare worker

**Files:**
- Create: `tools/cloudflare_demo_deploy_verify.py`
- Create: `tests/cloudflare/test_live_verifier.py`
- Modify: `src/molt/cli.py`
- Modify: `docs/OPERATIONS.md`
- Modify: `examples/cloudflare-demo/README.md`

- [ ] **Step 1: Write failing tests for live verification contract**

Require a deploy verification tool that:
- consumes the built Cloudflare artifact contract;
- runs post-deploy endpoint checks against a configured live base URL;
- fails on stale behavior, `1102`, malformed output, or route regressions;
- writes canonical logs/results.

- [ ] **Step 2: Run the targeted tests to verify they fail**

Run:
```bash
python3 -m pytest -q tests/cloudflare/test_live_verifier.py
```

Expected: FAIL because live verification tooling does not exist yet.

- [ ] **Step 3: Implement production deploy verification tooling**

Implement:
- authenticated deploy execution path using the exact generated artifact set;
- post-deploy live endpoint sweep;
- machine-readable summary suitable for gating production success.

- [ ] **Step 4: Re-run the targeted tests**

Run:
```bash
python3 -m pytest -q tests/cloudflare/test_live_verifier.py
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tools/cloudflare_demo_deploy_verify.py tests/cloudflare/test_live_verifier.py src/molt/cli.py docs/OPERATIONS.md examples/cloudflare-demo/README.md
git commit -m "feat: gate cloudflare deploys on live production verification"
```

### Task 7: Run the full local verification matrix

**Files:**
- Modify as needed from prior tasks only

- [ ] **Step 1: Run the focused Cloudflare test matrix**

Run:
```bash
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
python3 -m pytest -q \
  tests/cli/test_cli_import_collection.py \
  tests/test_wasm_split_runtime.py \
  tests/cloudflare/test_demo_endpoints.py \
  tests/cloudflare/test_demo_endpoint_fuzz.py \
  tests/cloudflare/test_live_verifier.py
```

Expected: PASS.

- [ ] **Step 2: Build the demo bundle with canonical artifact roots**

Run:
```bash
python3 -m molt.cli build examples/cloudflare-demo/src/app.py \
  --target wasm --profile cloudflare --split-runtime \
  --output tmp/cloudflare-demo/output.wasm \
  --linked-output tmp/cloudflare-demo/worker_linked.wasm
```

Expected: PASS with current-toolchain-compatible Cloudflare artifact output.

- [ ] **Step 3: Run local verification tooling**

Run:
```bash
python3 tools/cloudflare_demo_verify.py \
  --bundle-root tmp/cloudflare-demo \
  --base-url http://127.0.0.1:8789
```

Expected: PASS and write logs/results under `logs/` and `tmp/`.

- [ ] **Step 4: Commit verification-only doc or harness adjustments if required**

```bash
git add docs/OPERATIONS.md examples/cloudflare-demo/README.md
git commit -m "docs: finalize cloudflare demo verification workflow"
```

### Task 8: Run real production deploy verification

**Files:**
- No new files unless evidence/log path fixes are needed

- [ ] **Step 1: Run the production deploy-and-verify flow**

Run:
```bash
python3 tools/cloudflare_demo_deploy_verify.py \
  --entry examples/cloudflare-demo/src/app.py \
  --live-base-url https://molt-python-demo.adpena.workers.dev \
  --artifact-root logs/cloudflare_demo_$(date +%Y%m%d_%H%M%S)
```

Expected: PASS. Production worker serves the expected endpoint matrix with no stale behavior, no malformed output, and no `1102` failures.

- [ ] **Step 2: Capture the final evidence summary**

Summarize:
- build artifact identity;
- deploy result;
- live endpoint sweep result;
- any bounds/fuzz coverage evidence generated by the tooling.

- [ ] **Step 3: Final commit**

```bash
git add src/molt/cli.py tests/cli/test_cli_import_collection.py tests/test_wasm_split_runtime.py tests/cloudflare tools/cloudflare_demo_verify.py tools/cloudflare_demo_deploy_verify.py docs/OPERATIONS.md docs/cli-reference.md docs/SECURITY.md examples/cloudflare-demo/README.md examples/cloudflare-demo/src/app.py
git commit -m "fix: harden cloudflare demo build deploy and endpoint verification"
```
