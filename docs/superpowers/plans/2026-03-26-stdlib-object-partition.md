# Stdlib Object Partition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Re-enable native stdlib object caching by fixing backend ownership of stdlib init symbols and by making native link invalidation include the linked stdlib object.

**Architecture:** Keep `output.o` limited to user/runtime ABI roots, keep stdlib implementation bodies in `MOLT_STDLIB_OBJ`, and let the final native link resolve cross-object references. Tighten CLI link fingerprints so changes to the stdlib object force relink.

**Tech Stack:** Python CLI, Rust native backend, pytest

---

### Task 1: Add Failing Native Link Call-Site Coverage

**Files:**
- Modify: `tests/cli/test_cli_import_collection.py`
- Modify: `src/molt/cli.py`

- [ ] **Step 1: Write the failing test**

Add a test around the native link preparation call site proving that when
`MOLT_STDLIB_OBJ` exists and participates in the link, the assembled link
fingerprint input set also includes that stdlib object path.

- [ ] **Step 2: Run test to verify it fails**

Run: `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k stdlib_link_fingerprint`

Expected: the new test fails because the call site still assembles
`inputs=[stub_path, output_obj, runtime]` and omits `MOLT_STDLIB_OBJ`.

- [ ] **Step 3: Write minimal implementation**

Thread the optional linked stdlib object into the native link fingerprint input
set at the real native link call site in `src/molt/cli.py`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k stdlib_link_fingerprint`

Expected: the new call-site coverage passes.

### Task 2: Add Failing Native Stdlib-Object Env Coverage

**Files:**
- Modify: `tests/cli/test_cli_import_collection.py`
- Modify: `src/molt/cli.py`

- [ ] **Step 1: Write the failing test**

Add a focused unit test around the native backend subprocess path verifying that
eligible native compiles set `MOLT_STDLIB_OBJ` in the backend environment and
that wasm/transpile paths do not. The test must exercise the actual helper or
code path that computes the stdlib object cache path.

- [ ] **Step 2: Run test to verify it fails**

Run: `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k MOLT_STDLIB_OBJ`

Expected: the native path assertion fails because the env var is never set.

- [ ] **Step 3: Write minimal implementation**

In `src/molt/cli.py`, re-enable setting `MOLT_STDLIB_OBJ` for eligible native
backend subprocess compiles using an explicit stdlib object cache-path helper,
not the module output cache path.

- [ ] **Step 4: Run tests to verify they pass**

Run: `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k MOLT_STDLIB_OBJ`

Expected: the new env-routing test passes.

### Task 3: Add Failing Rust Ownership Coverage

**Files:**
- Modify: `runtime/molt-backend/src/main.rs`

- [ ] **Step 1: Write the failing test**

Extract or add a narrow Rust-side helper for ownership classification and add a
Rust unit test proving stdlib `molt_init_*` symbols are not retained in the user
object merely because they match the `molt_init_` prefix.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p molt-backend stdlib_object_partition -- --nocapture`

Expected: the classifier test fails because the backend currently treats every
`molt_init_*` symbol as user-owned.

- [ ] **Step 3: Write minimal implementation**

Refine the ownership heuristic in `runtime/molt-backend/src/main.rs` so the
user object keeps only entry/runtime ABI roots while stdlib init bodies remain
in the stdlib object.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p molt-backend stdlib_object_partition -- --nocapture`

Expected: the new classifier test passes.

### Task 4: Run End-to-End Verification

**Files:**
- Modify: `docs/OPERATIONS.md`

- [ ] **Step 1: Update workflow docs if behavior changed**

Document the re-enabled stdlib object caching workflow in `docs/OPERATIONS.md`
if the final implementation changes the supported local flow.

- [ ] **Step 2: Run the focused CLI file**

Run: `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py`

Expected: full file passes.

- [ ] **Step 3: Refresh Linear artifacts**

Run: `python3 tools/linear_hygiene.py refresh-local-artifacts --repo-root .`

Expected: `changed_file_count: 0`.

- [ ] **Step 4: Verify live Linear sync remains converged**

Run: `env -u LINEAR_API_KEY python3 tools/linear_workspace.py sync-index --team Moltlang --index ops/linear/manifests/index.json --update-existing --close-duplicates --close-missing --duplicate-state Canceled --dry-run`

Expected: no planned creates/updates/closures.
