# Linear Grouped Backlog Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace leaf-per-issue Linear seeding with a grouped, impact-aware backlog model that stays within the live workspace cap while preserving repo-derived priority signal.

**Architecture:** Keep the full leaf seed inventory, then add a deterministic grouping/reduction layer that produces grouped manifest issues with explicit impact and pressure metadata. Reuse the existing manifest and sync pipeline where possible by keeping the outer issue schema stable and teaching refresh/sync code to operate on grouped items.

**Tech Stack:** Python 3.12 tooling, JSON manifests, pytest, Linear GraphQL API

---

### Task 1: Add grouped backlog tests

**Files:**
- Modify: `tests/test_linear_seed_backlog_tool.py`
- Modify: `tests/test_linear_workspace_sync.py`

- [ ] **Step 1: Write failing tests for grouping and priority rollup**
- [ ] **Step 2: Run targeted tests to verify they fail**
Run: `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/test_linear_seed_backlog_tool.py tests/test_linear_workspace_sync.py`
Expected: failures for missing grouping helpers / missing grouped metadata assertions
- [ ] **Step 3: Add minimal grouping implementation hooks**
- [ ] **Step 4: Re-run targeted tests to verify they pass**

### Task 2: Implement grouped reduction in seed tooling

**Files:**
- Modify: `tools/linear_seed_backlog.py`

- [ ] **Step 1: Add deterministic category/group key derivation**
- [ ] **Step 2: Add impact scoring and pressure summary rollups**
- [ ] **Step 3: Add grouped manifest item builder with stable title/description/metadata**
- [ ] **Step 4: Keep leaf inventory output available for local auditing**
- [ ] **Step 5: Run targeted tests**

### Task 3: Switch local artifact refresh to grouped manifests

**Files:**
- Modify: `tools/linear_hygiene.py`
- Modify: `tests/test_linear_hygiene_tool.py`

- [ ] **Step 1: Write failing tests for grouped artifact refresh counts/shape**
- [ ] **Step 2: Run the hygiene tests and verify failure**
- [ ] **Step 3: Update refresh-local-artifacts to build grouped manifests/index rows**
- [ ] **Step 4: Re-run hygiene tests and verify pass**

### Task 4: Teach sync to work cleanly with grouped issues

**Files:**
- Modify: `tools/linear_workspace.py`
- Modify: `tests/test_linear_workspace_sync.py`
- Modify: `tests/test_linear_workspace_tool.py`

- [ ] **Step 1: Add failing tests for grouped issue descriptions / metadata stability**
- [ ] **Step 2: Run sync/workspace tests and verify failure**
- [ ] **Step 3: Implement grouped description/rendering changes with stable sync keys**
- [ ] **Step 4: Re-run sync/workspace tests and verify pass**

### Task 5: Regenerate local artifacts and validate convergence

**Files:**
- Modify: `ops/linear/seed_backlog.json`
- Modify: `ops/linear/manifests/index.json`
- Modify: `ops/linear/manifests/*.json`
- Modify: `docs/OPERATIONS.md`

- [ ] **Step 1: Run grouped refresh-local-artifacts with `--apply`**
- [ ] **Step 2: Verify grouped issue counts are materially lower and priority ordering looks sane**
- [ ] **Step 3: Update docs for grouped backlog workflow**
- [ ] **Step 4: Re-run refresh-local-artifacts without `--apply` and verify no-op**

### Task 6: Migrate live Linear to grouped/category issues

**Files:**
- Runtime data only: live Linear workspace
- Audit outputs: `tmp/linear_*`

- [ ] **Step 1: Export current live issues**
- [ ] **Step 2: Sync grouped manifests into Linear**
- [ ] **Step 3: Delete or close superseded leaf issues not represented by grouped manifests**
- [ ] **Step 4: Re-run sync until only cap-induced residual creates remain, or convergence is reached**

### Task 7: Final verification

**Files:**
- Modify: none required beyond prior tasks

- [ ] **Step 1: Run `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/test_linear_hygiene_tool.py tests/test_linear_workspace_sync.py tests/test_linear_workspace_tool.py tests/test_linear_seed_backlog_tool.py`**
- [ ] **Step 2: Run `python3 tools/linear_hygiene.py refresh-local-artifacts --repo-root .` and verify no-op**
- [ ] **Step 3: Run live `sync-index --dry-run` and confirm create pressure is reduced to grouped residuals**
