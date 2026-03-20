# WASM VFS Packaging Implementation Plan (Plan C)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `--bundle` and `--profile` flags to `molt build` that package Python source into a tar archive and configure build settings per deployment target.

**Architecture:** CLI creates a tar bundle from source directory, generates a deployment manifest, and passes the bundle path to the wasmtime host. Profile presets (cloudflare, browser, wasi, fastly) set default optimization and capability settings.

**Tech Stack:** Python (CLI), tarfile stdlib

**Depends on:** Plan A (VFS Core) — completed, Plan B (Host Adapters)

---

### Task 1: Bundle Creation Tool

**Files:**
- Create: `tools/wasm_bundle.py`

- [ ] **Step 1: Write bundle creation tool**

Create `tools/wasm_bundle.py` that packages a directory into a tar with manifest:
- Walks directory recursively, adds all files
- Generates `__manifest__.json` with file list, sizes, total bytes
- Rejects symlinks and paths with `..`
- Sorts entries for determinism

- [ ] **Step 2: Add tests**
- [ ] **Step 3: Commit**

---

### Task 2: --bundle CLI Flag

**Files:**
- Modify: `src/molt/cli.py`

- [ ] **Step 1: Add --bundle argument to build subparser**
- [ ] **Step 2: Call wasm_bundle.py to create tar during build**
- [ ] **Step 3: Pass bundle path to host via environment or sidecar**
- [ ] **Step 4: Commit**

---

### Task 3: --profile Presets

**Files:**
- Modify: `src/molt/cli.py`

- [ ] **Step 1: Add --profile argument (cloudflare, browser, wasi, fastly)**
- [ ] **Step 2: Each profile sets defaults for --wasm-opt-level, --wasm-profile, --precompile, capabilities, tmp quota**
- [ ] **Step 3: Commit**

---

### Task 4: Worker.js Generation

**Files:**
- Create: `tools/wasm_worker_template.js`
- Modify: `src/molt/cli.py`

- [ ] **Step 1: Create Cloudflare Worker entry point template**
- [ ] **Step 2: Generate worker.js during --profile cloudflare builds**
- [ ] **Step 3: Commit**
