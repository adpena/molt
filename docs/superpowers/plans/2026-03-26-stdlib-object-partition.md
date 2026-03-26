# Stdlib Object Partition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement option `3/C`: multi-artifact stdlib partitioning for native builds with explicit daemon/link plumbing and versioned cache behavior.

**Architecture:** Keep `output.o` as the user object, emit stdlib sidecar batch objects under a versioned partition root, propagate that root through daemon/subprocess compile paths, and link using explicit artifact lists rather than ambient env state.

**Tech Stack:** Python CLI, Rust backend, pytest, cargo test

---

### Task 1: Lock The Backend Ownership Boundary

**Files:**
- Modify: `runtime/molt-backend/src/main.rs`

- [ ] **Step 1: Write the failing Rust test**

Add or refine a Rust unit test proving non-entry stdlib `molt_init_*` symbols
are excluded from the user-owned root set while true entry/runtime ABI roots are
retained.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p molt-backend --features native-backend user_owned_symbol_whitelist_keeps_only_entry_roots -- --nocapture`

Expected: FAIL until the whitelist matches the intended user/runtime ABI roots.

- [ ] **Step 3: Implement minimal Rust fix**

Refine the ownership classifier in `runtime/molt-backend/src/main.rs`.

- [ ] **Step 4: Run Rust test to verify it passes**

Run: `cargo test -p molt-backend --features native-backend user_owned_symbol_whitelist_keeps_only_entry_roots -- --nocapture`

Expected: PASS.

### Task 2: Version The Partition Mode In Python Cache Setup

**Files:**
- Modify: `src/molt/cli.py`
- Modify: `tests/cli/test_cli_import_collection.py`

- [ ] **Step 1: Write the failing Python test**

Add a test proving native cache identity changes when stdlib partition mode is
enabled versus monolithic output.

- [ ] **Step 2: Run test to verify it fails**

Run: `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k stdlib_partition_mode`

Expected: FAIL until cache setup encodes partition mode/version.

- [ ] **Step 3: Implement minimal Python fix**

Add partition-mode/version bits to the backend cache variant and introduce the
canonical stdlib partition-root helper.

- [ ] **Step 4: Re-run targeted test**

Run: `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k stdlib_partition_mode`

Expected: PASS.

### Task 3: Plumb Partition Metadata Through Daemon And Subprocess Paths

**Files:**
- Modify: `src/molt/cli.py`
- Modify: `tests/cli/test_cli_import_collection.py`

- [x] **Step 1: Write the failing Python tests**

Add targeted tests proving:

- daemon request payload includes partition-root metadata and entry-module data;
- subprocess fallback receives the same partition metadata in env.

- [x] **Step 2: Run tests to verify they fail**

Run: `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k 'stdlib_partition_daemon or stdlib_partition_subprocess'`

Expected: FAIL until both paths are wired.

- [x] **Step 3: Implement minimal Python fix**

Plumb explicit partition-root metadata through `_backend_daemon_compile_request_bytes`
and `_execute_backend_compile`.

- [x] **Step 4: Re-run targeted tests**

Run: `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k 'stdlib_partition_daemon or stdlib_partition_subprocess'`

Expected: PASS.

### Task 4: Make Native Linking Explicit And Deterministic

**Files:**
- Modify: `src/molt/cli.py`
- Modify: `tests/cli/test_cli_import_collection.py`

- [ ] **Step 1: Write the failing Python tests**

Add tests proving:

- native link preparation includes stdlib partition artifacts explicitly;
- changing a linked stdlib partition artifact changes the link fingerprint.

- [ ] **Step 2: Run tests to verify they fail**

Run: `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k stdlib_link_fingerprint`

Expected: FAIL until native link preparation reads explicit partition artifacts.

- [ ] **Step 3: Implement minimal Python fix**

Replace env-only link behavior with explicit stdlib partition artifact inputs.

- [ ] **Step 4: Re-run targeted tests**

Run: `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k stdlib_link_fingerprint`

Expected: PASS.

### Task 5: Decide And Enforce `emit=obj` Contract

**Files:**
- Modify: `src/molt/cli.py`
- Modify: `tests/cli/test_cli_import_collection.py`
- Modify: `docs/OPERATIONS.md`

- [ ] **Step 1: Write the failing test**

Add a focused test for the chosen `emit=obj` behavior under partition mode.

- [ ] **Step 2: Run test to verify it fails**

Run: `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k stdlib_partition_emit_obj`

Expected: FAIL until the contract is implemented.

- [ ] **Step 3: Implement the chosen behavior**

Either partial-link the sidecar stdlib artifacts into the emitted object or
raise explicitly if partition mode is unsupported for `emit=obj`.

- [ ] **Step 4: Re-run targeted test**

Run: `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k stdlib_partition_emit_obj`

Expected: PASS.

### Task 6: Full Verification And Workspace Hygiene

**Files:**
- Modify: `docs/OPERATIONS.md`

- [x] **Step 1: Run focused Python verification**

Run: `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py`

Expected: PASS.

- [x] **Step 2: Run focused Rust verification**

Run: `cargo test -p molt-backend --features native-backend user_owned_symbol_whitelist_keeps_only_entry_roots -- --nocapture`

Expected: PASS.

- [x] **Step 3: Refresh Linear artifacts**

Run: `python3 tools/linear_hygiene.py refresh-local-artifacts --repo-root .`

Expected: `changed_file_count: 0`.

- [x] **Step 4: Verify live Linear sync remains converged**

Run: `env -u LINEAR_API_KEY python3 tools/linear_workspace.py sync-index --team Moltlang --index ops/linear/manifests/index.json --update-existing --close-duplicates --close-missing --duplicate-state Canceled --dry-run`

Expected: no planned creates/updates/closures.
