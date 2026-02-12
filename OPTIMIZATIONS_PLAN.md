# Molt Optimization Program (Comprehensive)

Last updated: 2026-02-12
Owner: compiler + runtime + backend + stdlib + tooling
Status: Phase 1 Week 1 Observability Completed; Week 0 baseline lock captured

## 0. Clean-Slate Kickoff (Assume No Prior Execution)

Kickoff date: 2026-02-11
Kickoff owner: compiler + runtime + backend + stdlib + tooling
Execution assumption (explicit): optimization implementation work is treated as not started.

### 0.1 Operating Rules For This Kickoff
- Existing optimization tracks in this document are treated as backlog/design scope until Week 0 evidence is captured.
- No optimization lane is counted as "started" until reproducible baseline artifacts exist for build, native runtime, and wasm runtime.
- We keep parity and correctness gates first (`docs/spec/STATUS.md` and differential suites), then optimization execution.

### 0.2 Kickoff Progress Log

| Date | Progress | Artifacts/Docs Updated | Notes |
| --- | --- | --- | --- |
| 2026-02-11 | Established clean-slate optimization kickoff and reset status assumptions for all optimization tracks. | `OPTIMIZATIONS_PLAN.md`, `docs/benchmarks/optimization_progress.md`, `ROADMAP.md`, `docs/spec/STATUS.md`, `README.md` | Planning + documentation only; benchmark execution has not started yet. |
| 2026-02-11 | Started Week 1 observability execution: runtime hot-path counters wired, JSON profile emission landed, and Codon subset profile artifacts captured. | `runtime/molt-runtime/src/constants.rs`, `runtime/molt-runtime/src/call/bind.rs`, `runtime/molt-runtime/src/builtins/attributes.rs`, `runtime/molt-runtime/src/object/ops.rs`, `bench/results/optimization_progress/2026-02-11_week1_observability/summary.md` | Validation gates passed (`cargo check -p molt-runtime -p molt-backend`; `uv run --python 3.12 pytest -q tests/test_codec_lowering.py` -> `33 passed`). |
| 2026-02-11 | Landed compiler mid-end CFG simplification expansion: executable-edge SCCP threading across loop/try paths, pre-SCCP structural canonicalization, loop-bound tuple extraction/proofs, alias-aware read CSE expansion (`GETATTR`/`LOAD_ATTR`/`INDEX`), and per-transform function-scoped acceptance/rejection telemetry. | `src/molt/frontend/__init__.py`, `tests/test_frontend_midend_passes.py`, `ROADMAP.md`, `docs/spec/STATUS.md`, `docs/spec/areas/compiler/0100_MOLT_IR.md` | Validation gates passed (`uv run --python 3.12 pytest -q tests/test_frontend_midend_passes.py tests/test_frontend_ir_alias_ops.py tests/test_frontend_builtin_call_lowering.py tests/test_check_molt_ir_ops.py` -> `64 passed`; `uv run --python 3.12 python3 tools/check_molt_ir_ops.py` -> `ok`; native smoke build/run `examples/hello.py` -> `42`). |
| 2026-02-11 | Tightened CFG rewrite correctness and specialization safety: balanced try-marker preservation in exceptional-edge pruning, explicit `CHECK_EXCEPTION` handler-edge threading into dominance-safe jumps, nested try/except multi-handler join normalization (label->jump trampoline threading) before CSE rounds, call-boundary read-heap invalidation, object-sensitive alias epochs, and SCCP range-safe `LEN`/`CONTAINS`/`INDEX` folds. | `src/molt/frontend/__init__.py`, `src/molt/frontend/cfg_analysis.py`, `tests/test_frontend_midend_passes.py`, `ROADMAP.md`, `docs/spec/STATUS.md`, `docs/spec/areas/compiler/0100_MOLT_IR.md` | Validation gates passed (`uv run --python 3.12 pytest -q tests/test_frontend_midend_passes.py tests/test_frontend_ir_alias_ops.py tests/test_frontend_builtin_call_lowering.py tests/test_check_molt_ir_ops.py` -> `71 passed`; `uv run --python 3.12 python3 tools/check_molt_ir_ops.py` -> `ok`; `PYTHONPATH=src uv run --python 3.12 python3 -m molt.cli run --profile dev examples/hello.py` -> `42`). |
| 2026-02-12 | Advanced fixed-point optimizer maturity: broadened read-heap CSE coverage (`MODULE_GET_ATTR`, `GETATTR_GENERIC_*`, `GETATTR_NAME_DEFAULT`, `GUARDED_GETATTR`), enforced hard read-key invalidation across uncertain call/FFI boundaries, expanded affine loop reasoning (recursive affine-term extraction + same-IV offset compare proofs + wider monotonic compare handling), strengthened PHI executable-edge simplification, and widened conservative LICM hoisting beyond loop-prefix-only scanning. | `src/molt/frontend/__init__.py`, `tests/test_frontend_midend_passes.py` | Validation gates passed (`uv run --python 3.12 pytest -q tests/test_frontend_midend_passes.py` -> `47 passed`; `uv run --python 3.12 pytest -q tests/test_frontend_ir_alias_ops.py tests/test_check_molt_ir_ops.py` -> `28 passed`; `uv run --python 3.12 python3 tools/check_molt_ir_ops.py` -> `ok`). |
| 2026-02-12 | Stabilized mid-end into deterministic fixed-point rounds with explicit fail-fast on non-convergence (`simplify -> SCCP/edge-thread -> join canonicalize -> prune -> verifier -> DCE -> CSE`), extended nested try/except check-edge ladder threading (including `CHECK_EXCEPTION` + same-target jump compaction), and expanded per-function attempted/accepted/rejected telemetry wiring for `edge_thread`, `loop_rewrite`, `cse_readheap`, `dce_pure_op`, `LICM`, and guard-hoist split counters. | `src/molt/frontend/__init__.py`, `tests/test_frontend_midend_passes.py`, `OPTIMIZATIONS_PLAN.md` | Validation gates passed (`uv run --python 3.12 pytest -q tests/test_frontend_midend_passes.py tests/test_frontend_ir_alias_ops.py tests/test_check_molt_ir_ops.py` -> `77 passed`; `uv run --python 3.12 python3 tools/check_molt_ir_ops.py` -> `ok`). |
| 2026-02-12 | Completed next optimizer expansion: loop/try structural threading now rewrites more `LOOP_END`/`LOOP_BREAK_IF_*` cases into direct label jumps when safe, SCCP now applies executable-predecessor-aware PHI lattice merges plus richer type-fact implications (`TYPE_OF`/`EQ` and selected `GUARD_TYPE` handling), alias-aware read CSE expanded to additional attr/container reads (`GETATTR_NAME`, `HASATTR_NAME`, `GETATTR_SPECIAL_OBJ`) with conservative immutable-class buckets, and region-level redundant guard elimination landed with write/call invalidation safety. | `src/molt/frontend/__init__.py`, `tests/test_frontend_midend_passes.py`, `OPTIMIZATIONS_PLAN.md` | Validation gates passed (`uv run --python 3.12 pytest -q tests/test_frontend_midend_passes.py tests/test_frontend_ir_alias_ops.py tests/test_check_molt_ir_ops.py` -> `81 passed`; `uv run --python 3.12 python3 tools/check_molt_ir_ops.py` -> `ok`). |
| 2026-02-12 | Landed pre-round full CFG canonicalizer before optimization rounds: canonicalization fixed-point now runs ahead of SCCP/CSE to align PHI arg shapes to CFG predecessors, thread/deepen try/label ladders, and prune dead label/jump scaffolding. Also upgraded CFG analysis metadata (`try_end_to_start`, `block_entry_label`) for stronger structural rewrites, tightened SCCP type/guard fact propagation on executable predecessors, and expanded read-heap CSE alias classes for additional attr/container forms. | `src/molt/frontend/__init__.py`, `src/molt/frontend/cfg_analysis.py`, `tests/test_frontend_midend_passes.py`, `OPTIMIZATIONS_PLAN.md` | Validation gates passed (`uv run --python 3.12 pytest -q tests/test_frontend_midend_passes.py tests/test_frontend_ir_alias_ops.py tests/test_check_molt_ir_ops.py` -> `84 passed`; `uv run --python 3.12 python3 tools/check_molt_ir_ops.py` -> `ok`). |
| 2026-02-12 | Reconciled SCCP non-hang safeguards with optimization throughput: replaced fixed tiny SCCP iteration limit with dynamic function-scaled cap, preserved conservative semantic fallback on cap hit, and wired cap-hit visibility into global/per-function telemetry and hotspot reporting (`sccp_iteration_cap_hits`) so regressions are actionable without risking compile stalls. | `src/molt/frontend/__init__.py`, `OPTIMIZATIONS_PLAN.md` | Validation gates passed (`uv run --python 3.12 pytest -q tests/test_frontend_midend_passes.py tests/test_frontend_ir_alias_ops.py tests/test_check_molt_ir_ops.py` -> `84 passed`; `uv run --python 3.12 python3 tools/check_molt_ir_ops.py` -> `ok`). |
| 2026-02-12 | Replaced SCCP full block-scan convergence loop with a queue-driven solver (`edge`, `value`, `block` worklists) so propagation only revisits impacted regions, reduced false cap-hit risk by counting transfer work (not every queue pop), and kept conservative cap fallback semantics intact. Added focused regression coverage for large CFG growth (no hang), semantic stability under forced cap fallback, and telemetry gating where cap hits are observed only in pathological runs. | `src/molt/frontend/__init__.py`, `tests/test_frontend_midend_passes.py`, `OPTIMIZATIONS_PLAN.md` | Validation gates passed (`uv run --python 3.12 pytest -q tests/test_frontend_midend_passes.py` -> `58 passed`; `uv run --python 3.12 pytest -q tests/test_frontend_ir_alias_ops.py tests/test_check_molt_ir_ops.py` -> `28 passed`; targeted SCCP regression subset -> `2 passed`). |

### 0.3 Week 0 Deliverables (Required Before Week 1 Execution)
- [x] Capture fresh build baseline (`tools/compile_progress.py`) with reproducibility metadata and artifact paths.
- [x] Capture fresh native benchmark baseline (`tools/bench.py`) with JSON output and summary snapshot.
- [x] Capture fresh wasm benchmark baseline (`tools/bench_wasm.py`) with JSON output and wasm/native ratio summary.
- [x] Capture current lowering/program counts (`0016_STDLIB_INTRINSICS_AUDIT.md`, runtime import-call density snapshot).
- [x] Publish a single Week 0 "baseline locked" entry in `docs/benchmarks/optimization_progress.md`.
- [x] Land and validate Week 1 observability primitives (runtime counters + machine-readable profile output) so baseline runs can attribute hot paths deterministically.

### 0.4 Definition Of "Optimization Work Started"
Optimization execution is considered started only when all are true:
1. Week 0 baselines are captured and linked in this plan.
2. At least one OPT track has a committed experiment plan with owner, KPI, and rollback switch.
3. Parity/correctness gates are recorded for the same revision used for baseline benchmarks.

### 0.5 Historical Content Handling
- The detailed OPT tracks below remain the canonical optimization scope.
- Their implementation status is reset to not started for this kickoff unless a new progress entry explicitly promotes a track state.

## 1A. Concrete 6-Week Execution Plan (Codon-Focused, Generalizable)

Last updated: 2026-02-11
Plan owner: compiler + runtime + backend + stdlib + tooling
Execution style: correctness-first, measurable, rollback-safe, no benchmark-only hacks

### Scope and North-Star Goals
- Keep `sum.py` at parity or faster while closing the largest gaps in `word_count.py` and `taq.py`.
- Convert benchmark-specific wins into reusable compiler/runtime primitives for ETL, analytics, service hot paths, and general Python workloads.
- Preserve semantics strictly (no host-Python runtime fallback, no behavior shortcuts).

### Baseline Snapshot (2026-02-11, runtime-only comparison)
- `sum.py`: Molt ~0.0113s vs Codon ~0.0115s (near parity/slightly faster).
- `word_count.py`: Molt ~0.0305s vs Codon ~0.0131s (~2.33x slower).
- `taq.py`: Molt ~0.0531s vs Codon ~0.0135s (~3.93x slower).
- Build times are tracked separately and are not included in runtime ratio calculations.

### Program-Level Guardrails (applies every week)
1. Correctness gate first:
   - `cargo check -p molt-runtime -p molt-backend`
   - `uv run --python 3.12 pytest -q tests/test_codec_lowering.py`
2. Performance gate second:
   - Run Codon subset (`sum.py`, `word_count.py`, `taq.py`) with stable samples.
   - Record JSON artifact under `bench/results/`.
3. Regression policy:
   - Stop on first regression in runtime, correctness, or differential behavior.
   - Fix/regress-proof before moving to the next cluster.
4. Generalization policy:
   - No optimization accepted if only usable by one benchmark shape.
   - Each landing must map to reusable lanes/primitives used by at least one non-benchmark workload family.

### Week-by-Week Plan

#### Week 1: Observability and Hot-Path Attribution
- Deliverables:
  - Extend runtime perf diagnostics with machine-readable per-kernel counters (dispatch, attr, alloc, deopt/fallback, hot primitive hit/miss).
  - Add benchmark artifact diff tooling for cluster-to-cluster trend checks.
  - Normalize benchmark methodology docs for reproducibility and runtime-vs-build clarity.
- Target outcomes:
  - Single-command evidence for top cost centers in `word_count` and `taq`.
  - Stable baseline artifacts for all later regression checks.
- Exit gate:
  - Metrics emitted and consumed in `bench/results/` workflow.

##### Week 1 Progress Snapshot (2026-02-11)
- Completed:
  - Added runtime counters for:
    - call-site IC effectiveness (`call_bind_ic_hit`, `call_bind_ic_miss`),
    - attribute site-name cache hit/miss,
    - split whitespace lane selection (`split_ws_ascii`, `split_ws_unicode`),
    - `dict[str] += int` prehash lane (`hit`, `miss`, `deopt`),
    - TAQ ingest path (`taq_ingest_calls`, `taq_ingest_skip_marker`),
    - ASCII `int` parse failures.
  - Extended `molt_profile_dump` to emit both:
    - existing human-readable `molt_profile ...` line, and
    - `molt_profile_json { ... }` payload when `MOLT_PROFILE_JSON=1`.
  - Captured reproducible profile artifacts for Codon subset:
    - `bench/results/optimization_progress/2026-02-11_week1_observability/sum_profile.log`
    - `bench/results/optimization_progress/2026-02-11_week1_observability/word_count_profile.log`
    - `bench/results/optimization_progress/2026-02-11_week1_observability/taq_profile.log`
    - summary: `bench/results/optimization_progress/2026-02-11_week1_observability/summary.md`
- Key evidence:
  - `word_count.py` primarily stresses `split_ws_ascii` (`20002` hits in sample).
  - `taq.py` primarily stresses fused ingest (`taq_ingest_calls=20001`) and currently deopts `dict[str] += int` prehash lane heavily (`dict_str_int_prehash_deopt=20000` in sample).
- Correctness/quality gates run and passing:
  - `cargo check -p molt-runtime -p molt-backend`
  - `uv run --python 3.12 pytest -q tests/test_codec_lowering.py`
- Next actions for Week 1 completion:
  - land benchmark artifact diff tooling (`tools/bench_diff.py`) for counter/time trend comparisons across runs, (done)
  - lock Week 0 baselines (build/native/wasm JSON) so Week 2 specialization can gate on stable before/after deltas. (done)
  - open dedicated wasm-stabilization clusters for linked-run fragility + wasm runtime parity failures observed in baseline lock.
  - extend wasm bench triage tooling to classify runner failures and run wasmtime controls on failed node cases (in progress via `tools/bench_wasm.py` updates).

#### Week 2: Typed Hot-Loop Specialization and Exception Regioning
- Deliverables:
  - Introduce stronger typed loop-region lowering for induction variables + arithmetic + bounds-hoisted loops.
  - Tighten `check_exception` placement to region boundaries where proven safe.
  - Add loop-shape regression tests for safe/unsafe control-flow boundaries.
- Target outcomes:
  - Reduced exception-check density in hot loops.
  - No semantic regressions in control-flow heavy modules.
- Exit gate:
  - Lowered IR shows reduced guard overhead with test parity intact.

#### Week 3: Text ETL Runtime Fast Paths (Reusable)
- Deliverables:
  - Extend fused text primitives (`split/find/count/tokenize + dict increment/update`) for broader ETL/API log usage.
  - Strengthen `dict[str] += int` lane behavior and fallbacks under mixed types.
  - Add kernel-level counters to verify hot-lane hit rates on real workloads.
- Target outcomes:
  - Material drop in object churn for `word_count` style loops.
  - Generalized speedups for text analytics pipelines beyond benchmark code.
- Exit gate:
  - `word_count.py` ratio improves with no loss in semantic coverage.

#### Week 4: TAQ/Data Ingest + Algorithmic Rolling Statistics
- Deliverables:
  - Expand fused ingest primitive design into generalized split/validate/parse/group/update kernels.
  - Implement and lower repeated slice-stat loops (`mean`/`stdev`) to rolling-window kernels (`O(n)` behavior).
  - Add strict numerical and error semantics tests for rolling stats.
- Target outcomes:
  - Major TAQ improvement by removing repeated slice recomputation overhead.
  - Reusable rolling-stats primitive for monitoring/anomaly/stream workloads.
- Exit gate:
  - `taq.py` ratio drops materially and rolling stats path is validated.

#### Week 5: Monomorphic/PIC Call-Attr Caches + Startup Cost Reductions
- Deliverables:
  - Upgrade call/attr IC architecture to bounded PIC behavior at stable hot sites with safe invalidation.
  - Freeze frequent intrinsic/stdlib bindings at init where safe; remove avoidable import-time heavy work.
  - Ensure lifecycle teardown clears all relevant caches deterministically.
- Target outcomes:
  - Reduced call dispatch and attr lookup costs in long-running services and hot scripts.
  - Lower startup/import overhead in common stdlib paths.
- Exit gate:
  - Reduced `call_dispatch` and `attr_lookup` counters on representative workloads.

#### Week 6: Architecture-Specific Kernelization + Hardening
- Deliverables:
  - Finalize architecture-tuned kernel paths (portable baseline + SIMD/multiversion lanes).
  - Add explicit native-arch benchmark profile playbook and reporting standards.
  - Run full validation sweep and document final before/after deltas.
- Target outcomes:
  - Best practical performance on production hardware while preserving portability lane.
  - Clear release-ready evidence and rollback strategy.
- Exit gate:
  - All weekly gates green; top benchmark targets improved with no semantic regressions.

### Concrete Targets by End of Week 6
- `sum.py`: maintain parity or better.
- `word_count.py`: move from ~2.3x slower toward <= 1.3x slower.
- `taq.py`: move from ~3.9x slower toward <= 1.8x slower.
- Maintain zero tolerance for semantic regressions and hidden fallback behavior.

### Risks and Mitigations
- Risk: Over-specialization that misses real-world patterns.
  - Mitigation: require each primitive to demonstrate benefit on at least one non-benchmark workload class.
- Risk: IC/cache invalidation correctness bugs.
  - Mitigation: strict mutation/version guards, conservative deopt path, stress tests.
- Risk: Architecture-specific wins reduce portability.
  - Mitigation: portable default lane required; native-arch lane opt-in and measured.
- Risk: Guard-hoisting introduces subtle exception behavior changes.
  - Mitigation: conservative proofs, differential tests, stop-on-regression policy.

### Reporting Cadence for This 6-Week Plan
- Weekly status update in this document:
  - work completed
  - benchmark deltas
  - regressions encountered/fixed
  - next week adjustments
- Artifact requirements:
  - JSON benchmark reports under `bench/results/`
  - any methodology changes reflected in `docs/BENCHMARKING.md`

## 1. Objectives

- Increase primitive lowering coverage across core Python ops and stdlib so compiled binaries run with minimal dynamic/runtime-call overhead.
- Improve build throughput and iteration speed without sacrificing determinism.
- Raise native and wasm performance together, with parity and regression gates.
- Keep correctness first: no semantic shortcuts, no host-Python fallback.

## 2. Current Baseline (Evidence)

### Benchmarking Canonical Docs
- [Benchmarking and performance gates](docs/BENCHMARKING.md)
- [Bench summary (latest combined native+wasm report)](docs/benchmarks/bench_summary.md)
- [Compile progress tracker](docs/benchmarks/compile_progress.md)

### Build Pipeline
- Source: [Compile progress tracker](docs/benchmarks/compile_progress.md)
- `dev` warm cache-hit build: 6.121s (target <= 3.0s, yellow).
- `dev` warm no-cache daemon-on: 8.150s (target <= 15.0s, green).
- `release` warm no-cache: 3.033s (target <= 18.0s, green).
- `release-fast` warm cache-hit: 2.033s (green).
- `hello.py` native IR size reduced from 40.923MB to 5.289MB; ops from 409900 to 50483 after init-closure tightening.

### Runtime / Performance
- Source: [Bench summary](docs/benchmarks/bench_summary.md)
- 45/45 native and 45/45 wasm benches passing.
- Median native speedup vs CPython: 3.75x.
- Median wasm speedup vs CPython: 0.47x.
- Median wasm/native ratio: 4.81x.
- Native regressions remain in attribute/descriptor/struct/tuple/deep-loop/csv/fib lanes.

### Lowering Coverage Signals
- Stdlib audit source: `/Users/adpena/PycharmProjects/molt/docs/spec/areas/compat/0016_STDLIB_INTRINSICS_AUDIT.md`
- Audited modules: 112 total.
- `intrinsic-backed`: 58.
- `intrinsic-partial`: 16.
- `probe-only`: 13.
- `python-only`: 25.

- Core lowering program source: `/Users/adpena/PycharmProjects/molt/docs/spec/areas/compat/0026_RUST_LOWERING_PROGRAM.md`
- Phase 2 (concurrency substrate) is active: `socket` -> `threading` -> `asyncio`.

- Backend call density (current implementation signal):
  - ~393 imported runtime functions declared in native backend (`runtime/molt-backend/src/lib.rs`).
  - ~365 unique wasm import ids used (`runtime/molt-backend/src/wasm.rs`).
  - ~415 wasm import-call sites (`emit_call(... import_ids[...])`).
- Frontend fast-int hinting is intentionally narrow today (`ADD/SUB/MUL` and selected compares).

## 3. Program Board (End-to-End)

| ID | Track | Priority | Status | Primary KPI | Exit Gate |
| --- | --- | --- | --- | --- | --- |
| OPT-1001 | Build Throughput and Determinism | P0 | Not Started (Kickoff Reset) | `dev` warm cache-hit <= 3.0s | compile progress KPI green for 7 consecutive runs |
| OPT-1002 | Core Primitive Lowering Expansion | P0 | Not Started (Kickoff Reset) | reduce runtime-call density in hot numeric/control ops | no new regressions in core arithmetic/loop benches |
| OPT-1003 | Stdlib Rust Lowering Acceleration | P0 | Not Started (Kickoff Reset) | `python-only` modules from 25 -> <= 5 (shipped surface) | strict lowering gates green |
| OPT-1004 | Runtime Dispatch/Object Fast Paths | P1 | Not Started (Kickoff Reset) | eliminate top native regressions (`attr_access`, `descriptor_property`, `struct`) | those benches >= 1.0x CPython |
| OPT-1005 | WASM Lowering and Runtime Parity | P0 | Not Started (Kickoff Reset) | wasm/native ratio median < 2.5x | wasm no longer dominant bottleneck on top-10 slowest lanes |
| OPT-1006 | Data/Parsing/Container Kernel Program | P1 | Not Started (Kickoff Reset) | close csv/tuple/deep-loop gaps | each lane >= 1.0x CPython or documented incompat-risk |
| OPT-1007 | Perf Governance and CI Guardrails | P0 | Not Started (Kickoff Reset) | prevent hidden regressions | budget checks enforced in CI and local tooling |
| OPT-1008 | Friend-Native Benchmark Program | P0 | Not Started (Kickoff Reset) | run Molt against friend-owned suites reproducibly | published scorecard with fair, apples-to-apples methodology |

---

## OPT-1001: Build Throughput and Determinism

### Problem Statement
- Iteration time is still inconsistent under contention even though several no-cache lanes are now green.
- Developers need predictable and fast inner-loop builds, especially with many concurrent agents.

### Current Evidence
- `dev` warm cache-hit: 6.121s (target <= 3.0s).
- Release lanes can hit host-level interruption/timeout in contended runs.
- Backend/codegen still dominates diagnostics in some runs.

### Hypotheses
- H1: Build graph invalidation is broader than necessary for warm cache-hit paths.
- H2: Backend daemon/dispatch queue behavior under contention still causes tail-latency spikes.
- H3: Shared target/cache lock contention causes avoidable serialization.

### Alternative Implementations
- A1: Incremental key-scope tightening and finer-grained invalidation for frontend+backend artifacts.
  - Expected speed impact: +20% to +45% on warm cache-hit.
  - Memory impact: neutral to mild increase for metadata.
  - Complexity: medium.
- A2: Queue-aware backend daemon scheduler (priority lanes for local hot builds).
  - Expected speed impact: lower p95/p99 latency under contention.
  - Memory impact: moderate daemon state growth.
  - Complexity: medium-high.
- A3: Target-dir sharding by profile/workload class with adaptive lock backoff.
  - Expected speed impact: better throughput in multi-agent runs.
  - Memory impact: higher disk footprint.
  - Complexity: medium.

### Benchmarking Matrix
- Baseline: `tools/compile_progress.py` full suite.
- Metrics: median/p95 build latency, cache-hit ratio, timeout/retry count.
- Workloads: `examples/hello.py`, representative stdlib-heavy script, differential shard build.
- Expected deltas:
  - A1: -30% warm cache-hit median (confidence: medium).
  - A2: -40% p95 tail under contention (confidence: medium).
  - A3: -20% multi-agent wall-time (confidence: low-medium).

### Risk and Rollback Plan
- Risks: stale-cache correctness, daemon instability, disk blowup.
- Mitigations: checksum guardrails, fallback to no-daemon lane, cache pruning policy.
- Rollback: disable new cache keys/scheduler by env flag and revert to current lock strategy.

### Integration Steps
1. Add build-key attribution report to diagnostics output.
2. Ship invalidation tightening in guarded slices (frontend, then backend).
3. Add queue-aware daemon scheduling and lock telemetry.
4. Promote only after 7-run stability pass.

### Validation Checklist
- [ ] Compile progress KPIs green including warm cache-hit target.
- [ ] No correctness drift in `tools/dev.py test` and differential smoke.
- [ ] Timeout/retry events reduced in contention probes.

---

## OPT-1002: Core Primitive Lowering Expansion

### Problem Statement
- A large fraction of core ops still lower to runtime calls rather than primitive inlined lanes.
- Primitive lowering currently focuses on a narrow subset (notably `ADD/SUB/MUL` and selected comparisons).
- This limits speedups for loops, tuple-heavy code, bit ops, and numeric branches.

### Current Evidence
- Frontend `_should_fast_int(...)` is narrow (`ADD`, `SUB`, `MUL`, inplace variants, `LT`, `EQ`, `NE`).
- Backend includes int fast paths in some arithmetic ops, but many operators still call runtime imports (bitwise, shifts, `div`/`floordiv`/`mod`/`pow`, etc.).
- Native regressions are concentrated in workloads likely affected by call-heavy dynamic paths.

### Hypotheses
- H1: Expanding typed/guarded primitive lanes for integer/boolean/control hot ops will reduce dispatch overhead materially.
- H2: Value-shape/monomorphic guards can keep correctness while still enabling aggressive inlining.
- H3: Primitive lowering in lockstep for native+wasm avoids widening the current wasm gap.

### Alternative Implementations
- A1: Extend current `fast_int` style hints + guard blocks to more op families.
  - Expected speed impact: +10% to +40% in deep-loop and numeric lanes.
  - Memory impact: small code-size growth.
  - Complexity: medium.
- A2: Introduce typed SSA lanes (e.g., `i64`, `bool`) through frontend IR and lower directly in backend.
  - Expected speed impact: +20% to +60% in core loops and arithmetic chains.
  - Memory impact: moderate codegen complexity, possible IR size increase initially.
  - Complexity: high.
- A3: Hybrid: widen `fast_int` now, then migrate to explicit typed SSA by phase.
  - Expected speed impact: near-term gains plus long-term maintainability.
  - Complexity: medium-high, best practical path.

### Benchmarking Matrix
- Baseline benches: `bench_deeply_nested_loop`, `bench_fib`, `bench_tuple_pack`, `bench_tuple_index`, `bench_try_except`, `bench_sum_list`.
- Metrics: wall-time, branch miss proxy (where available), generated IR size, runtime call count per op trace.
- Expected deltas:
  - A1: +15% median on target set (confidence: medium).
  - A2: +30% median on target set (confidence: low-medium initially).
  - A3: +15% then +30% phased (confidence: medium).

### Risk and Rollback Plan
- Risks: semantic mismatches (overflow/sign/exception behavior), code-size growth.
- Mitigations: guarded lowering, differential tests by op family, per-op kill-switch flags during rollout.
- Rollback: disable widened primitive lanes for specific op groups.

### Integration Steps
1. Expand frontend hint policy to include bitwise/shift/comparison families where safe.
2. Add backend primitive blocks for those op kinds with strict guards.
3. Add typed-lane design doc and prototype in one hot path (loop arithmetic).
4. Roll out to native+wasm together, gated by per-family perf and parity checks.

### Validation Checklist
- [ ] Differential op-family tests green (3.12/3.13/3.14).
- [ ] No regression in correctness edge cases (negative shifts/division/mod semantics).
- [ ] Target regression benches move to >= 1.0x CPython.

---

## OPT-1003: Stdlib Rust Lowering Acceleration

### Problem Statement
- Too many stdlib modules remain `intrinsic-partial`, `probe-only`, or `python-only`, limiting both capability and performance in compiled mode.
- Full lowering is mandatory for production-grade compiled execution semantics.

### Current Evidence
- 58 intrinsic-backed, 16 intrinsic-partial, 13 probe-only, 25 python-only.
- Program phase sequencing already exists and is active for concurrency substrate.

### Hypotheses
- H1: Moving high-fanout stdlib dependencies to intrinsic-backed status will reduce import/runtime overhead and unblock more optimizations.
- H2: Enforcing transitive strict-import closure for critical roots avoids accidental fallback drift and improves determinism.

### Alternative Implementations
- A1: Strict phase order from spec (socket/selectors -> threading -> asyncio -> P1 families).
  - Expected speed impact: medium across real workloads via fewer fallback wrappers.
  - Complexity: medium.
- A2: Popularity-driven lowering (json/csv/pickle first).
  - Expected speed impact: strong in data workloads.
  - Complexity: medium, but higher risk to core runtime sequencing.
- A3: Blended queue: keep phase order for P0 substrate while running independent P1/P2 modules in parallel owners.
  - Expected speed impact: highest throughput without violating substrate dependencies.
  - Complexity: high coordination.

### Benchmarking Matrix
- Baseline: import latency for core packages, startup and steady-state app workloads, differential stdlib suite.
- Metrics: import time, runtime allocations, intrinsic miss/failure counts.
- Expected deltas:
  - A1: lower risk, steady compatibility gains.
  - A3: faster total program completion with similar quality.

### Risk and Rollback Plan
- Risks: partial lowering that looks complete, capability-gating drift.
- Mitigations: mandatory manifest + generated bindings + strict gates in CI.
- Rollback: revert module-level lowering changes; keep required-missing intrinsic raising behavior.

### Integration Steps
1. Keep 0026 program order as canonical.
2. Create module-family work packets with explicit intrinsic manifests.
3. Require native+wasm parity test per promoted module.
4. Reduce `python-only` count every sprint and publish scoreboard.

### Validation Checklist
- [ ] `tools/check_stdlib_intrinsics.py` and `tools/check_core_lane_lowering.py` green.
- [ ] Promoted modules documented in status/roadmap.
- [ ] No host-Python fallback paths introduced.

---

## OPT-1004: Runtime Dispatch/Object Fast Paths

### Problem Statement
- Attribute access, descriptor/property, struct-like objects, and tuple operations remain major native regressions.

### Current Evidence
- Regressions: `bench_attr_access` (0.40x), `bench_descriptor_property` (0.44x), `bench_struct` (0.20x), tuple lanes (~0.42x).

### Hypotheses
- H1: Shape-aware inline caches for attribute and descriptor lookup will remove repeated dynamic lookups.
- H2: Structified field layout and monomorphic call sites can remove boxing/lookup overhead.
- H3: Tuple pack/index/slice lanes need specialized kernels and allocation reuse.

### Alternative Implementations
- A1: PEP659-style adaptive inline caches for load/store attr and method calls.
- A2: Stronger class-layout metadata with stabilized shapes and direct slot offsets.
- A3: Combined cache+layout plan with tiered invalidation.

### Benchmarking Matrix
- Target benches: `bench_attr_access`, `bench_descriptor_property`, `bench_struct`, `bench_tuple_pack`, `bench_tuple_index`.
- Metrics: runtime, cache hit rate, invalidation frequency, allocation count.
- Expected deltas:
  - A1: +25% to +80% in attr/descriptor lanes.
  - A2: +20% to +70% in struct/tuple-object-heavy lanes.

### Risk and Rollback Plan
- Risks: stale cache invalidation bugs.
- Mitigations: guard/version checks on type dict mutation, conservative deopt path.
- Rollback: disable cache tier via env flag and retain semantic slow path.

### Integration Steps
1. Add cache instrumentation counters.
2. Land read-only attr cache tier.
3. Extend to descriptor/method calls with mutation invalidation.
4. Add tuple allocation reuse and fixed-layout path.

### Validation Checklist
- [ ] Target regression lanes reach >= 1.0x CPython.
- [ ] Differential attribute/descriptor mutation tests green.
- [ ] Cache invalidation correctness stress tests green.

---

## OPT-1005: WASM Lowering and Runtime Parity

### Problem Statement
- WASM is far behind native (median wasm/native 4.81x), especially in high-frequency call and string lanes.

### Current Evidence
- Worst wasm/native ratios include channel throughput and multiple string/search lanes (up to ~44x).
- Backend currently emits a high number of imported calls in wasm.

### Hypotheses
- H1: Imported-call density dominates wasm overhead.
- H2: Primitive lowering expansion and reduced boundary crossings will yield outsized wasm gains.
- H3: Link-time and table/relocation tuning can reduce runtime overhead and code size.

### Alternative Implementations
- A1: Lower more core ops directly in wasm backend with guard blocks.
- A2: Batch/runtime ABI redesign to reduce import-call granularity.
- A3: Keep A1/A2 plus wasm link/profile tuning (`wasm-ld`, table base, opt passes).

### Benchmarking Matrix
- Target benches: wasm slowest 10 lanes from bench summary.
- Metrics: wasm runtime, import-call count, code size, wasm/native ratio.
- Expected deltas:
  - A1: -20% to -40% wasm runtime on call-heavy lanes.
  - A2: -30% to -60% on boundary-heavy lanes.

### Risk and Rollback Plan
- Risks: wasm-specific semantic drift, ABI churn.
- Mitigations: shared semantic tests, native+wasm lockstep review for each lowered op family.
- Rollback: keep compatibility ABI path behind feature switch.

### Integration Steps
1. Add import-call count to perf diagnostics.
2. Prioritize lowering for hottest wasm call clusters.
3. Tune link/profile settings on representative workloads.
4. Require wasm/native improvement before broad rollout.

### Validation Checklist
- [ ] wasm/native median ratio < 2.5x on benchmark suite.
- [ ] No native regressions introduced by shared lowering changes.
- [ ] wasm differential parity remains green.

---

## OPT-1006: Data/Parsing/Container Kernel Program

### Problem Statement
- CSV parsing and tuple/loop-heavy paths still underperform despite strong wins in some vector and string kernels.

### Current Evidence
- `bench_csv_parse`: 0.50x.
- `bench_csv_parse_wide`: 0.26x.
- `bench_deeply_nested_loop`: 0.31x.
- strong existing wins in sum/vector/string lanes show optimization potential.

### Hypotheses
- H1: Parser/tokenizer and allocation patterns dominate csv and tuple regressions.
- H2: Loop-carried dynamic dispatch and boxing overhead dominate deep nested loops.

### Alternative Implementations
- A1: Native parser kernels and fast tokenizer primitives for csv paths.
- A2: Container specialization for tuple pack/index loops and stack promotion where safe.
- A3: Workload-driven microkernel approach with per-lane instrumentation and phased landing.

### Benchmarking Matrix
- Metrics: runtime, allocations, bytes copied, branch behavior, generated IR size.
- Workloads: csv micro+macro datasets, tuple-heavy synthetic and real scripts.
- Expected deltas:
  - A1: +1.5x to +3x in csv lanes.
  - A2: +1.2x to +2x in tuple/deep-loop lanes.

### Risk and Rollback Plan
- Risks: spec drift in parser edge cases.
- Mitigations: strict differential fixture expansion before promotion.
- Rollback: keep current parser path as strict fallback (still intrinsic-backed only).

### Integration Steps
1. Build parser/tokenizer profiling corpus.
2. Implement and benchmark one kernel at a time.
3. Expand differential edge-case corpus before each rollout.
4. Promote only with stable gains across both narrow and wide csv workloads.

### Validation Checklist
- [ ] csv and tuple target benches >= 1.0x CPython.
- [ ] Parser correctness edge cases green.
- [ ] No regressions in existing high-win lanes.

---

## OPT-1007: Performance Governance and Guardrails

### Problem Statement
- Optimization velocity is high; without strong governance, regressions can land unnoticed across lanes or targets.

### Current Evidence
- Existing summaries are strong but still require manual triage and cross-lane interpretation.

### Hypotheses
- H1: Automated budget checks per benchmark cluster will reduce regression escape rate.
- H2: Separate gates for throughput, runtime speed, and lowering coverage will keep priorities balanced.

### Alternative Implementations
- A1: Static threshold budgets for all benches and compile KPIs.
- A2: Rolling-window control limits per benchmark (median and p95).
- A3: Hybrid: static red lines + rolling warning bands.

### Benchmarking Matrix
- Metrics: speedup ratios, compile times, wasm/native ratio, intrinsic coverage counts.
- Expected deltas: lower perf-regression incidents and faster triage.

### Risk and Rollback Plan
- Risks: flaky perf gates causing noisy CI.
- Mitigations: warmup normalization, rerun policy, lane-specific confidence thresholds.
- Rollback: convert hard-fail to soft-warn for unstable lanes until stabilized.

### Integration Steps
1. Define red-line thresholds for critical benchmark families.
2. Add automated extraction and dashboard from benchmark JSON outputs.
3. Gate merges for P0 lanes; warn-only for unstable lanes until confidence rises.

### Validation Checklist
- [ ] CI emits clear pass/fail/warn by lane.
- [ ] Regression triage time reduced.
- [ ] Guardrails include build, runtime, wasm parity, and lowering coverage.

---

## OPT-1008: Friend-Native Benchmark Program (Use Their Own Suites)

### Problem Statement
- Current benchmarking is strong internally, but we need external validity by running Molt against friends on each friend's own benchmark suite.
- Without this, we can miss workload classes friends optimize for and under-prioritize high-impact gaps.

### Current Evidence
- Existing baseline workflows are documented in [Benchmarking and performance gates](docs/BENCHMARKING.md).
- Current bench reports are Molt-centric and do not yet include a standardized friend-owned-suite scoreboard.
- Phase 1 scaffolding is now implemented: `tools/bench_friends.py`, `bench/friends/manifest.toml`, and published summary target `docs/benchmarks/friend_summary.md`.

### Target Friend Set (Initial)
- Codon
- PyPy
- Nuitka
- Cython
- Numba

### Hypotheses
- H1: Friend-owned suites will expose different hot paths (startup, specialization, object model, parser/tokenizer, numeric kernels) than Molt's current suite.
- H2: A fair, pinned harness will produce stable comparisons that are actionable for roadmap prioritization.
- H3: Cross-suite wins will correlate more strongly with real adoption than single-suite microbench improvements.

### Alternative Implementations
- A1: One unified harness that checks out each friend benchmark suite at pinned commits and runs with a common protocol.
  - Expected speed impact: none directly; high prioritization quality gain.
  - Maintenance impact: medium-high.
- A2: Per-friend adapters first, then unify after method stabilizes.
  - Expected speed impact: none directly; fastest path to first data.
  - Maintenance impact: medium.
- A3: Start with one deep friend lane (Codon), then expand to others after governance settles.
  - Expected speed impact: none directly; lowest initial complexity.
  - Maintenance impact: low initially, medium later.

### Fairness and Reproducibility Protocol
- Pin friend benchmark repo commit SHAs and toolchain versions.
- Use identical hardware, isolated runs, fixed CPU/power settings where possible, and repeat-count standards (`--super` equivalent where available).
- Record compile time and run time separately when a friend has compilation.
- Keep semantic constraints explicit:
  - `runs_unmodified`
  - `requires_adapter`
  - `unsupported_by_molt` (with reason)
- Publish full command lines and environment in machine-readable metadata.

### Benchmarking Matrix
- Baseline docs/method: [Benchmarking and performance gates](docs/BENCHMARKING.md)
- Metrics:
  - runtime median/p95
  - compile/build time (if applicable)
  - throughput/latency where suite defines them
  - geometric-mean speedup vs CPython and vs friend
  - pass/fail/unsupported coverage counts
- Workloads:
  - friend-owned suites (pinned revisions)
  - Molt internal suites (for continuity)
- Expected deltas:
  - Near-term: better prioritization and clearer optimization ROI.
  - Mid-term: measurable closing of top friend gaps in shared workload families.

### Risk and Rollback Plan
- Risks: apples-to-oranges comparisons, benchmark harness drift, friend-suite licensing/compat issues.
- Mitigations: pinned manifests, transparent scoring rules, per-suite adapter audit logs.
- Rollback: keep friend results as advisory-only until reproducibility and fairness gates are stable.

### Integration Steps
1. [x] Add a `bench/friends/manifest.toml` with pinned suites, commits, and canonical run commands.
2. [x] Add `tools/bench_friends.py` to orchestrate checkout, environment setup, run execution, and artifact capture.
3. Emit results under `bench/results/friends/<timestamp>/` with JSON + markdown summaries.
4. Generate `docs/benchmarks/friend_summary.md` with:
   - top wins/losses
   - per-suite coverage
   - reproducibility metadata
5. Add CI lane (nightly) and local lane (on-demand) with stable rerun policy.
6. Feed top loss clusters directly into OPT-1002/1004/1006 prioritization.

### Validation Checklist
- [ ] Friend manifest is fully pinned and reproducible.
- [ ] At least one full run per friend suite completes with published artifacts.
- [ ] Summary distinguishes runtime vs compile-time comparisons clearly.
- [ ] Results are actionable (each top loss mapped to an optimization owner/track).

---

## 4. Primitive-Lowering Expansion Roadmap (What to Add Now)

### Phase A (Immediate, 1-2 weeks)
- Expand frontend lowering hints beyond current narrow set for clearly safe integer and boolean op families.
- Add backend guarded primitive paths for bitwise/shift and selected numeric operators where semantics are fully defined.
- Add instrumentation for runtime import-call density per benchmark run.

### Phase B (Near-term, 2-4 weeks)
- Introduce typed SSA lanes for hot loop kernels.
- Apply same lowering lanes in native and wasm backends.
- Add deopt/guard counters to identify unstable specialization points.

### Phase C (Programmatic, 1-2 months)
- Convert high-fanout stdlib module families to intrinsic-backed state in phase order.
- Replace remaining probe-only/python-only modules in shipped surface according to roadmap priorities.
- Tie lowering-completion milestones to benchmark goals and release gates.

## 5. Success Criteria (Program)

- Build:
  - `dev` warm cache-hit <= 3.0s sustained on compile progress tracker.
- Runtime native:
  - Remove current P0 regressions to >= 1.0x CPython in target lanes.
- WASM:
  - Improve median wasm/native ratio from 4.81x to < 2.5x.
- Lowering:
  - `python-only` stdlib modules reduced from 25 to <= 5 in shipped compiled surface.
  - measurable reduction in backend runtime-call/import-call density on hot lanes.
- Governance:
  - perf and lowering gates enforced in CI with clear red-line thresholds.
- Friend benchmarking:
  - friend-owned suite scorecard published and reproducible with pinned manifests.
  - top 10 loss clusters mapped into active optimization tracks each sprint.

## 6. Reporting Cadence

- Update this plan at least once per optimization PR touching compiler/backend/runtime/stdlib hot paths.
- Record every optimization milestone and benchmark run in `docs/benchmarks/optimization_progress.md`.
- Keep benchmark and compile references synchronized with:
  - [Benchmarking and performance gates](docs/BENCHMARKING.md)
  - [Bench summary](docs/benchmarks/bench_summary.md)
  - [Compile progress tracker](docs/benchmarks/compile_progress.md)
  - [Stdlib intrinsics audit](docs/spec/areas/compat/0016_STDLIB_INTRINSICS_AUDIT.md)
  - [Rust lowering program](docs/spec/areas/compat/0026_RUST_LOWERING_PROGRAM.md)
