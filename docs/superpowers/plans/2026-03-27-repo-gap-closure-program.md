# ~~Molt Repo Gap Closure Program Implementation Plan~~ [SUPERSEDED]

> **SUPERSEDED** by Operation Greenfield (2026-03-27): see `docs/superpowers/specs/2026-03-27-operation-greenfield-design.md` and the Wave A/C/B plans.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert the current mix of blockers, partial implementations, compatibility debt, optimization proposals, and validation gaps into a deterministic execution program that first restores trusted green state and then compounds toward performance, portability, correctness, security, and binary-quality targets.

**Architecture:** This plan is intentionally split into sequential workstreams rather than one giant feature branch. The repo already has enough architecture and roadmap material; the problem is closure discipline. The correct execution order is: prove current truth, eliminate live blockers, finish half-landed architectural slices, close high-leverage parity gaps at the primitive/intrinsic boundary, then harden performance and verification so future gains are real and durable.

**Tech Stack:** Python CLI/frontend, Rust runtime/backend crates, differential harness, benchmark tooling, compatibility matrices, formal/fuzz tooling, native + wasm targets

---

## Program Rules

- Blocker-first. No new optimization wave lands while current native/wasm/runtime blockers remain unresolved.
- Primitive-first. Reusable semantics belong in Rust runtime/backend primitives, exposed by intrinsics, with Python wrappers limited to argument normalization and error mapping.
- Evidence-first. Any correctness or perf claim must cite fresh command output under canonical artifact roots.
- Canonical-doc sync is mandatory. When semantics or status change, update `docs/spec/STATUS.md`, `ROADMAP.md`, and the relevant compat/perf docs in the same change.
- Split execution into separate implementation plans when a workstream touches different ownership boundaries. This document is the master program, not a license to batch unrelated edits together.

### Task 1: Re-establish trusted ground truth

**Files:**
- Modify: `docs/spec/STATUS.md`
- Modify: `ROADMAP.md`
- Modify: `tests/differential/INDEX.md`
- Verify: `bench/results/`
- Verify: `logs/`

- [ ] **Step 1: Export canonical env roots**

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

- [ ] **Step 2: Capture current repo truth**

Run:
```bash
git status --short
git log --oneline -n 20
python3 tools/linear_hygiene.py refresh-local-artifacts --repo-root .
```

- [ ] **Step 3: Re-run the minimum high-signal validation slice**

Run:
```bash
UV_NO_SYNC=1 uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py
cargo test -p molt-backend --features native-backend user_owned_symbol_whitelist_keeps_only_entry_roots -- --nocapture
PYTHONPATH=src MOLT_BACKEND_DAEMON=0 UV_NO_SYNC=1 uv run --python 3.12 python3 -m molt.cli run --profile dev examples/hello.py
```

- [ ] **Step 4: Refresh the explicit blocker artifacts**

Run:
```bash
PYTHONPATH=src MOLT_BACKEND_DAEMON=0 UV_NO_SYNC=1 uv run --python 3.12 python3 tools/bench.py --bench tests/benchmarks/bench_sum.py --output bench/results/bench_native_refresh_20260327.json
PYTHONPATH=src UV_NO_SYNC=1 uv run --python 3.12 python3 tools/bench_wasm.py --bench tests/benchmarks/bench_sum.py --linked --output bench/results/bench_wasm_refresh_20260327.json
```

- [ ] **Step 5: Update canonical status docs with only freshly verified claims**

Files to sync:
- `docs/spec/STATUS.md`
- `ROADMAP.md`
- `tests/differential/INDEX.md`

### Task 2: Close the live P0/P1 blockers before expansion

**Files:**
- Modify: `src/molt/stdlib/datetime.py`
- Modify: `src/molt/stdlib/importlib/__init__.py`
- Modify: `src/molt/stdlib/importlib/machinery.py`
- Modify: `src/molt/cli.py`
- Modify: `runtime/molt-runtime/src/`
- Modify: `tests/differential/basic/`
- Modify: `tests/differential/stdlib/`
- Modify: `tests/benchmarks/`

- [ ] **Step 1: Reproduce and fix the `datetime.timedelta` constructor failure**

Required output:
- minimized regression
- stack trace logged under `logs/`
- permanent differential/native regression

- [ ] **Step 2: Reproduce and fix linked wasm `importlib.machinery` import failure at the importlib boundary**

Required output:
- import probe in `tests/differential/stdlib/`
- linked wasm bench rerun artifact
- no caller-specific shim

- [ ] **Step 3: Reproduce backend-daemon benchmark stall and either fix ownership/locking or explicitly downgrade daemon default for affected lanes**

Required output:
- minimal reproducer under `logs/`
- benchmark before/after or an explicit policy change documented in `STATUS.md` and `ROADMAP.md`

- [ ] **Step 4: Finish the remaining `stdlib-object-partition` tasks**

Required files:
- `src/molt/cli.py`
- `runtime/molt-backend/src/main.rs`
- `tests/cli/test_cli_import_collection.py`
- `docs/OPERATIONS.md`

Specific closures:
- versioned partition cache identity
- explicit link artifact list
- deterministic native link/object contracts, including the landed `emit=obj`
  partial-link path

### Task 3: Finish half-landed control-plane and wrapper slices

**Files:**
- Modify: `src/molt/cli.py`
- Modify: `tests/cli/test_cli_import_collection.py`
- Modify: `docs/OPERATIONS.md`
- Modify: `PRIMARY_HANDOFF.md`

- [ ] **Step 1: Unify `compare()` onto the shared build artifact contract**

- [ ] **Step 2: Add explicit Windows `.exe` coverage for `run` and `compare`**

- [ ] **Step 3: Add wasm explicit `--output` and unlinked `consumer_output` contract tests**

- [ ] **Step 4: Add Luau explicit `--output` contract tests**

- [ ] **Step 5: Harden Cloudflare negative-path containment and artifact validation**

- [ ] **Step 6: Harden backend-daemon readiness, stale-socket recovery, and quiet-by-default logging**

### Task 4: Convert roadmap debt into executable compatibility tranches

**Files:**
- Modify: `docs/spec/areas/compat/surfaces/language/type_coverage_matrix.md`
- Modify: `docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md`
- Modify: `docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md`
- Modify: `docs/spec/STATUS.md`
- Modify: `ROADMAP.md`
- Modify: `ops/linear/manifests/*.json`

- [ ] **Step 1: Treat `ROADMAP.md` P0/P1 items as the only active parity queue until validation is green**

- [ ] **Step 2: Break execution into these closure tranches**

Tranche order:
1. `SL1` intrinsic-only core stdlib closure
2. `TC1` exception and control-flow parity
3. `LF1/LF2` context managers, classes, descriptors, method binding
4. `SL2` regex/datetime/json/gc/socket ancillary parity
5. `RT2` async/runtime/GIL ownership closures
6. `RT2/RT3` wasm runtime-heavy parity blockers

- [ ] **Step 3: Refresh grouped Linear manifests so they represent real active work, not only the giant stdlib umbrella**

Must add visible grouped issues for:
- native/wasm blocker recovery
- wrapper/daemon control-plane hardening
- optimization verification and throughput
- formal/fuzz/translation-validation hardening

### Task 5: Close semantic debt at the IR/runtime boundary

**Files:**
- Modify: `src/molt/frontend/__init__.py`
- Modify: `runtime/molt-backend/src/native_backend/function_compiler.rs`
- Modify: `runtime/molt-backend/src/wasm.rs`
- Modify: `runtime/molt-runtime/src/object/ops.rs`
- Modify: `tests/test_frontend_midend_passes.py`
- Modify: `tests/differential/basic/`
- Modify: `docs/spec/areas/compiler/0100_MOLT_IR_IMPLEMENTATION_COVERAGE_2026-02-11.md`

- [ ] **Step 1: Finish semantic hardening for partial IR lanes already present in inventory**

Priority order:
- `CALL_INDIRECT`
- `INVOKE_FFI`
- `GUARD_TAG`
- `GUARD_DICT_SHAPE`
- `INC_REF` / `DEC_REF`
- `BORROW` / `RELEASE`
- `BOX` / `UNBOX` / `CAST` / `WIDEN`

- [ ] **Step 2: Replace backend panics on malformed IR/runtime edge cases with deterministic compile errors where reachable from user programs**

- [ ] **Step 3: Re-run IR probe gate and required dedicated-lane differentials**

Run:
```bash
UV_NO_SYNC=1 uv run --python 3.12 python3 tools/check_molt_ir_ops.py
UV_NO_SYNC=1 uv run --python 3.12 python3 -m pytest -q tests/test_frontend_midend_passes.py
MOLT_DIFF_MEASURE_RSS=1 MOLT_DIFF_RLIMIT_GB=10 UV_NO_SYNC=1 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic --jobs 1
```

### Task 6: Run the intrinsic-first stdlib closure program

**Files:**
- Modify: `src/molt/stdlib/**`
- Modify: `runtime/molt-runtime/src/intrinsics/manifest.pyi`
- Modify: `runtime/molt-runtime/src/**`
- Modify: `src/molt/_intrinsics.pyi`
- Modify: `runtime/molt-runtime/src/intrinsics/generated.rs`
- Modify: `docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md`

- [ ] **Step 1: Prioritize modules that are both performance-sensitive and dependency-heavy**

Order:
1. `functools`, `itertools`, `operator`
2. `math`, `json`, `datetime`, `gc`
3. `threading`, `asyncio`, `selectors`, `socket`
4. `importlib` family and metadata/resource surfaces
5. `collections`, `heapq`, `bisect`, `statistics`, `enum`, `random`

- [ ] **Step 2: Reject breadth-first import-only churn unless it unlocks a real dependency chain**

- [ ] **Step 3: For each stdlib module change, land the full intrinsic mapping in the same change**

Run:
```bash
python3 tools/gen_intrinsics.py
python3 tools/gen_stdlib_module_union.py
python3 tools/sync_stdlib_top_level_stubs.py --write
python3 tools/sync_stdlib_submodule_stubs.py --write
python3 tools/check_stdlib_intrinsics.py --update-doc
python3 tools/gen_compat_platform_availability.py --write
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
python3 tools/check_stdlib_intrinsics.py --critical-allowlist
```

### Task 7: Recover wasm parity as a first-class target, not an afterthought

**Files:**
- Modify: `runtime/molt-backend/src/wasm.rs`
- Modify: `runtime/molt-runtime/src/`
- Modify: `tests/test_wasm_*.py`
- Modify: `tests/differential/stdlib/`
- Modify: `docs/spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md`
- Modify: `docs/spec/STATUS.md`

- [ ] **Step 1: Clear the current runtime-heavy wasm blockers before adding new wasm features**

Named blockers from docs:
- `importlib.machinery`
- runtime-heavy asyncio loop mismatch
- linked-runtime table-ref traps
- zipimport parity
- unsupported thread-dependent lanes

- [ ] **Step 2: Separate true wasm limitations from fixable parity bugs with explicit capability/error contracts**

- [ ] **Step 3: Resume wasm size/startup work only after semantic parity lanes are green**

Priority later:
- shared trampoline helpers
- zero-copy string passing
- direct-linking restore
- function-table/reference-type improvements

### Task 8: Performance compounding after correctness closure

**Files:**
- Modify: `docs/benchmarks/optimization_progress.md`
- Modify: `bench/results/optimization_progress/`
- Modify: `runtime/molt-runtime/src/`
- Modify: `runtime/molt-backend/src/`
- Modify: `src/molt/frontend/__init__.py`

- [ ] **Step 1: Use the optimization plans as a menu, not as permission to batch changes**

Priority order:
1. startup/import-path overhead
2. lock-sensitive runtime hot paths
3. safe direct-linking / stdlib partition size wins
4. loop/vectorization and string kernels that have benchmarks
5. PGO release lane once baseline noise is controlled

- [ ] **Step 2: Require benchmark packet + correctness packet for every optimization landing**

Minimum packet:
- `tools/bench.py` or `tools/bench_wasm.py` artifact
- differential probe artifact
- binary size delta when relevant
- compile-time delta when relevant

- [ ] **Step 3: Prefer wins that reduce both startup and code size, not only steady-state throughput**

Examples:
- intrinsic import binding cost reduction
- module-init call traffic reduction
- outlined cold paths
- wasm trampoline deduplication
- stdlib partition deduplication

- [ ] **Step 4: Tighten multi-crate feature boundaries before any new size/startup campaign**

Concrete closure order:
- disable accidental subcrate default features at the top-level runtime boundary (`molt-runtime-crypto`, `molt-runtime-compression`, `molt-runtime-net`, `molt-runtime-path`, `molt-runtime-serial`, `molt-runtime-tk`)
- forward only the intended `stdlib_*` feature slices from `runtime/molt-runtime/Cargo.toml`
- keep wasm runtime artifact production explicit (`cargo rustc ... -- --crate-type=cdylib`) instead of relying on archive-only builds to somehow yield `molt_runtime.wasm`
- after the feature graph is explicit, measure `stdlib_micro`, linked wasm runtime size, split-runtime `molt_runtime.wasm`, and native startup deltas before doing broader refactors

### Task 9: Security, determinism, and runtime-safety hardening

**Files:**
- Modify: `docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md`
- Modify: `tools/ci_gate.py`
- Modify: `.github/workflows/*.yml`
- Modify: `runtime/molt-runtime/src/`
- Modify: `formal/`
- Modify: `fuzz/`

- [ ] **Step 1: Promote Miri/Kani/fuzz/translation-validation from side tooling into required change gates for runtime/backend work**

- [ ] **Step 2: Add targeted runtime-safety lanes for pointer registry, refcount, borrow/release, and attribute lookup**

- [ ] **Step 3: Expand reproducibility evidence for release-lane builds, including artifact hashes and environment capture**

- [ ] **Step 4: Keep dynamic execution and bridge work explicitly capability-gated and policy-deferred unless reprioritized**

### Task 10: Final integrated gate before any “major milestone complete” claim

**Files:**
- Modify: `docs/spec/STATUS.md`
- Modify: `ROADMAP.md`
- Modify: `docs/ROADMAP_90_DAYS.md`

- [ ] **Step 1: Run the minimum must-pass matrix**

Run:
```bash
cargo check -p molt-runtime -p molt-backend
UV_NO_SYNC=1 uv run --python 3.12 python3 tools/dev.py lint
UV_NO_SYNC=1 uv run --python 3.12 python3 tools/check_molt_ir_ops.py
UV_NO_SYNC=1 uv run --python 3.12 pytest -q tests/test_codec_lowering.py
MOLT_DIFF_MEASURE_RSS=1 MOLT_DIFF_RLIMIT_GB=10 MOLT_DIFF_TIMEOUT=180 UV_NO_SYNC=1 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic tests/differential/stdlib --jobs 1
UV_NO_SYNC=1 uv run --python 3.12 python3 tools/check_molt_ir_ops.py --require-probe-execution --probe-rss-metrics tmp/diff/rss_metrics.jsonl --failure-queue tmp/diff/failures.txt
UV_NO_SYNC=1 uv run --python 3.12 python3 tools/dev.py test
PYTHONPATH=src MOLT_PROFILE=1 MOLT_RUNTIME_FEEDBACK=1 MOLT_RUNTIME_FEEDBACK_FILE=target/molt_runtime_feedback_gate.json UV_NO_SYNC=1 uv run --python 3.12 python3 -m molt.cli run --profile dev examples/hello.py
UV_NO_SYNC=1 uv run --python 3.12 python3 tools/check_runtime_feedback.py target/molt_runtime_feedback_gate.json
```

- [ ] **Step 2: Re-run periodic parity and security lanes for milestone/release candidates**

Run:
```bash
UV_NO_SYNC=1 uv run --python 3.12 python3 tools/cpython_regrtest.py --uv --uv-python 3.12 --uv-prepare
cargo audit
cargo deny check
UV_NO_SYNC=1 uv run --python 3.12 pip-audit
```

- [ ] **Step 3: Update status only after the evidence is archived under canonical roots**

## Recommended Program Split

Do not execute this as one mega-change. Create separate implementation plans in this order:

1. blocker recovery (`datetime`, wasm `importlib.machinery`, daemon stall, stdlib partition finish)
2. wrapper/control-plane hardening (`compare`, Windows/wasm/Luau/Cloudflare contract coverage, daemon UX)
3. IR semantic hardening (`CALL_INDIRECT`, guards, ownership/conversion lanes)
4. stdlib intrinsic closure tranche 1 (`functools`/`itertools`/`operator`/`math`/`json`)
5. async + concurrency substrate closure (`threading`, `selectors`, `socket`, `asyncio`, wasm parity)
6. optimization tranche 1 (startup/import-path overhead, code size, lock-sensitive hot paths)
7. formal/fuzz/security gate promotion

## Evidence Used For This Program

- `docs/spec/STATUS.md`
- `ROADMAP.md`
- `docs/ROADMAP_90_DAYS.md`
- `PRIMARY_HANDOFF.md`
- `DICT_BUG.md`
- `docs/superpowers/plans/2026-03-26-stdlib-object-partition.md`
- `docs/superpowers/plans/2026-03-27-wrapper-artifact-contract.md`
- `docs/superpowers/plans/2026-03-27-molt-stabilization-and-roadmap-continuation.md`
- `docs/spec/areas/compiler/0100_MOLT_IR_IMPLEMENTATION_COVERAGE_2026-02-11.md`
- `docs/spec/areas/compat/surfaces/language/type_coverage_matrix.md`
- `docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md`
- `docs/spec/areas/perf/0601_BENCHMARK_HARNESS_AND_CI_GATES.md`
- `docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md`
- `docs/spec/areas/runtime/MEMORY_OPTIMIZATION_PLAN.md`
- `docs/spec/areas/runtime/SIMD_OPTIMIZATION_PLAN.md`
- `docs/spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md`
