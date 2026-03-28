# ~~Molt Stabilization And Roadmap Continuation Implementation Plan~~ [SUPERSEDED]

> **SUPERSEDED** by Operation Greenfield (2026-03-27): see `docs/superpowers/specs/2026-03-27-operation-greenfield-design.md` and the Wave A/C/B plans.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert the current mix of recent handoff notes, roadmap blockers, and partially completed feature work into a deterministic recovery-and-execution sequence that restores green validation, resumes roadmap work, and keeps repo progress aligned with the grouped Linear workspace.

**Architecture:** Treat this as a control-plane-first recovery. First re-establish reproducible local truth (git state, benchmark state, targeted failures, daemon behavior, wasm/runtime blockers). Then close immediate correctness and infra regressions blocking native and wasm confidence. After that, finish the in-flight architectural slices already partially implemented, and only then resume broader roadmap expansion through grouped workstreams already represented in `ops/linear`.

**Tech Stack:** Python 3.12 tooling, Rust runtime/backend crates, pytest, cargo test, Molt differential harness, grouped Linear manifests, benchmark artifacts under `bench/results/`

---

### Task 1: Re-establish canonical local truth

**Files:**
- Modify: `docs/spec/STATUS.md`
- Modify: `ROADMAP.md`
- Modify: `docs/ROADMAP_90_DAYS.md`
- Modify: `bench/results/`
- Modify: `logs/`

- [ ] **Step 1: Export canonical env roots before any build/test/bench work**
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
```

- [ ] **Step 2: Capture current repo truth before touching runtime/compiler code**
Run:
```bash
git status --short
git log --oneline -n 20
python3 tools/linear_hygiene.py refresh-local-artifacts --repo-root .
```
Expected: only intentional local drift remains; grouped `ops/linear` artifacts stay converged.

- [ ] **Step 3: Re-run the smallest high-signal validation matrix**
Run:
```bash
UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py
cargo test -p molt-backend --features native-backend user_owned_symbol_whitelist_keeps_only_entry_roots -- --nocapture
MOLT_BACKEND_DAEMON=0 PYTHONPATH=src uv run --python 3.12 python3 -m molt.cli run --profile dev examples/hello.py
```
Expected: establish what is actually green today, not what old handoff notes claimed.

- [ ] **Step 4: Refresh targeted blocker evidence into canonical artifact roots**
Run:
```bash
MOLT_BACKEND_DAEMON=0 PYTHONPATH=src uv run --python 3.12 python3 tools/bench.py --bench tests/benchmarks/bench_sum.py --output bench/results/bench_native_refresh.json
PYTHONPATH=src uv run --python 3.12 python3 tools/bench_wasm.py --bench tests/benchmarks/bench_sum.py --linked --output bench/results/bench_wasm_refresh.json
```
Expected: fresh artifacts for the native release/daemon blocker and the wasm import blocker.

### Task 2: Close the immediate correctness and runtime blockers

**Files:**
- Modify: `src/molt/stdlib/datetime.py`
- Modify: `src/molt/stdlib/importlib/__init__.py`
- Modify: `src/molt/stdlib/importlib/machinery.py`
- Modify: `runtime/molt-runtime/src/`
- Modify: `tests/differential/basic/`
- Modify: `tests/differential/stdlib/`
- Modify: `tests/benchmarks/`

- [ ] **Step 1: Reproduce the `datetime.timedelta` positional-argument failure from the absorbed handoff note**
Run the exact failing binary or a minimized regression and capture the stack trace under `logs/`.

- [ ] **Step 2: Add a focused differential/native regression for that `datetime` constructor failure**
Expected: fail before the fix, then remain as a permanent guard.

- [ ] **Step 3: Reproduce the linked wasm `ImportError: No module named 'importlib.machinery'` failure**
Use the existing linked wasm targeted bench lane and a tiny import probe under `tests/differential/stdlib/`.

- [ ] **Step 4: Fix the importlib boundary, not a caller-specific shim**
Scope: keep version-gated absence behavior centralized in `src/molt/stdlib/importlib/__init__.py` and ensure `importlib.machinery` resolves in both native and wasm-supported lanes.

- [ ] **Step 5: Re-check backend-daemon lock contention as an infra bug, not a benchmark caveat**
If the daemon still stalls default release benchmarking, produce a minimal reproducer and fix the lock/state ownership in the daemon/build-state path before treating any benchmark lane as authoritative.

- [ ] **Step 6: Re-run targeted evidence**
Run:
```bash
MOLT_BACKEND_DAEMON=0 PYTHONPATH=src uv run --python 3.12 python3 -m pytest -q tests/differential/basic/re_parity.py tests/differential/basic/dataclasses_parity.py
PYTHONPATH=src uv run --python 3.12 python3 tools/bench_wasm.py --bench tests/benchmarks/bench_sum.py --linked --output bench/results/bench_wasm_postfix.json
```

### Task 3: Finish the partially landed architectural slices before new expansion

**Files:**
- Modify: `src/molt/cli.py`
- Modify: `runtime/molt-backend/src/main.rs`
- Modify: `tests/cli/test_cli_import_collection.py`
- Modify: `docs/OPERATIONS.md`
- Modify: `src/molt/frontend/__init__.py`
- Modify: `tests/test_frontend_midend_passes.py`

- [ ] **Step 1: Finish the remaining unchecked items in `docs/superpowers/plans/2026-03-26-stdlib-object-partition.md`**
Specifically: cache-mode versioning and explicit native link fingerprinting. The
native `emit=obj` partial-link contract is already landed and should now be
treated as baseline behavior, not an open design question.

- [ ] **Step 2: Audit recent TIR/mid-end emergency bypass commits**
Recent `main` history shows repeated `MOLT_TIR_SKIP` and loop/TIR bypass work. Reproduce the underlying failing cases and convert emergency skips into explicit, bounded policy with regression coverage.

- [ ] **Step 3: Decide the long-term contract for TIR optimization safety**
Either restore the passes with verifier-backed fixes or formalize a narrower profile-gated path. Do not leave silent skip creep in place.

- [ ] **Step 4: Re-run targeted frontend/backend validation**
Run:
```bash
UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/test_frontend_midend_passes.py tests/cli/test_cli_import_collection.py
cargo test -p molt-backend --features native-backend -- --nocapture
```

### Task 4: Align the grouped Linear workspace with real active blockers

**Files:**
- Modify: `docs/superpowers/plans/2026-03-26-linear-grouped-backlog.md`
- Modify: `ops/linear/seed_backlog.json`
- Modify: `ops/linear/manifests/index.json`
- Modify: `ops/linear/manifests/*.json`

- [ ] **Step 1: Treat `ops/linear` as the live execution index, not just TODO reflection**
Map active blockers from `docs/spec/STATUS.md`, `ROADMAP.md`, and this plan into grouped workstreams where the current manifests underrepresent them.

- [ ] **Step 2: Add or refresh grouped issues for currently missing control-plane work**
At minimum ensure live grouped coverage exists for:
  native benchmark/daemon contention,
  wasm parity/importlib blockers,
  TIR stabilization,
  stdlib partition completion.

- [ ] **Step 3: Refresh local artifacts and confirm deterministic convergence**
Run:
```bash
python3 tools/linear_hygiene.py refresh-local-artifacts --repo-root .
env -u LINEAR_API_KEY python3 tools/linear_workspace.py sync-index --team Moltlang --index ops/linear/manifests/index.json --update-existing --close-duplicates --close-missing --duplicate-state Canceled --dry-run
```

- [ ] **Step 4: Keep this plan and the grouped backlog in lockstep**
Whenever a workstream moves from blocker to active execution or from active to complete, update both the relevant plan/spec docs and the grouped manifest artifacts in the same change.

### Task 5: Resume roadmap execution in strict order

**Files:**
- Modify: `docs/spec/STATUS.md`
- Modify: `ROADMAP.md`
- Modify: `OPTIMIZATIONS_PLAN.md`
- Modify: `docs/benchmarks/optimization_progress.md`
- Modify: `docs/spec/areas/compat/`
- Modify: `runtime/molt-runtime/src/intrinsics/manifest.pyi`
- Modify: `runtime/molt-runtime/src/intrinsics/generated.rs`
- Modify: `src/molt/_intrinsics.pyi`

- [ ] **Step 1: Finish blocker-first runtime/stdlb parity items already called out by canonical docs**
Priority order:
  `datetime` correctness blocker,
  `importlib.machinery` wasm parity blocker,
  native daemon/bench reliability,
  remaining `stdlib-object-partition` tasks.

- [ ] **Step 2: Resume Rust-first stdlib lowering only after the blocker-first tranche is green**
Use the grouped Runtime & Intrinsics issue as the umbrella, but execute in bounded thematic slices with manifest/intrinsic regeneration and compatibility-doc refresh each time.

- [ ] **Step 3: Resume optimization Wave 0/1/2 only after release and wasm baselines are trustworthy again**
No new optimization landing without fresh perf artifacts plus correctness gates.

- [ ] **Step 4: Resume formal backlog only after active runtime/frontend behavior stops moving under it**
Use the existing Testing & Differential grouped issue for the M4 formalization blockers once implementation churn is back under control.

### Task 6: Final verification and reporting discipline

**Files:**
- Modify: `docs/spec/STATUS.md`
- Modify: `ROADMAP.md`
- Modify: `docs/ROADMAP_90_DAYS.md`
- Modify: `docs/benchmarks/optimization_progress.md`
- Modify: `tests/differential/INDEX.md`

- [ ] **Step 1: Run the required focused verification for the tranche you land**
Always include targeted pytest/cargo commands plus any differential/bench commands needed for the touched area.

- [ ] **Step 2: Update canonical docs in the same change**
If behavior or roadmap status changed, sync `docs/spec/STATUS.md`, `ROADMAP.md`, `docs/ROADMAP_90_DAYS.md`, and optimization docs before stopping.

- [ ] **Step 3: Record residual blockers explicitly**
If any claim still lacks proof, document the exact missing guarantee, its impact, and the next closure command instead of leaving another root-level handoff file.
