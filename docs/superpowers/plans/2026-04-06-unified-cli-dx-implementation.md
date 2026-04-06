# Unified CLI DX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `molt setup`, `molt doctor`, and `molt validate` the single
cross-platform authority for bootstrap, diagnostics, and end-to-end validation.

**Architecture:** Consolidate behavior into `src/molt/cli.py`, reduce scripts
and packaging entrypoints to thin delegates, and encode the full local release
matrix in a first-party `validate` command with canonical artifact output.

**Tech Stack:** Python CLI (`src/molt/cli.py`), packaging shell/PowerShell
wrappers, existing Rust/Python test suites, canonical docs under `docs/`

---

## File Map

| Path | Responsibility |
| --- | --- |
| `src/molt/cli.py` | implement `setup` and `validate`, tighten `doctor`, and centralize env/toolchain logic |
| `tools/dev.py` | delegate to canonical CLI flows |
| `packaging/install.sh` | bootstrap + delegate to `molt setup` |
| `packaging/install.ps1` | bootstrap + delegate to `molt setup` |
| `packaging/config.toml` | metadata only; update comments if needed |
| `tests/cli/` | CLI regressions for setup/doctor/validate and wrapper behavior |
| `docs/DEVELOPER_GUIDE.md` | developer workflow docs |
| `docs/OPERATIONS.md` | operational validation workflow docs |
| `CONTRIBUTING.md` | contributor command guidance |
| `docs/spec/STATUS.md` | current-state status |
| `ROADMAP.md` | forward-looking DX follow-up notes |

## Task 1: Add failing CLI coverage for the canonical DX contract

**Files:**
- Modify: `tests/cli/test_cli_smoke.py`
- Modify: `tests/cli/test_cli_import_collection.py`
- Create: `tests/cli/test_cli_setup_validate.py`

- [ ] **Step 1: Write failing tests for `molt setup` and `molt validate` discovery**

Cover:

- `molt setup --help` exists;
- `molt validate --help` exists;
- `molt doctor --json` still reports canonical artifact/env fields;
- `tools/dev.py doctor` and `tools/dev.py update` remain usable during the cutover.

- [ ] **Step 2: Run the new CLI tests to verify they fail**

Run:

```bash
export MOLT_SESSION_ID=dx-cli-tests-1
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
PYTHONPATH=src ./.venv/bin/python -m pytest -q tests/cli/test_cli_setup_validate.py tests/cli/test_cli_smoke.py tests/cli/test_cli_import_collection.py
```

Expected: FAIL because `setup` / `validate` are not yet implemented as the
canonical surface.

- [ ] **Step 3: Commit**

```bash
git add tests/cli/test_cli_smoke.py tests/cli/test_cli_import_collection.py tests/cli/test_cli_setup_validate.py
git commit -m "Lock unified CLI DX contract with failing coverage"
```

## Task 2: Implement canonical `molt setup` and tighten `molt doctor`

**Files:**
- Modify: `src/molt/cli.py`
- Test: `tests/cli/test_cli_setup_validate.py`

- [ ] **Step 1: Add a shared toolchain/env readiness model in `cli.py`**

Implement reusable helpers for:

- required vs optional tool classification;
- backend/target/profile readiness;
- canonical env resolution;
- remediation rows for human + JSON output.

- [ ] **Step 2: Implement `molt setup`**

Support:

- dry detection and reporting;
- optional install guidance mode;
- JSON output;
- canonical env/config guidance.

- [ ] **Step 3: Tighten `molt doctor` around the same shared model**

Make sure `doctor` and `setup` share logic rather than duplicating checks.

- [ ] **Step 4: Run the focused CLI tests**

Run:

```bash
export MOLT_SESSION_ID=dx-cli-setup-2
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
PYTHONPATH=src ./.venv/bin/python -m pytest -q tests/cli/test_cli_setup_validate.py
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/molt/cli.py tests/cli/test_cli_setup_validate.py
git commit -m "Make setup and doctor the canonical readiness surface"
```

## Task 3: Implement `molt validate` as the canonical end-to-end gate

**Files:**
- Modify: `src/molt/cli.py`
- Modify: `tests/cli/test_cli_smoke.py`
- Modify: `tests/cli/test_cli_import_collection.py`
- Modify: `tests/test_native_lir_loop_join_semantics.py`
- Modify: `tests/test_wasm_control_flow.py`
- Modify: `tests/test_wasm_class_smoke.py`

- [ ] **Step 1: Add a structured validation matrix in `cli.py`**

Encode:

- command smoke lanes;
- `dev` and `release`;
- native / LLVM / linked wasm;
- conformance suite entrypoint;
- benchmark entrypoint;
- JSON and human-readable result modes.

- [ ] **Step 2: Add scoped selection flags**

Support:

- `--backend`
- `--profile`
- `--suite`
- `--json`

- [ ] **Step 3: Run the focused validate tests**

Run:

```bash
export MOLT_SESSION_ID=dx-cli-validate-3
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
PYTHONPATH=src ./.venv/bin/python -m pytest -q tests/cli/test_cli_setup_validate.py tests/cli/test_cli_smoke.py tests/cli/test_cli_import_collection.py
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/molt/cli.py tests/cli/test_cli_setup_validate.py tests/cli/test_cli_smoke.py tests/cli/test_cli_import_collection.py tests/test_native_lir_loop_join_semantics.py tests/test_wasm_control_flow.py tests/test_wasm_class_smoke.py
git commit -m "Make validate the canonical end-to-end release gate"
```

## Task 4: Reduce wrappers and dev tooling to thin delegates

**Files:**
- Modify: `tools/dev.py`
- Modify: `packaging/install.sh`
- Modify: `packaging/install.ps1`
- Modify: `packaging/config.toml`

- [ ] **Step 1: Simplify `tools/dev.py`**

Keep only delegation to:

- `molt doctor`
- `molt update`
- `molt validate`

Do not preserve duplicated behavioral ownership.

- [ ] **Step 2: Simplify install wrappers**

Make shell and PowerShell installers:

- bootstrap the shipped binary or Python entrypoint;
- delegate into `molt setup`;
- avoid owning independent dependency logic.

- [ ] **Step 3: Add/update tests for wrapper delegation where feasible**

Prefer lightweight unit/behavioral assertions over real package-install flows.

- [ ] **Step 4: Commit**

```bash
git add tools/dev.py packaging/install.sh packaging/install.ps1 packaging/config.toml
git commit -m "Reduce install and dev wrappers to thin delegates"
```

## Task 5: Update docs to the new canonical DX contract

**Files:**
- Modify: `docs/DEVELOPER_GUIDE.md`
- Modify: `docs/OPERATIONS.md`
- Modify: `CONTRIBUTING.md`
- Modify: `docs/spec/STATUS.md`
- Modify: `ROADMAP.md`

- [ ] **Step 1: Replace parallel workflow guidance with the canonical commands**

Point docs at:

- `molt setup`
- `molt doctor`
- `molt validate`

- [ ] **Step 2: Document canonical artifact roots and validation output**

Make sure docs match the current repo policy exactly.

- [ ] **Step 3: Commit**

```bash
git add docs/DEVELOPER_GUIDE.md docs/OPERATIONS.md CONTRIBUTING.md docs/spec/STATUS.md ROADMAP.md
git commit -m "Document the unified CLI DX contract"
```

## Task 6: Run the real end-to-end proof matrix

**Files:**
- Modify only if proof failures require fixes in touched paths

- [ ] **Step 1: Run the canonical CLI proof bundle**

Run:

```bash
export MOLT_SESSION_ID=dx-proof-6
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
PYTHONPATH=src ./.venv/bin/python -m pytest -q tests/cli/test_cli_setup_validate.py tests/cli/test_cli_smoke.py tests/cli/test_cli_import_collection.py
PYTHONPATH=src ./.venv/bin/python -m pytest -q tests/test_native_lir_loop_join_semantics.py tests/test_wasm_control_flow.py tests/test_wasm_class_smoke.py -k 'native_loop_join_semantics or preserves_type_name or wasm_module_try_exception_loop_parity'
cargo test -p molt-backend --features native-backend --test entry_block_param_shadow --test lir_loop_and_join_regressions --test native_extern_linkage --test ir_contract_validation -- --nocapture
cargo test -p molt-backend --features wasm-backend --test lir_wasm_repr_regressions --test wasm_lir_fast_path_integration -- --nocapture
```

Expected: PASS.

- [ ] **Step 2: Run the canonical `molt validate` command**

Run:

```bash
export MOLT_SESSION_ID=dx-proof-6b
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
PYTHONPATH=src ./.venv/bin/python -m molt.cli validate --json
```

Expected: PASS with structured output and no hidden fallback lanes.

- [ ] **Step 3: Commit**

```bash
git add src/molt/cli.py tools/dev.py packaging/install.sh packaging/install.ps1 packaging/config.toml tests/cli/test_cli_setup_validate.py tests/cli/test_cli_smoke.py tests/cli/test_cli_import_collection.py docs/DEVELOPER_GUIDE.md docs/OPERATIONS.md CONTRIBUTING.md docs/spec/STATUS.md ROADMAP.md
git commit -m "Unify setup, doctor, and validate into one DX surface"
```
