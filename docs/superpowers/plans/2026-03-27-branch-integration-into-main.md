# ~~Branch Integration Into Main Implementation Plan~~ [SUPERSEDED]

> **SUPERSEDED** by Operation Greenfield (2026-03-27): see `docs/superpowers/specs/2026-03-27-operation-greenfield-design.md` and the Wave A/C/B plans.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Integrate the unique work from the local side branches into `main` without trampling current `main`, losing uncommitted workspace signal, or reintroducing known-bad reverted behavior.

**Architecture:** Preserve the current dirty `main` state first, then treat each branch as a queue of candidate commits to replay onto current `main`. Prefer cherry-picking or manual reapplication of the branch-only commits over full branch merges because the side branches are hundreds of commits behind `main` and contain superseded and reverted history. Verify after each tranche and stop on the first unsafe conflict.

**Tech Stack:** `git`, pytest, cargo test, Molt CLI smoke runs, repo-local plans and canonical `tmp/` artifacts

---

### Task 1: Preserve Current Local Signal Before Integration

**Files:**
- Modify: `tmp/branch_integration/`
- Modify: git stash state only

- [ ] **Step 1: Capture tracked diff and untracked file inventory**

Run:
```bash
mkdir -p tmp/branch_integration
git status --short > tmp/branch_integration/premerge_status.txt
git diff > tmp/branch_integration/premerge_tracked.diff
git ls-files --others --exclude-standard > tmp/branch_integration/premerge_untracked.txt
```

- [ ] **Step 2: Stash current dirty state with a named stash**

Run:
```bash
git stash push -u -m "pre-branch-integration-2026-03-27"
git stash list | head
```

Expected: working tree becomes clean enough for replay work, with the stash entry visible and recoverable.

### Task 2: Reconfirm Branch-Only Commit Sets

**Files:**
- Modify: `tmp/branch_integration/`

- [ ] **Step 1: Export branch-only commit lists**

Run:
```bash
git log --reverse --oneline main..perf/outline-class-def > tmp/branch_integration/perf_outline_commits.txt
git log --reverse --oneline main..session-optimizations > tmp/branch_integration/session_optimizations_commits.txt
```

- [ ] **Step 2: Split candidates into buckets**

Bucket the branch-only commits into:
- safe to replay directly
- likely superseded by `main`
- likely dangerous / requires manual re-implementation

- [ ] **Step 3: Record exclusions explicitly**

For every skipped commit, record why it is skipped in `tmp/branch_integration/decision_log.md`.

### Task 3: Replay `perf/outline-class-def` Carefully

**Files:**
- Modify: `runtime/molt-backend/src/native_backend/function_compiler.rs`
- Modify: `runtime/molt-runtime/src/object/ops.rs`
- Modify: `src/molt/cli.py`
- Modify: `src/molt/frontend/__init__.py`

- [ ] **Step 1: Attempt replay of the guarded-call outlining pair**

Replay in order:
```bash
git cherry-pick 8e6609c6
git cherry-pick 387fc140
```

Expected: if conflicts arise, resolve manually in favor of current `main` behavior plus the outlining intent.

- [ ] **Step 2: Verify after guarded-call outlining**

Run:
```bash
cargo test -p molt-backend --features native-backend -- --nocapture
PYTHONPATH=src uv run --python 3.12 python3 -m pytest -q tests/test_frontend_midend_passes.py tests/cli/test_cli_import_collection.py
MOLT_BACKEND_DAEMON=0 PYTHONPATH=src uv run --python 3.12 python3 -m molt.cli run --profile dev examples/hello.py
```

- [ ] **Step 3: Replay compiler artifact/layout commits if still applicable**

Replay in order only if Step 2 is green and the diff is still additive:
```bash
git cherry-pick ef6de613
git cherry-pick 64e9ba29
```

- [ ] **Step 4: Re-evaluate Cranelift speed/atomic RC commit instead of blind replay**

Do **not** auto-apply `3c84d7d1` unless the current backend state proves the same assumptions still hold. Compare it against the current backend configuration and only reapply the safe subset.

### Task 4: Replay `session-optimizations` In Safety Order

**Files:**
- Modify: `runtime/molt-runtime/src/object/mod.rs`
- Modify: `runtime/molt-runtime/src/object/ops.rs`
- Modify: `runtime/molt-runtime/src/object/memoryview.rs`
- Modify: `runtime/molt-runtime/src/builtins/attr.rs`
- Modify: `runtime/molt-backend/src/lib.rs`
- Modify: `src/molt/stdlib/threading.py`
- Modify: `src/molt/stdlib/tkinter/__init__.py`
- Modify: `Cargo.toml`
- Modify: `runtime/molt-runtime/Cargo.toml`
- Modify: `src/molt/cli.py`
- Modify: `src/molt/frontend/__init__.py`

- [ ] **Step 1: Replay the runtime UB fixes first**

Preferred order:
```bash
git cherry-pick 09c025ad
git cherry-pick 9af5b35d
git cherry-pick 6df731c6
```

Expected: these are correctness/safety fixes and should be evaluated before later perf toggles.

- [ ] **Step 2: Verify runtime safety tranche**

Run:
```bash
cargo test -p molt-runtime -- --nocapture
cargo test -p molt-backend --features native-backend -- --nocapture
```

- [ ] **Step 3: Replay attribute-correctness fixes**

Replay in order:
```bash
git cherry-pick 0545e561
git cherry-pick 514a1efe
```

- [ ] **Step 4: Verify focused runtime and stdlib behavior**

Run targeted tests or smoke probes covering attribute lookup and tkinter import/behavior.

- [ ] **Step 5: Replay code-size and build-contract improvements that still compose with current `main`**

Potential candidates, one at a time with verification:
```bash
git cherry-pick 335831c4
git cherry-pick 5fc9a425
git cherry-pick 73f36839
git cherry-pick 8d769513
git cherry-pick e8583b87
git cherry-pick 893c88b6
git cherry-pick 2cd7aa66
git cherry-pick b9dd2a9f
```

- [ ] **Step 6: Skip or manually inspect unsafe/superseded backend-speed commits**

Do not blind-replay these without manual inspection against current `main`:
- `cfb89e0d`
- `ee10cc06`
- `fce6ffe5`
- `9705fab8`
- `d2701d35`
- `94c3f721`
- `19af212b` if already covered by Task 3
- `8ba7a921` if its parent change was not replayed

### Task 5: Restore Local Workspace State And Finalize

**Files:**
- Modify: restored local files from stash
- Modify: `tmp/branch_integration/decision_log.md`

- [ ] **Step 1: Restore the pre-integration stash**

Run:
```bash
git stash list | head
git stash pop
```

- [ ] **Step 2: Resolve any stash-restore conflicts without dropping merged work**

If conflicts appear, preserve both the restored local edits and the newly replayed branch work.

- [ ] **Step 3: Final verification**

Run:
```bash
git status --short
cargo test -p molt-backend --features native-backend -- --nocapture
PYTHONPATH=src uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py
MOLT_BACKEND_DAEMON=0 PYTHONPATH=src uv run --python 3.12 python3 -m molt.cli run --profile dev examples/hello.py
```

- [ ] **Step 4: Document any commits intentionally not integrated**

Update `tmp/branch_integration/decision_log.md` with skipped commits and reasons so no branch signal is silently lost.
