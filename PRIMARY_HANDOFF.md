# Primary Handoff

This file is the canonical fast resume point for continuing Molt development on `main`.

Last updated: 2026-04-20

## Current State

- Repo: `/Users/adpena/Projects/molt`
- Branch: `main`
- Current HEAD at handoff time: `2fa35f83`
- Remote status observed: `HEAD -> main, origin/main, origin/HEAD`
- Active local work at handoff time: this handoff consolidation file is staged.
- Active implementation lane: Molt GPU/tinygrad/Falcon-OCR WASM compilation, Cloudflare deployment, and enjoice OCR integration.
- Disk pressure has been remediated enough to continue implementation work.

Recent commits in the active lane:

```text
2fa35f83 docs: update SUPPORT_TEAM.md - WASM compilation works, remaining items
f708fc72 feat: Falcon-OCR WASM compilation succeeds (13.4 MB, 4 MB gzipped)
0b20e15c fix: eliminate dead blocks after SCCP branch folding to prevent false SSA dominance violations
ac875a30 feat: register GPU primitive intrinsics for WASM compilation
1bc24a58 fix: restore x402 enforcement on Workers AI fast path
```

## Canonical Handoff Policy

- `PRIMARY_HANDOFF.md` is the active consolidated resume point.
- `SUPPORT_TEAM.md` is detailed source context for the parallel WASM/Falcon lane, not a competing primary handoff.
- Older Downloads handoffs are archival unless the user explicitly asks to resume one:
  - `/Users/adpena/Downloads/MOLT_HANDOFF_20260402.md`
  - `/Users/adpena/Downloads/MOLT_HANDOFF_20260403.md`
  - `/Users/adpena/Downloads/MOLT_SPRINT_HANDOFF.md`
  - `/Users/adpena/Downloads/molt-handoff-20260402-opus-sprint.md`
  - `/Users/adpena/Downloads/SESSION_HANDOFF_20260414.md`
  - `/Users/adpena/Downloads/SESSION_HANDOFF_20260415.md`
  - `/Users/adpena/Downloads/SESSION_HANDOFF_20260415_v2.md`
- New handoff material should be merged here or referenced from here. Do not create another root-level handoff file unless the user asks for a separate lane-specific artifact.

## Active Implementation Context

The current work is no longer primarily disk cleanup. The main product lane is:

1. `runtime/molt-gpu/`
   - Current local size: about `944K`.
   - Local file count observed: `64`.
   - Rust/TOML LOC observed: `23181 total`.
   - Contains GPU primitive/render/backend substrate.

2. `src/molt/stdlib/tinygrad/`
   - Current local size: about `412K`.
   - Local file count observed: `29`.
   - Python LOC observed: `10402 total`.
   - Contains the tinygrad-facing Tensor/Falcon-OCR/WASM driver surface.

3. `runtime/molt-runtime/src/builtins/gpu_primitives.rs`
   - Present locally.
   - Size observed: about `19K`.
   - Bridge for exposing `molt-gpu` operations to runtime/builtin paths.

4. `src/molt/stdlib/tinygrad/wasm_driver.py`
   - Present locally.
   - Exports the intended WASM API surface:
     - `init(...)`
     - `ocr_tokens(...)`
   - Delegation contract is tested in `tests/e2e/test_deployment.py`.

5. `deploy/cloudflare/`
   - Worker/API path is active.
   - `deploy/cloudflare/worker.js` currently looks for R2 object `models/falcon-ocr/falcon-ocr.wasm`.
   - Workers AI fallback and x402 payment code live in this tree.

6. `docs/integration/enjoice-ocr-migration.md`
   - Integration guide for moving enjoice from the older browser-module/manifest flow to direct Molt WASM loading.

## Current Claims To Verify Before Building Further

`SUPPORT_TEAM.md` says:

- `MOLT_HERMETIC_MODULE_ROOTS=1 molt build wasm_driver.py --target wasm` succeeds.
- The Falcon-OCR WASM binary is `13.4 MB` linked and `4.0 MB` gzipped.
- The binary was uploaded to R2 at `models/falcon-ocr/falcon-ocr.wasm`.
- Remaining work:
  - fix WASM linker import resolution by original name, not alias
  - test that the WASM binary actually runs and loads weights
  - optimize binary size toward `< 2 MB` gzipped
  - wire into browser WebGPU/offline inference path

These claims are current handoff context, but the R2 object and runtime execution were not independently verified during this handoff consolidation. Verify before making deployment claims.

## Highest Priority Next Work

1. Verify the WASM artifact path end to end.
   - Rebuild `wasm_driver.py` from the current repo.
   - Confirm output size and export surface.
   - Confirm import alias/linker behavior.

2. Prove the WASM binary runs.
   - Load the module.
   - Load weights/config.
   - Call `init`.
   - Call `ocr_tokens` with a minimal deterministic image/prompt.
   - Record exact output/error shape.

3. Reconcile deployment docs.
   - `SUPPORT_TEAM.md` says the full Falcon-OCR WASM build now succeeds.
   - `docs/deployment/DEPLOYMENT_LOG.md` still contains older "WASM binary not found / not yet built" blocker text from 2026-04-14.
   - Consolidate docs so there is one current deployment truth.

4. Continue enjoice integration work.
   - Use `docs/integration/enjoice-ocr-migration.md` and `deploy/enjoice/INTEGRATION_PR.md`.
   - Keep PaddleOCR and server OCR fallback unchanged unless the user explicitly changes scope.

5. Keep `molt clean` and disk hygiene working as implementation support.
   - The cleanup command is important because this lane creates large Cargo/WASM/model artifacts.
   - Treat cleanup as operational support, not the product objective.

## Suggested Verification Commands

Start with canonical env:

```bash
cd /Users/adpena/Projects/molt

export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
```

Inspect local state:

```bash
git status --short
git log -5 --oneline --decorate
```

Run focused contract tests:

```bash
UV_NO_SYNC=1 UV_CACHE_DIR=$PWD/.uv-cache TMPDIR=$PWD/tmp \
  uv run --python 3.12 python3 -m pytest -q \
  tests/e2e/test_deployment.py \
  tests/test_gpu_turboquant.py \
  tests/test_gpu_kv_cache.py \
  tests/test_gpu_generate.py
```

Check runtime WASM compilation health:

```bash
export MOLT_SESSION_ID="wasm-verify"
export CARGO_TARGET_DIR=$PWD/target

cargo check -p molt-runtime --target wasm32-wasip1
```

Try the Falcon-OCR WASM build:

```bash
MOLT_HERMETIC_MODULE_ROOTS=1 \
PYTHONPATH=src \
UV_NO_SYNC=1 \
UV_CACHE_DIR=$PWD/.uv-cache \
TMPDIR=$PWD/tmp \
uv run --python 3.12 python3 -m molt.cli build \
  --target wasm \
  --profile dev \
  --out-dir tmp/falcon_ocr_wasm_verify \
  src/molt/stdlib/tinygrad/wasm_driver.py
```

If this succeeds, inspect artifact size and exports. If it fails, capture logs under `logs/` or `tmp/` and update this handoff with exact errors.

## Key Files

- `SUPPORT_TEAM.md`
- `runtime/molt-gpu/`
- `src/molt/stdlib/tinygrad/`
- `src/molt/stdlib/tinygrad/wasm_driver.py`
- `src/molt/stdlib/tinygrad/wasm_manifest.json`
- `runtime/molt-runtime/src/builtins/gpu_primitives.rs`
- `deploy/cloudflare/worker.js`
- `deploy/cloudflare/ocr_api.js`
- `deploy/cloudflare/ai-fallback.js`
- `deploy/cloudflare/x402.js`
- `deploy/cloudflare/DEPLOYMENT_LOG.md`
- `docs/deployment/DEPLOYMENT_LOG.md`
- `docs/integration/enjoice-ocr-migration.md`
- `deploy/enjoice/INTEGRATION_PR.md`
- `tests/e2e/test_deployment.py`

## Disk And Process State

Disk cleanup was performed before this consolidation because the machine was effectively full.

Important cleanup facts:

- Original pressure:
  - `/System/Volumes/Data`: about `123Mi` available on a `1.8Ti` volume
  - `/Users/adpena/Projects/molt`: about `1.0T`
  - `/Users/adpena/Projects/molt/tmp`: about `947G`
- After cleanup and subsequent verification:
  - `/System/Volumes/Data`: about `543Gi` used, about `1.2Ti` available, about `30%` full
  - `/Users/adpena/Projects`: about `181G`
  - `/Users/adpena/Projects/molt`: about `7.5G`
  - `/Users/adpena/Projects/chodex`: about `854M`
  - `/Users/adpena/Library/Caches`: about `14G`

Removed:

- stale `molt/tmp` scratch/build trees
- root and nested Cargo targets that were safe to delete
- old `chodex` Rust target directories
- `/Users/adpena/Projects/.repo-handoff-backups`
- large Codex Sparkle cache, though it later began recreating

Killed:

- all `rbx-studio-mcp` processes

Observed after verification:

- no `rbx-studio-mcp` process remained
- a Molt backend daemon existed under `target/release-fast/...`
- several old `codex --yolo` sessions still existed and were intentionally not killed

## `molt clean` Status

The current HEAD already contains the `molt clean` repair:

- plain `molt clean` has `--scratch` enabled by default
- repo scratch/cache roots include `tmp/`, `.uv-cache*`, `.molt_cache*`, `.pytest_cache`, `.ruff_cache`, `.mypy_cache`, and `__pycache__`
- `--cargo-target` includes root `target/`, legacy `target-*`, and top-level nested workspace `*/target`
- `--repo-artifacts` does not remove tracked `vendor/`

Focused verification already run in this session:

```text
tests/cli/test_cli_import_collection.py -k 'clean_'
3 passed, 306 deselected, 1 warning

ruff check src/molt/cli.py tests/cli/test_cli_import_collection.py
All checks passed!
```

Use this after large verification/build runs if scratch output starts growing again:

```bash
PYTHONPATH=src .venv/bin/python -m molt.cli clean \
  --no-cache --no-artifacts --scratch --no-bins \
  --no-repo-artifacts --no-cargo-target --json
```

Use this for heavier Rust artifact cleanup:

```bash
PYTHONPATH=src .venv/bin/python -m molt.cli clean \
  --no-cache --no-artifacts --no-scratch --no-bins \
  --no-repo-artifacts --cargo-target --json
```

## Documentation Consolidation Work Still Needed

1. Keep `PRIMARY_HANDOFF.md` as the only active root handoff.
2. Update `SUPPORT_TEAM.md` to point here as the canonical summary while retaining detailed parallel-lane context.
3. Reconcile these deployment documents:
   - `SUPPORT_TEAM.md`
   - `deploy/cloudflare/DEPLOYMENT_LOG.md`
   - `docs/deployment/DEPLOYMENT_LOG.md`
   - `docs/integration/enjoice-ocr-migration.md`
   - `deploy/enjoice/INTEGRATION_PR.md`
4. Archive or explicitly mark stale the old Downloads handoffs if the user wants a filesystem-level cleanup of handoff docs.

## Guardrails For Next Agent

- Do not fake DFlash support. Preserve target-conditioned drafter/verifier/KV contracts and raise missing trained-drafter limitations explicitly.
- Do not implement Python stdlib behavior without Rust intrinsics.
- Do not use CPython fallback for compiled binaries.
- Keep native and WASM same-contract parity as the goal.
- For Cloudflare/enjoice work, do not claim deployment state without checking the Worker/R2 path.
- For model/WASM work, distinguish:
  - compiles to WASM
  - uploads to R2
  - instantiates
  - loads weights/config
  - returns plausible tokens
  - integrated in browser/enjoice
- These are separate proof points.

## Historical Context

The previous `PRIMARY_HANDOFF.md` content described a wrapper/build-contract slice audited on 2026-03-30. That is no longer the active resume point. Use git history for that old content if needed.
