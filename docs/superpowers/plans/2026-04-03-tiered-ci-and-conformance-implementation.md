# Tiered CI And Canonical Conformance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split Molt CI into cheap presubmit vs targeted hosted validation, and make Molt conformance use one canonical smoke/full runner with shared env/result logic.

**Architecture:** Keep `push`/`pull_request` CI cheap and Linux-first, move heavy correctness/release/perf work into targeted workflows, and make `tests/harness/run_molt_conformance.py` the single correctness entrypoint with `--suite smoke|full`. Shared conformance mechanics live in `src/molt/harness_conformance.py`, and `src/molt/harness_layers.py` becomes a consumer of the canonical runner instead of a second implementation.

**Tech Stack:** GitHub Actions YAML, Python, pytest, existing harness scripts, existing Molt CLI/tooling

---

## File Map

| Path | Responsibility |
| --- | --- |
| `.github/workflows/ci.yml` | Cheap required presubmit only |
| `.github/workflows/nightly-correctness.yml` | Scheduled/manual correctness-heavy hosted validation |
| `.github/workflows/release-validation.yml` | Tag/manual hosted cross-platform release validation |
| `.github/workflows/perf-validation.yml` | Manual or optional scheduled benchmark lane |
| `src/molt/harness_conformance.py` | Shared env builder, suite loader, summary writer, exit policy |
| `tests/harness/corpus/monty_compat/SMOKE.txt` | Canonical smoke-suite manifest |
| `tests/harness/run_molt_conformance.py` | Canonical Molt conformance runner with `--suite smoke|full` |
| `tests/test_monty_conformance_runner.py` | Runner tests for suite selection, summary artifact, exit policy |
| `src/molt/harness_layers.py` | Harness integration that delegates to the canonical runner |
| `tests/test_harness_layers.py` | Tests for harness-layer delegation behavior |
| `docs/spec/areas/tooling/0011-ci.md` | CI architecture documentation |
| `docs/spec/STATUS.md` | Current-state wording aligned to canonical correctness lane if needed |
| `docs/DEVELOPER_GUIDE.md` | Local developer entrypoints for smoke/full conformance |

## Coordination Constraints

- There are active local edits in:
  - `src/molt/harness_layers.py`
  - `tests/harness/run_molt_conformance.py`
  - `tests/test_harness_layers.py`
  - untracked `tests/test_monty_conformance_runner.py`
- Read those files carefully before editing.
- Extend or integrate with partner work; do not revert, overwrite, or reformat blindly.
- Keep the shared utility focused. Do not build a second harness framework.

## Task 1: Split GitHub Workflows Into Cheap Presubmit And Targeted Hosted Lanes

**Files:**
- Modify: `.github/workflows/ci.yml`
- Create: `.github/workflows/nightly-correctness.yml`
- Create: `.github/workflows/release-validation.yml`
- Create: `.github/workflows/perf-validation.yml`
- Modify: `docs/spec/areas/tooling/0011-ci.md`
- Modify: `docs/DEVELOPER_GUIDE.md`

- [ ] **Step 1: Write failing workflow-shape tests or assertions**

Add or extend a small Python test file under `tests/` that asserts:

- push/PR workflow does not contain benchmark jobs;
- push/PR workflow does not contain full differential jobs;
- `ci.yml` uses Linux for cheap gates;
- nightly/release/perf workflows exist at the expected paths.

Use a focused test file such as:

```python
def test_ci_push_path_is_cheap_only():
    ...
```

- [ ] **Step 2: Run the workflow-shape test to verify it fails**

Run:

```bash
./.venv/bin/python -m pytest -q tests/test_ci_workflow_topology.py
```

Expected: FAIL because the split workflow topology does not exist yet.

- [ ] **Step 3: Simplify `ci.yml` into required cheap presubmit**

Implement:

- docs gates on `ubuntu-latest`;
- Python/tooling smoke on `ubuntu-latest`;
- Rust build/unit smoke on `ubuntu-latest`;
- no full benchmark or full differential jobs in push/PR CI.

- [ ] **Step 4: Add nightly correctness workflow**

Create `.github/workflows/nightly-correctness.yml` with:

- `schedule` trigger;
- `workflow_dispatch` trigger;
- full Molt conformance job;
- differential job for `tests/differential/basic` and `tests/differential/stdlib`.

- [ ] **Step 5: Add release and perf workflows**

Create:

- `.github/workflows/release-validation.yml` with explicit:
  - `push` tags trigger for release validation;
  - `workflow_dispatch` trigger for manual release validation;
  - one Linux release build/package validation job;
  - one macOS release build/package validation job;
  - artifact upload or publication-check steps for both hosts;
- `.github/workflows/perf-validation.yml` for manual benchmark execution, with optional low-frequency schedule only if it is cheap enough to justify.

- [ ] **Step 6: Update CI docs**

Rewrite `docs/spec/areas/tooling/0011-ci.md` so it clearly states:

- local verification is the full gatekeeper;
- push/PR GitHub CI is cheap presubmit only;
- nightly correctness is automated but not push-required;
- release/perf lanes are targeted hosted validation.

Also update `docs/DEVELOPER_GUIDE.md` so the new local correctness entrypoints
are discoverable:

- `python3 tests/harness/run_molt_conformance.py --suite smoke`
- `python3 tests/harness/run_molt_conformance.py --suite full`

- [ ] **Step 7: Run tests and basic workflow sanity checks**

Run:

```bash
./.venv/bin/python -m pytest -q tests/test_ci_workflow_topology.py
./.venv/bin/python tools/check_docs_architecture.py
```

Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add .github/workflows/ci.yml .github/workflows/nightly-correctness.yml .github/workflows/release-validation.yml .github/workflows/perf-validation.yml docs/spec/areas/tooling/0011-ci.md docs/DEVELOPER_GUIDE.md tests/test_ci_workflow_topology.py
git commit -m "ci: split presubmit and hosted validation lanes"
```

## Task 2: Add Shared Conformance Utility And Canonical Smoke Manifest

**Files:**
- Create: `src/molt/harness_conformance.py`
- Create: `tests/harness/corpus/monty_compat/SMOKE.txt`
- Create: `tests/test_harness_conformance.py`

- [ ] **Step 1: Write the failing shared-utility tests**

Create `tests/test_harness_conformance.py` covering:

- `build_molt_conformance_env(...)` populates canonical roots and session ID;
- `ensure_molt_conformance_dirs(...)` creates required directories;
- `load_molt_conformance_suite(..., suite=\"smoke\", ...)` preserves manifest order;
- blank lines and `#` comments in `SMOKE.txt` are ignored;
- `conformance_exit_code(...)` returns nonzero on failures/compile errors/timeouts.

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
./.venv/bin/python -m pytest -q tests/test_harness_conformance.py
```

Expected: FAIL because `src/molt/harness_conformance.py` does not exist yet.

- [ ] **Step 3: Create the shared utility**

Implement `src/molt/harness_conformance.py` with exactly these focused helpers:

```python
def build_molt_conformance_env(project_root: Path, session_id: str) -> dict[str, str]: ...
def ensure_molt_conformance_dirs(env: dict[str, str]) -> None: ...
def load_molt_conformance_suite(corpus_dir: Path, suite: str, smoke_manifest: Path) -> list[Path]: ...
def write_molt_conformance_summary(path: Path, summary: dict[str, object]) -> None: ...
def conformance_exit_code(summary: dict[str, object]) -> int: ...
```

Do not add unrelated orchestration or CLI code here.

- [ ] **Step 4: Create the smoke manifest**

Add `tests/harness/corpus/monty_compat/SMOKE.txt` with a small, representative,
deterministic subset. Keep it intentionally small enough for push-time use.

- [ ] **Step 5: Run tests**

Run:

```bash
PYTHONPATH=src ./.venv/bin/python -m pytest -q tests/test_harness_conformance.py
```

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/molt/harness_conformance.py tests/harness/corpus/monty_compat/SMOKE.txt tests/test_harness_conformance.py
git commit -m "harness: add shared conformance utility"
```

## Task 3: Upgrade The Canonical Molt Conformance Runner

**Files:**
- Modify: `tests/harness/run_molt_conformance.py`
- Create or Modify: `tests/test_monty_conformance_runner.py`

- [ ] **Step 1: Write or extend failing runner tests**

In `tests/test_monty_conformance_runner.py`, cover:

- `--suite smoke` loads `tests/harness/corpus/monty_compat/SMOKE.txt`;
- `--suite full` loads the full corpus;
- JSON summary artifact contains exact required fields:
  - `suite`
  - `manifest_path`
  - `corpus_root`
  - `duration_s`
  - `total`
  - `passed`
  - `failed`
  - `compile_error`
  - `timeout`
  - `skipped`
  - `failures`
  - `compile_errors`
  - `timeouts`
- JSON summary artifact preserves per-test failure buckets with path/detail data;
- exit code is nonzero on `failed`, `compile_error`, or `timeout`;
- runner imports and uses `src/molt/harness_conformance.py` helpers.

If `tests/test_monty_conformance_runner.py` already exists locally, extend it
instead of replacing it.

- [ ] **Step 2: Run the runner tests to verify they fail**

Run:

```bash
PYTHONPATH=src ./.venv/bin/python -m pytest -q tests/test_monty_conformance_runner.py
```

Expected: FAIL because the runner does not yet implement the canonical smoke/full contract.

- [ ] **Step 3: Upgrade `run_molt_conformance.py`**

Implement:

- `--suite smoke|full`;
- optional `--json-out <path>`;
- use of `build_molt_conformance_env`, `ensure_molt_conformance_dirs`,
  `load_molt_conformance_suite`, `write_molt_conformance_summary`, and
  `conformance_exit_code`;
- one canonical JSON summary shape:

```json
{
  "suite": "smoke",
  "manifest_path": "tests/harness/corpus/monty_compat/SMOKE.txt",
  "corpus_root": "tests/harness/corpus/monty_compat",
  "duration_s": 1.23,
  "total": 10,
  "passed": 8,
  "failed": 1,
  "compile_error": 0,
  "timeout": 0,
  "skipped": 1,
  "failures": [{"path": "foo.py", "detail": "expected exit 0"}],
  "compile_errors": [],
  "timeouts": []
}
```

- one result taxonomy only:
  - `passed`
  - `failed`
  - `compile_error`
  - `timeout`
  - `skipped`

Do not duplicate environment/result logic inline once the shared utility exists.

- [ ] **Step 4: Run tests**

Run:

```bash
PYTHONPATH=src ./.venv/bin/python -m pytest -q tests/test_monty_conformance_runner.py
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add tests/harness/run_molt_conformance.py tests/test_monty_conformance_runner.py
git commit -m "harness: add smoke and full conformance suites"
```

## Task 4: Make Harness Layers Delegate To The Canonical Runner

**Files:**
- Modify: `src/molt/harness_layers.py`
- Modify: `tests/test_harness_layers.py`
- Modify: `docs/spec/STATUS.md`

- [ ] **Step 1: Write or extend failing harness-layer tests**

Add targeted tests in `tests/test_harness_layers.py` that verify:

- the harness conformance layer invokes the canonical runner rather than duplicating suite/env logic;
- the layer reads the canonical summary artifact fields from the runner output:
  - `total`
  - `passed`
  - `failed`
  - `compile_error`
  - `timeout`
  - `skipped`
  - `duration_s`
- pass/fail mapping follows the canonical result taxonomy.

- [ ] **Step 2: Run the harness-layer tests to verify they fail**

Run:

```bash
PYTHONPATH=src ./.venv/bin/python -m pytest -q tests/test_harness_layers.py
```

Expected: FAIL because the layer still has duplicated or divergent behavior.

- [ ] **Step 3: Modify `src/molt/harness_layers.py`**

Refactor only the conformance integration path so that it:

- shells out to `tests/harness/run_molt_conformance.py --suite full --json-out ...` for the deep conformance layer;
- consumes the summary artifact rather than rebuilding the same accounting;
- keeps existing layer-report structure intact where possible.

Do not turn `harness_layers.py` into another conformance implementation.

- [ ] **Step 4: Update status/docs wording if needed**

If the current wording in `docs/spec/STATUS.md` still describes correctness
authority vaguely, tighten it so it points at the canonical conformance lane
and local-first validation model.

- [ ] **Step 5: Run focused tests**

Run:

```bash
PYTHONPATH=src ./.venv/bin/python -m pytest -q tests/test_harness_layers.py tests/test_monty_conformance_runner.py tests/test_harness_conformance.py
```

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/molt/harness_layers.py tests/test_harness_layers.py docs/spec/STATUS.md
git commit -m "harness: delegate layers to canonical conformance runner"
```

## Final Verification

- [ ] **Step 1: Run the focused tranche verification**

```bash
PYTHONPATH=src ./.venv/bin/python -m pytest -q tests/test_ci_workflow_topology.py tests/test_harness_conformance.py tests/test_monty_conformance_runner.py tests/test_harness_layers.py
```

Expected: PASS

- [ ] **Step 2: Run docs/policy verification touched by the tranche**

```bash
./.venv/bin/python tools/check_docs_architecture.py
./.venv/bin/python tools/update_status_blocks.py --check
./.venv/bin/python tools/bench_report.py --manifest bench/results/docs_manifest.json --check --update-status-doc
```

Expected: PASS

- [ ] **Step 3: Run git diff hygiene check**

```bash
git diff --check
git diff --cached --check
```

Expected: PASS

- [ ] **Step 4: Final commit**

```bash
git add .github/workflows src/molt/harness_conformance.py tests/harness/corpus/monty_compat/SMOKE.txt tests/harness/run_molt_conformance.py tests/test_harness_conformance.py tests/test_monty_conformance_runner.py src/molt/harness_layers.py tests/test_harness_layers.py docs/spec/areas/tooling/0011-ci.md docs/spec/STATUS.md
git commit -m "Implement tiered CI and canonical conformance lanes"
```
