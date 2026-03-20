# WASM VFS Snapshot Implementation Plan (Plan D)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `molt.snapshot` artifact generation and restore for sub-millisecond cold starts on edge platforms.

**Architecture:** After deterministic init (imports resolved, top-level code executed), serialize WASM linear memory + globals into a snapshot blob. At cold start, restore from snapshot instead of re-executing init.

**Tech Stack:** Rust (molt-wasm-host for capture/restore), Python (CLI for --snapshot flag)

**Depends on:** Plans A, B, C

---

### Task 1: Snapshot Capture

**Files:**
- Modify: `runtime/molt-wasm-host/src/main.rs`

- [ ] **Step 1: After init completes, capture memory and global state**
- [ ] **Step 2: Serialize to molt.snapshot format (JSON header + binary blob)**
- [ ] **Step 3: Include mount_plan, capability_manifest, module_hash in header**
- [ ] **Step 4: Commit**

---

### Task 2: Snapshot Restore

**Files:**
- Modify: `runtime/molt-wasm-host/src/main.rs`

- [ ] **Step 1: On startup, check for molt.snapshot sidecar**
- [ ] **Step 2: Validate snapshot_version, abi_version, module_hash**
- [ ] **Step 3: Restore memory and globals from blob, skip init**
- [ ] **Step 4: Commit**

---

### Task 3: --snapshot CLI Flag

**Files:**
- Modify: `src/molt/cli.py`

- [ ] **Step 1: Add --snapshot flag to build command**
- [ ] **Step 2: When set, run init in sandbox and capture snapshot**
- [ ] **Step 3: Include molt.snapshot in deployment artifact**
- [ ] **Step 4: Commit**

---

### Task 4: Snapshot Tests

**Files:**
- Create: `tests/test_wasm_snapshot.py`

- [ ] **Step 1: Test snapshot determinism (same input → same bytes)**
- [ ] **Step 2: Test snapshot restore correctness**
- [ ] **Step 3: Test stale snapshot rejection (hash mismatch)**
- [ ] **Step 4: Commit**
