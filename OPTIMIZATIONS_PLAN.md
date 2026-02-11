# Molt Optimization Program (Comprehensive)

Last updated: 2026-02-10
Owner: compiler + runtime + backend + stdlib + tooling
Status: Active

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
| OPT-1001 | Build Throughput and Determinism | P0 | Active | `dev` warm cache-hit <= 3.0s | compile progress KPI green for 7 consecutive runs |
| OPT-1002 | Core Primitive Lowering Expansion | P0 | Planned | reduce runtime-call density in hot numeric/control ops | no new regressions in core arithmetic/loop benches |
| OPT-1003 | Stdlib Rust Lowering Acceleration | P0 | Active | `python-only` modules from 25 -> <= 5 (shipped surface) | strict lowering gates green |
| OPT-1004 | Runtime Dispatch/Object Fast Paths | P1 | Planned | eliminate top native regressions (`attr_access`, `descriptor_property`, `struct`) | those benches >= 1.0x CPython |
| OPT-1005 | WASM Lowering and Runtime Parity | P0 | Planned | wasm/native ratio median < 2.5x | wasm no longer dominant bottleneck on top-10 slowest lanes |
| OPT-1006 | Data/Parsing/Container Kernel Program | P1 | Planned | close csv/tuple/deep-loop gaps | each lane >= 1.0x CPython or documented incompat-risk |
| OPT-1007 | Perf Governance and CI Guardrails | P0 | Active | prevent hidden regressions | budget checks enforced in CI and local tooling |
| OPT-1008 | Friend-Native Benchmark Program | P0 | Active | run Molt against friend-owned suites reproducibly | published scorecard with fair, apples-to-apples methodology |

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
- Keep benchmark and compile references synchronized with:
  - [Benchmarking and performance gates](docs/BENCHMARKING.md)
  - [Bench summary](docs/benchmarks/bench_summary.md)
  - [Compile progress tracker](docs/benchmarks/compile_progress.md)
  - [Stdlib intrinsics audit](docs/spec/areas/compat/0016_STDLIB_INTRINSICS_AUDIT.md)
  - [Rust lowering program](docs/spec/areas/compat/0026_RUST_LOWERING_PROGRAM.md)
