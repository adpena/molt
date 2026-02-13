# Molt Roadmap (Active)

Canonical current status: [docs/spec/STATUS.md](docs/spec/STATUS.md). This roadmap is forward-looking.

## Planning Doc Hierarchy
- Current state and capabilities: [docs/spec/STATUS.md](docs/spec/STATUS.md) (canonical source of truth).
- Active project plan and backlog: [ROADMAP.md](ROADMAP.md) (this file).
- Near-term sequencing and execution windows: [docs/ROADMAP_90_DAYS.md](docs/ROADMAP_90_DAYS.md).
- Optimization strategy and track scope: [OPTIMIZATIONS_PLAN.md](OPTIMIZATIONS_PLAN.md).
- Optimization execution history and artifacts: [docs/benchmarks/optimization_progress.md](docs/benchmarks/optimization_progress.md).
- Month 1 enforcement artifacts:
  - [docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md](docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md)
  - [docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md](docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md)
- Historical/detail roadmap archive: [docs/ROADMAP.md](docs/ROADMAP.md) (retained for context; do not treat as canonical state).

## Legend
- **Status:** Implemented (done), Partial (some semantics missing), Planned (scoped but not started), Missing (no implementation), Divergent (intentional difference from CPython).
- **Priority:** P0 (blocker), P1 (high), P2 (medium), P3 (lower).
- **Tier/Milestone:** `TC*` (type coverage), `SL*` (stdlib), `DB*` (database), `DF*` (dataframe/pandas), `LF*` (language features), `RT*` (runtime), `TL*` (tooling), `M*` (syntax milestones).

## Strategic North-Star
- Performance target: parity with or superiority to Codon on tracked benches.
- Compatibility target: near-Nuitka CPython coverage + interoperability for
  Molt-supported semantics, without violating Molt break-policy constraints.

## Optimization Program Kickoff (2026-02-11)
- Week 1 observability is complete and Week 0 baseline-lock artifacts are captured.
- Canonical optimization scope: [OPTIMIZATIONS_PLAN.md](OPTIMIZATIONS_PLAN.md).
- Canonical execution log and milestone history: [docs/benchmarks/optimization_progress.md](docs/benchmarks/optimization_progress.md).
- Current Week 1 evidence artifact: [bench/results/optimization_progress/2026-02-11_week1_observability/summary.md](bench/results/optimization_progress/2026-02-11_week1_observability/summary.md).
- Week 0 baseline lock summary: [bench/results/optimization_progress/2026-02-11_week0_baseline_lock/baseline_lock_summary.md](bench/results/optimization_progress/2026-02-11_week0_baseline_lock/baseline_lock_summary.md).
- Guard/deopt feedback artifact wiring is available: runtime emits
  `molt_runtime_feedback.json` when `MOLT_RUNTIME_FEEDBACK=1`
  (override path with `MOLT_RUNTIME_FEEDBACK_FILE`), and schema validation is
  enforced via `tools/check_runtime_feedback.py` including required
  `deopt_reasons.call_indirect_noncallable` and
  `deopt_reasons.invoke_ffi_bridge_capability_denied`, plus
  `deopt_reasons.guard_tag_type_mismatch` and
  `deopt_reasons.guard_dict_shape_layout_mismatch` with guard-layout
  mismatch breakdown counters (`*_null_obj`, `*_non_object`,
  `*_class_mismatch`, `*_non_type_class`,
  `*_expected_version_invalid`, `*_version_mismatch`).
- Week 2 readiness note: baseline gate is satisfied; prioritize specialization + wasm-stabilization clusters from the lock summary failure lists.

## Stdlib Intrinsics Program (2026-02-12)
- Canonical plan: [docs/spec/areas/compat/0028_STDLIB_INTRINSICS_EXECUTION_PLAN.md](docs/spec/areas/compat/0028_STDLIB_INTRINSICS_EXECUTION_PLAN.md).
- Hard gate contract in `tools/check_stdlib_intrinsics.py` now includes:
  - zero `probe-only`,
  - zero `python-only`,
  - intrinsic-partial ratchet budget (`tools/stdlib_intrinsics_ratchet.json`),
  - fallback anti-pattern blocking for `_py_*` direct/dynamic imports.
- Blocker-first tranche update:
  - landed importlib blocker/resolver hardening (`importlib.machinery` + `importlib.util`)
    with regression tests and targeted differential evidence.
  - `concurrent.futures` currently intrinsic-backed; `pickle` remains intrinsic-partial.
  - wasm-linked build blocker fixed in `tools/wasm_link.py`: malformed UTF-8
    function-name entries in optional `name` sections no longer hard-fail table-ref append.
  - wasm runner hardening landed: deterministic Node resolver (`MOLT_NODE_BIN`
    + auto-select Node >= 18) and explicit `run_wasm.js` WASI fallback
    (`node:wasi` -> `wasi`) with actionable error text.
  - wasm socket constants payload now exports required CPython-facing names
    (`AF_INET`, `SOCK_STREAM`, `SOL_SOCKET`, etc.) from runtime intrinsic
    `molt_socket_constants`.
  - linked-wasm asyncio table-ref trap is closed: poll dispatch now uses
    runtime table-base addressing + legacy-slot normalization, linked artifacts
    export `molt_set_wasm_table_base`, and scheduler execution no longer
    recursively acquires `task_queue_lock`.
  - runtime-heavy wasm regression lane is green for this blocker tranche:
    `tests/test_wasm_runtime_heavy_regressions.py` now passes on
    asyncio/zipimport/smtplib targeted cases.
  - runtime-heavy/data/metadata-email/tooling clusters remain intrinsic-partial and are the active burn-down queue.
- Current snapshot: `intrinsic-backed=0`, `intrinsic-partial=873`,
  `probe-only=0`, `python-only=0`; strict gate keeps modules/submodules
  intrinsic-partial until full CPython 3.12+ parity/TODO burn-down is complete.
- Current wasm blockers before runtime-heavy promotion:
  - thread-dependent stdlib server paths remain capability/host blocked on wasm
    by design (`NotImplementedError: threads are unavailable in wasm`), so
    full server parity for these lanes still requires an explicit wasm threading
    strategy.
  - Node/V8 Zone OOM remains reproducible on some linked runtime-heavy modules
    (`zipfile`/`zipimport` family) even with single-task wasm compilation.
- Weekly scoreboard (required): track
  `intrinsic-backed`, `intrinsic-partial`, `probe-only`, `python-only`,
  missing required top-level/submodule entries, native pass %, wasm pass %, and
  memory regressions.

## 90-Day Priority Queue: Molt IR Spec Closure (2026-02-11)
- Source audit: [docs/spec/areas/compiler/0100_MOLT_IR_IMPLEMENTATION_COVERAGE_2026-02-11.md](docs/spec/areas/compiler/0100_MOLT_IR_IMPLEMENTATION_COVERAGE_2026-02-11.md).
- Historical baseline snapshot (pre-closure audit): 109 implemented,
  13 partial, 12 missing.
- Current inventory gate (`tools/check_molt_ir_ops.py`) reports
  `missing=0` for spec-op presence in frontend emit/lowering coverage, and
  required dedicated-lane presence in native + wasm backends, plus
  behavior-level semantic assertions for dedicated call/guard/ownership/
  conversion lanes.
- 2026-02-11 implementation update: dedicated frontend/lowering/backend lanes are
  now present for `CALL_INDIRECT`, `INVOKE_FFI`, `GUARD_TAG`,
  `GUARD_DICT_SHAPE`, `INC_REF`/`DEC_REF`/`BORROW`/`RELEASE`, and conversion
  ops (`BOX`/`UNBOX`/`CAST`/`WIDEN`); semantic hardening and differential
  evidence remain in progress.
- Behavior-level lane regression tests are in
  `tests/test_frontend_ir_alias_ops.py` for raw emit + lowered lane presence
  (`call_indirect`, `guard_tag`, `guard_dict_shape`, ownership lanes, and
  conversion lanes).
- Differential parity evidence now includes dedicated-lane probes:
  `tests/differential/basic/call_indirect_dynamic_callable.py`,
  `tests/differential/basic/call_indirect_noncallable_deopt.py`,
  `tests/differential/basic/invoke_ffi_os_getcwd.py`,
  `tests/differential/basic/invoke_ffi_bridge_capability_enabled.py`,
  `tests/differential/basic/invoke_ffi_bridge_capability_denied.py`,
  `tests/differential/basic/guard_tag_type_hint_fail.py`, and
  `tests/differential/basic/guard_dict_shape_mutation.py`.
- CI enforcement update (2026-02-11): after `diff-basic`, CI now runs
  `tools/check_molt_ir_ops.py --require-probe-execution` against
  `rss_metrics.jsonl` + `ir_probe_failures.txt`, making required probe
  execution/failure-queue linkage a hard gate.
- `INVOKE_FFI` hardening update (2026-02-11): frontend now tags bridge-policy
  invocations with a dedicated lane marker (`s_value="bridge"`), native/wasm
  backends route through `molt_invoke_ffi_ic`, and runtime enforces
  `python.bridge` capability in non-trusted mode for bridge-tagged calls.
- `CALL_INDIRECT` hardening update (2026-02-11): native/wasm backends route
  `call_indirect` through dedicated `molt_call_indirect_ic` /
  `call_indirect_ic` lanes with explicit callable precheck before IC dispatch.
- Frontend mid-end update (2026-02-11): `SimpleTIRGenerator.map_ops_to_json`
  now runs a lightweight optimization pipeline before lowering
  (`_coalesce_check_exception_ops` + CFG/dataflow mid-end). The mid-end now
  builds explicit basic blocks, computes CFG successors/predecessors,
  dominators, and backward liveness, then applies deterministic fixed-point
  passes (`simplify -> SCCP -> canonicalize -> DCE`) with SCCP sparse lattice
  propagation (`unknown`/`constant`/`overdefined`) over SSA names and now
  tracks executable CFG edges explicitly (edge-filtered predecessor merges).
  SCCP coverage now includes arithmetic, boolean, comparison, `TYPE_OF`,
  `CONTAINS`/`INDEX` constant-folding, selected `ISINSTANCE` folds, and
  selected guard/type fact propagation (including guard-failure edge
  termination). It now tracks both try exceptional and try normal completion
  facts and uses them for explicit try-edge threading. Control simplification
  now threads executable edges across `IF`, `LOOP_BREAK_IF_*`, `LOOP_END`,
  and `TRY_*`, applies deeper loop/try rewrites (including conservative
  dead-backedge loop marker flattening and dead try-body suffix pruning after
  proven guard/raise exits), and performs region-aware CFG simplification across
  `IF`/`ELSE`, `LOOP_*`, `TRY_*`, and `LABEL`/`JUMP` regions (including
  dead-label pruning and no-op jump elimination). A structural pre-SCCP
  canonicalization round now strips degenerate empty branch/loop/try regions
  before each SCCP round. The pass also adds conservative branch-tail merging +
  loop-invariant pure-op hoisting and runs effect-aware CSE/DCE under CFG
  safety checks. Read-heap CSE now uses conservative
  alias/effect classes (`dict`/`list`/`indexable`/`attr`) so unrelated writes
  no longer invalidate all read value numbers, including global reuse for
  `GETATTR`/`LOAD_ATTR`/`INDEX` reads under no-interfering-write guards.
  Read-heap invalidation now treats call/invoke operations as conservative
  write barriers, and class-level alias epochs are augmented with lightweight
  object-sensitive epochs for higher hit-rate without unsafe reuse.
  Exceptional try-edge pruning now preserves balanced `TRY_START`/`TRY_END`
  structure unless dominance/post-dominance plus pre-trap
  `CHECK_EXCEPTION`-free proofs permit marker elision.
  The mid-end now also models explicit `CHECK_EXCEPTION` CFG branch targets and
  threads proven exceptional checks into direct `JUMP` edges to handler labels
  with dominance-safe guards, and normalizes nested try/except multi-handler
  join trampolines (label->jump chains) before CSE rounds.
  Expanded cross-block value reuse remains explicitly gated by a CFG
  definite-assignment verifier with automatic fallback to safe mode when proof
  fails. Loop analysis now tracks `(start, step, bound, compare-op)` tuples for
  affine induction facts and monotonic loop-bound proofs used by SCCP. CFG
  construction now lives in a dedicated
  `src/molt/frontend/cfg_analysis.py` module with explicit `BasicBlock` and
  `CFGGraph` structures; mid-end telemetry now reports expanded-mode acceptance
  plus per-transform diagnostics (`sccp_branch_prunes`,
  `loop_edge_thread_prunes`, `try_edge_thread_prunes`,
  `unreachable_blocks_removed`, `cfg_region_prunes`, `label_prunes`,
  `jump_noop_elisions`, `licm_hoists`, `guard_hoist_*`, `gvn_hits`,
  `dce_removed_total`) through `MOLT_MIDEND_STATS=1`. Function-scoped
  acceptance/attempt telemetry is now tracked in `midend_stats_by_function`
  (`sccp`, `edge_thread`, `loop_rewrite`, `guard_hoist`, `cse`,
  `cse_readheap`, `gvn`, `licm`, `dce`, `dce_pure_op`) with
  attempted/accepted/rejected breakdown for transform families. It currently
  elides trivial `PHI`
  nodes, proven no-op `GUARD_TAG` checks, and redundant branch-symmetric guards,
  with join preservation across structured
  `IF`/`ELSE`, `LOOP_*`, `TRY_*`, and `LABEL`/`JUMP` regions; regression coverage
  lives in
  `tests/test_frontend_midend_passes.py`.
- P0 closure items (dedicated lanes landed; semantic/deopt and differential
  coverage hardening remain):
  - `CallIndirect`, `InvokeFFI`, `GuardTag`, `GuardDictShape`.
- P1 ownership/LIR gaps:
  - `IncRef`, `DecRef`, `Borrow`, `Release`.
- P2 conversion and canonicalization gaps:
  - `Box`, `Unbox`, `Cast`, `Widen` and alias-name normalization for partial ops.
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): complete `CALL_INDIRECT` hardening with broader deopt reason telemetry (dedicated runtime lane, noncallable differential probe, CI-enforced probe execution/failure-queue linkage, and runtime-feedback counter `deopt_reasons.call_indirect_noncallable` are landed).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): complete `INVOKE_FFI` hardening with broader deopt reason telemetry (bridge-lane marker, runtime capability gate, negative capability differential probe, CI-enforced probe execution/failure-queue linkage, and runtime-feedback counter `deopt_reasons.invoke_ffi_bridge_capability_denied` are landed).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): harden `GUARD_TAG` specialization/deopt semantics + coverage (runtime-feedback counter `deopt_reasons.guard_tag_type_mismatch` is landed).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): harden `GUARD_DICT_SHAPE` invalidation/deopt semantics + coverage (runtime-feedback aggregate counter `deopt_reasons.guard_dict_shape_layout_mismatch` and per-reason breakdown counters are landed).
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): enforce explicit LIR ownership invariants for `INC_REF`/`DEC_REF` across frontend/backend with differential parity evidence.
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): enforce borrow/release lifetime invariants for `BORROW`/`RELEASE` with safety checks and parity coverage.
- TODO(compiler, owner:compiler, milestone:LF2, priority:P2, status:partial): add generic conversion ops (`BOX`, `UNBOX`, `CAST`, `WIDEN`) with deterministic semantics and native/wasm parity coverage.
- TODO(compiler, owner:compiler, milestone:LF2, priority:P2, status:partial): normalize alias op naming (`BRANCH`/`RETURN`/`THROW`/`LOAD_ATTR`/`STORE_ATTR`/`CLOSURE_LOAD`/`CLOSURE_STORE`) or codify canonical aliases in `0100_MOLT_IR`.
- TODO(compiler, owner:compiler, milestone:LF2, priority:P1, status:partial): extend sparse SCCP beyond current arithmetic/boolean/comparison/type-of coverage into broader heap/call-specialization families and a stronger loop-bound solver for cross-iteration constant reasoning.
- TODO(compiler, owner:compiler, milestone:LF2, priority:P1, status:partial): extend loop/try edge threading beyond current executable-edge + conservative loop-marker rewrites into full loop-end and exceptional-handler CFG rewrites with dominance/post-dominance preservation.
- Implemented: CI hardening for `tools/check_molt_ir_ops.py` now includes mandatory `--require-probe-execution` after `diff-basic`, so required-probe execution status and failure-queue linkage regressions fail CI.

## Compiler Optimization Stabilization Tranche (2026-02-12)
- Priority override: recover frontend/mid-end compile throughput while preserving correctness and deterministic outputs.
- Current regression signal from active runs: stdlib-heavy module lowering tails dominate compile time and can timeout before wasm/native execution in no-cache bench paths.
- Current tranche status: profile plumbing, tier classification, per-function budget/degrade ladder, per-pass timing/hotspot telemetry, CLI diagnostics sink integration, and deterministic process-level parallel lowering (opt-in) are landed in frontend/CLI. Latest tightening pass now defaults stdlib functions to Tier C unless explicitly promoted, adds finer stage-level/pre-pass budget degrade checkpoints, and applies stdlib-aware effective min-cost thresholds in layer-parallel policy diagnostics; remaining work is broader parallel eligibility and diagnostics UX refinement.
- Execution order (implementation slices):
  1. Profile-gated policy matrix (`dev` cheap/correctness-first, `release` full fixed-point).
  2. Tiered optimization policy (Tier A hot, Tier B normal, Tier C heavy dependency/stdlib).
  3. Per-function budgets with degrade ladder (disable expensive transforms first, never correctness gates).
  4. Per-pass wall-time telemetry and top-offender diagnostics.
  5. Process-level parallel module lowering with deterministic merge order.
  6. Optional large-function optimization workers and staged Rust kernel migration.
- Exit criteria:
  - deterministic second-run IR stability,
  - reduced p95 frontend lowering latency on stdlib-heavy modules,
  - verifier fallback/correctness regressions do not increase.
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): ship profile-gated mid-end policy matrix (`dev` correctness-first cheap opts; `release` full fixed-point) with deterministic pass ordering and explicit diagnostics (CLI->frontend profile plumbing is landed; diagnostics sink expansion remains).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): add tiered optimization policy (Tier A entry/hot functions, Tier B normal user functions, Tier C heavy stdlib/dependency functions) with deterministic classification and override knobs (baseline deterministic classifier + env overrides are landed; telemetry-driven hotness promotion remains).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): enforce per-function mid-end wall-time budgets with an automatic degrade ladder that disables expensive transforms before correctness gates and records degrade reasons (budget/degrade ladder is landed in fixed-point loop; heuristic tuning + diagnostics surfacing remains).
- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): add per-pass wall-time telemetry (`attempted`/`accepted`/`rejected`/`degraded`, `ms_total`, `ms_p95`) plus top-offender diagnostics by module/function/pass (frontend per-pass timing/counters + hotspot rendering are landed; CLI/JSON sink wiring remains).
- TODO(compiler, owner:compiler, milestone:TL2, priority:P0, status:partial): root-cause/fix mid-end miscompiles feeding missing values into runtime lookup/call sites (temporary hard safety gates keep dev-profile mid-end off by default unless `MOLT_MIDEND_DEV_ENABLE=1`, and keep stdlib modules out of canonicalization by default in all profiles).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): surface active optimization profile/tier policy and degrade events in CLI build diagnostics and JSON outputs for deterministic triage (diagnostics sink now includes profile/tier/degrade summaries + pass hotspots; remaining work is richer UX controls/verbosity partitioning).
- TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): add process-level parallel frontend module-lowering and deterministic merge ordering, then extend to large-function optimization workers where dependency-safe (dependency-layer process-pool lowering is landed behind `MOLT_FRONTEND_PARALLEL_MODULES`; remaining work is broader eligibility and worker-level tuning telemetry).
- TODO(compiler, owner:compiler, milestone:LF3, priority:P1, status:planned): migrate hot mid-end kernels (CFG build, SCCP lattice transfer, dominator/liveness) to Rust with Python orchestration preserved for policy control.

## Parity-First Execution Plan
Guiding principle: lock CPython parity and robust test coverage before large optimizations or new higher-level surface area.

Parity gates (required before major optimizations that touch runtime, call paths, lowering, or object layout):
- Relevant matrix entries in [docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md](docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md), [docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md](docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md),
  [docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md](docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md), [docs/spec/areas/compat/0021_SYNTACTIC_FEATURES_MATRIX.md](docs/spec/areas/compat/0021_SYNTACTIC_FEATURES_MATRIX.md), and
  [docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md](docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md) are updated to match the implementation status.
- Differential tests cover normal + edge-case behavior (exception type/messages, ordering, and protocol fallbacks).
- Native + WASM parity checks added or updated for affected behaviors.
- Runtime lifecycle plan tracked and up to date ([docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md](docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md)).

Plan (parity-first, comprehensive):
1) Matrix audit and coverage map: enumerate missing/partial cells in the matrices above, link each to at least one
   differential test, and ensure TODOs exist in code for remaining gaps.
2) Core object protocols: attribute access/descriptor binding, dunder fallbacks, container protocols
   (`__iter__`/`__len__`/`__contains__`/`__reversed__`), equality/ordering/hash/format parity, and strict exception behavior.
3) Call + iteration semantics: CALL_BIND/CALL_METHOD, `*args`/`**kwargs`, iterator error propagation, generators,
   coroutines, and async iteration; keep native + WASM parity in lockstep.
4) Stdlib core: builtins + `collections`/`functools`/`itertools`/`operator`/`heapq`/`bisect` to parity per
   [docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md](docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md), with targeted differential coverage.
5) Security + robustness tests: capability gating, invalid input handling, descriptor edge cases, and recursion/stack
   behavior to catch safety regressions early.

## Concurrency & Parallelism (Vision -> Plan)
- Default: CPython-correct asyncio semantics on a single-threaded event loop (deterministic ordering, structured cancellation).
- True parallelism is explicit: executors + isolated runtimes/actors with message passing.
- Shared-memory parallelism is opt-in, capability-gated, and limited to explicitly safe types.
- Current: runtime mutation is serialized by a GIL-like lock in the global runtime state; see [docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md](docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md).

Planned milestones:
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P0, status:planned): Rust event loop + I/O poller with cancellation propagation and deterministic scheduling guarantees; expose as asyncio core.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P0, status:planned): full asyncio parity (tasks, task groups, streams, subprocess, executors) built on the runtime loop.
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:planned): define the per-runtime GIL strategy, runtime instance ownership model, and allowed cross-thread object sharing rules (see [docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md](docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md)).
- Implemented: explicit `PyToken` GIL token API and `with_gil`/`with_gil_entry` enforcement on runtime mutation entrypoints (see [docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md](docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md)).
- TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:planned): parallel runtime tier with isolated heaps/actors, explicit message passing, and capability-gated shared-memory primitives.
- TODO(wasm-parity, owner:runtime, milestone:RT3, priority:P1, status:planned): wasm host parity for the asyncio runtime loop, poller, sockets, and subprocess I/O.

## Performance
- Vector reduction kernels now cover `sum`/`prod`/`min`/`max` plus float `sum` lanes (list/tuple/range variants), with adaptive lane gating counters (`MOLT_ADAPTIVE_VEC_LANES`) to reduce failed-probe overhead while preserving generic fallbacks.
- Range materialization now has a dedicated runtime lane (`list_from_range`) used by `list(range(...))` and simple `[i for i in range(...)]` comprehensions to remove generator/list-append call overhead from hot loops.
- Dict increment idioms (`d[k] = d.get(k, 0) + delta`) now lower to a dedicated runtime lane (`dict_inc`) with int fast path + generic add fallback.
- Fused split+count lanes (`string_split_ws_dict_inc`, `string_split_sep_dict_inc`) now include a string-key dict probe fast path (hash+byte compare) with explicit fallback to generic dict semantics for mixed/non-string-key maps.
- Iterable element hints now propagate through for-loop lowering (including `file_text`/`file_bytes` iterables), unlocking broader split/find/count primitive lowering in ETL-style loops without manual type hints.
- `statistics.mean/stdev` on slice expressions now lower to dedicated runtime lanes (`statistics_mean_slice`, `statistics_stdev_slice`) with list/tuple fast paths and runtime-owned generic fallback for non-list/tuple inputs.
- Slice statistics lanes now include int/float element fast-coercion in hot loops (generic numeric fallback preserved).
- `abs(...)` now lowers to a dedicated runtime lane (`abs`) to remove dynamic-call overhead from numeric hot loops.
- `dict.setdefault(key, [])` now lowers to a dedicated lane (`dict_setdefault_empty_list`) that avoids eager empty-list allocation and reduces grouping overhead in ETL-style loops.
- String kernel SIMD paths cover find/split/replace with Unicode-safe index translation; next: Unicode index caches and wider SIMD (TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): Unicode index caches + wider SIMD).
- TODO(perf, owner:compiler, milestone:RT2, priority:P1, status:planned): reduce startup/import-path dispatch overhead for stdlib-heavy scripts (bind intrinsic-backed imports at lower cost and trim module-init call traffic) so wins translate to short-lived CLI/data scripts as well as long-running services.
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): implement sharded/lock-free handle resolution and track lock-sensitive benchmark deltas (attr access, container ops).
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): stream print writes to avoid building intermediate output strings for large payloads.
- TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): pre-size `dict.fromkeys` using iterable length hints to reduce rehashing.
- Implemented: websocket readiness integration via io_poller for native + wasm (`molt_ws_wait_new`) to avoid busy-polling and enable batch wakeups.
- Implemented: release iteration compile profile lane via Cargo `release-fast`, including dedicated compile-progress measurement cases (`release_fast_cold`, `release_fast_warm`, `release_fast_nocache_warm`) for before/after release-lane comparison.
- TODO(perf, owner:runtime, milestone:RT3, priority:P2, status:planned): cache mio websocket poll streams/registrations to avoid per-wait `TcpStream` clones.
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): re-enable safe direct-linking by relocating the runtime heap base or enforcing non-overlapping memory layouts to avoid wasm-ld in hot loops.
- Implemented: removed linked-wasm static intrinsic dispatch workaround for channel intrinsics by canonicalizing the runtime channel-handle ABI to 64-bit bits values, restoring stable dynamic intrinsic call dispatch.
- Implemented: use i32 locals for wasm pointer temporaries in the backend to trim wrap/extend churn.
- Wasmtime host runner is available (`molt-wasm-host`) with shared memory/table wiring and a `tools/bench_wasm.py --runner wasmtime` path for perf comparison against Node.
- Implemented: Wasmtime DB host delivery is non-blocking via `molt_db_host_poll` with stream semantics + cancellation checks; parity coverage still pending.

## Type Coverage
- memoryview (Partial): multi-dimensional `format`/`shape`/`strides`/`nbytes` + `cast`, tuple scalar indexing, 1D slicing/assignment for bytes/bytearray-backed views.
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:missing): memoryview multi-dimensional slicing + sub-views (C-order parity).
- Implemented: BigInt heap fallback + arithmetic parity beyond 47-bit inline ints.
- Implemented: class objects + basic descriptors (`classmethod`, `staticmethod`, `property`) + `__set_name__` hook.
- Implemented: C3 MRO + multiple inheritance for attribute lookup + `super()` resolution + data descriptor precedence.
- Implemented: reflection builtins (`type`, `isinstance`, `issubclass`, `object`) for base chains (no metaclasses).
- Implemented: BaseException root + exception chaining (`__cause__`, `__context__`, `__suppress_context__`) + `__traceback__` objects with line markers + StopIteration.value propagation.
- Implemented: ExceptionGroup/except* semantics (match/split/derive/combine) with BaseExceptionGroup hierarchy + try/except* lowering (native + wasm).
- TODO(semantics, owner:runtime, milestone:TC2, priority:P1, status:partial): tighten exception `__init__` + subclass attribute parity (ExceptionGroup tree).
- Implemented: dict subclass storage lives outside instance `__dict__`, matching CPython attribute/mapping separation.
- TODO(introspection, owner:runtime, milestone:TC2, priority:P1, status:partial): expand frame/traceback objects to CPython parity (`f_back`, `f_globals`, `f_locals`, live `f_lasti`/`f_lineno`).
- Implemented: descriptor deleter semantics (`__delete__`, property deleter) + attribute deletion wiring.
- Implemented: set literals/constructor with add/contains/iter/len + algebra (`|`, `&`, `-`, `^`); `frozenset` constructor + algebra.
- Implemented: augassign slice targets (`seq[a:b] += ...`) with extended-slice length checks.
- Implemented: format mini-language for ints/floats + f-string conversion flags (`!r`, `!s`, `!a`) + `str.format` field parsing (positional/keyword, attr/index, conversion flags, nested specs).
- Implemented: call argument binding for Molt functions (positional/keyword/`*args`/`**kwargs`) with pos-only/kw-only enforcement.
- Implemented: variadic call trampoline lifts compiled call-arity ceiling beyond 12 (native + wasm).
- Implemented: PEP 649 lazy annotations (`__annotate__` + lazy `__annotations__` cache for module/class/function; VALUE/STRING formats).
- Implemented: PEP 585 generic aliases for builtin containers (`list`/`dict`/`tuple`/`set`/`frozenset`/`type`) with `__origin__`/`__args__`.
- Implemented: PEP 584 dict union (`|`, `|=`), PEP 604 union types (`X | Y`), and zip(strict) (PEP 618).
- TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:partial): derive `types.GenericAlias.__parameters__` from `TypeVar`/`ParamSpec`/`TypeVarTuple` once typing metadata lands.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): implement full PEP 695 type params (bounds/constraints/defaults, ParamSpec/TypeVarTuple, alias metadata).
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P1, status:partial): implement `str.isdigit`.
- Implemented: lambda lowering with closures, defaults, and kw-only/varargs support.
- Implemented: `sorted()` builtin with stable ordering + key/reverse (core ordering types).
- Implemented: `sorted()` enforces keyword-only `key`/`reverse` arguments (CPython parity).
- Implemented: `list.sort` with key/reverse and rich-compare fallback for user-defined types.
- Implemented: `str.lower`/`str.upper`, `list.clear`/`list.copy`/`list.reverse`, and `dict.setdefault`/`dict.update`.
- Implemented: container dunder/membership fallbacks (`__contains__`/`__iter__`/`__getitem__`) and builtin class method access for list/dict/str/bytes/bytearray.
- Implemented: dynamic call binding for bound methods/descriptors with builtin defaults + expanded class decorator parity coverage.
- Implemented: print keyword-argument parity tests (`sep`, `end`, `file`, `flush`) for native + wasm.
- Implemented: compiled `sys.argv` initialization for native + wasm harness; TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial): filesystem-encoding + surrogateescape decoding parity.
- Implemented: `sys.executable` override via `MOLT_SYS_EXECUTABLE` (diff harness pins it to the host Python to avoid recursive `-c` subprocess spawns).
- TODO(introspection, owner:runtime, milestone:TC2, priority:P2, status:partial): complete code object parity for closure/generator/coroutine metadata (`co_freevars`/`co_cellvars` values and full `co_flags` bitmask semantics).
- TODO(introspection, owner:frontend, milestone:TC2, priority:P2, status:partial): implement `globals`/`locals`/`vars`/`dir` builtins with correct scope semantics + callable parity.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): importlib.machinery pending parity (package/module shaping + file reads + restricted exec are intrinsic-lowered; remaining loader/finder parity is namespace/extension/zip behavior).
- Implemented: iterator/view helper types now map to concrete builtin classes so `collections.abc` imports and registers without fallback/guards.
- TODO(stdlib-compat, owner:runtime, milestone:TC1, priority:P2, status:partial): bootstrap `sys.stdout` so print(file=None) always honors the sys stream.
- TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:missing): expose file handle `flush()` and wire wasm parity for file flushing.
- TODO(tests, owner:frontend, milestone:TC2, priority:P2, status:planned): KW_NAMES error-path coverage (duplicate keywords, positional-only violations) in differential tests.
- TODO(tests, owner:runtime, milestone:TC2, priority:P2, status:planned): security-focused attribute access tests (descriptor exceptions, `__getattr__` recursion traps).
- Implemented: async comprehensions (async for/await) with nested + await-in-comprehension coverage.
- TODO(type-coverage, owner:runtime, milestone:TC2, priority:P2, status:partial): matmul dunder hooks (`__matmul__`/`__rmatmul__`) with buffer2d fast path.
- Partial: wasm generator state machines + closure slot intrinsics + channel send/recv intrinsics + async pending/block_on parity landed; remaining scheduler semantics (TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): wasm scheduler semantics).
- Implemented: wasm async state dispatch uses encoded resume targets to avoid state-id collisions and keeps state/poll locals distinct (prevents pending-state corruption on resume).
- Implemented: async iterator protocol (`__aiter__`/`__anext__`) with `aiter`/`anext` lowering and `async for` support; sync-iter fallback remains for now.
- Implemented: `anext(..., default)` awaitable creation outside `await`.
- Implemented: `async with` lowering for `__aenter__`/`__aexit__`.
- Implemented: cancellation token plumbing with request-default inheritance and task override; automatic cancellation injection into awaits still pending (TODO(async-runtime, owner:runtime, milestone:RT2, priority:P1, status:partial): cancellation injection on await).
- TODO(async-runtime, owner:runtime, milestone:RT2, priority:P2, status:planned): native-only tokio host adapter for compiled async tasks with determinism guard + capability gating (no WASM impact).
- TODO(syntax, owner:frontend, milestone:M3, priority:P2, status:missing): structural pattern matching (`match`/`case`) lowering and semantics (see [docs/spec/areas/compat/0021_SYNTACTIC_FEATURES_MATRIX.md](docs/spec/areas/compat/0021_SYNTACTIC_FEATURES_MATRIX.md)).
- TODO(opcode-matrix, owner:frontend, milestone:M3, priority:P2, status:missing): `MATCH_*` opcode coverage for pattern matching (see [docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md](docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md)).
- TODO(syntax, owner:frontend, milestone:M2, priority:P2, status:partial): f-string format specifiers and debug spec (`f"{x:.2f}"`, `f"{x=}"`) parity (see [docs/spec/areas/compat/0021_SYNTACTIC_FEATURES_MATRIX.md](docs/spec/areas/compat/0021_SYNTACTIC_FEATURES_MATRIX.md)).
- TODO(syntax, owner:frontend, milestone:M3, priority:P3, status:missing): type alias statement (`type X = ...`) and generic class syntax (`class C[T]: ...`) coverage (see [docs/spec/areas/compat/0021_SYNTACTIC_FEATURES_MATRIX.md](docs/spec/areas/compat/0021_SYNTACTIC_FEATURES_MATRIX.md)).
- TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:partial): async generator opcode coverage and lowering gaps (see [docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md](docs/spec/areas/compiler/0019_BYTECODE_LOWERING_MATRIX.md)).
- TODO(compiler, owner:compiler, milestone:TC2, priority:P0, status:partial): fix async lowering/back-end verifier for `asyncio.gather` poll paths (dominance issues) and wasm stack-balance errors; async protocol parity tests currently fail.
- Implemented: generator/async poll trampolines are task-aware (generator/coroutine/asyncgen) so wasm no longer relies on arity overrides.
- TODO(perf, owner:compiler, milestone:TC2, priority:P2, status:planned): optimize wasm trampolines with bulk payload initialization and shared helpers to cut code size and call overhead.
- Implemented: cached task-trampoline eligibility on function headers to avoid per-call attribute lookups.
- Implemented: coroutine trampolines reuse the current cancellation token to avoid per-call token allocations.
- TODO(semantics, owner:runtime, milestone:TC3, priority:P2, status:missing): cycle collector implementation (see [docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md](docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md)).
- Implemented: runtime lifecycle refactor moved caches/pools/async registries into `RuntimeState`, removed lazy_static globals, and added TLS guard cleanup for user threads (see [docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md](docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md)).
- Implemented: host pointer args use raw pointer ABI; strict-provenance Miri stays green (pointer registry remains for NaN-boxed handles).
- TODO(runtime-provenance, owner:runtime, milestone:RT2, priority:P2, status:planned): bound or evict transient const-pointer registrations in the pointer registry.
- TODO(semantics, owner:runtime, milestone:TC2, priority:P3, status:divergent): formalize lazy-task divergence policy (see [docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md](docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md)).
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:missing): define and implement `libmolt` C API shim + `Py_LIMITED_API` target (see [docs/spec/areas/compat/0212_C_API_SYMBOL_MATRIX.md](docs/spec/areas/compat/0212_C_API_SYMBOL_MATRIX.md)).

## File/Open Parity Checklist (Production)
Checklist:
- `open()` signature: file/mode/buffering/encoding/errors/newline/closefd/opener + path-like + fd-based open (done; utf-8/utf-8-sig/cp1252/cp437/cp850/cp860/cp862/cp863/cp865/cp866/cp874/cp1250/cp1251/cp1253/cp1254/cp1255/cp1256/cp1257/koi8-r/koi8-u/iso8859-2/iso8859-3/iso8859-4/iso8859-5/iso8859-6/iso8859-7/iso8859-8/iso8859-10/iso8859-15/mac-roman/ascii/latin-1 only, opener error text, and wasm parity still tracked below).
- Mode parsing: validate combinations (`r/w/a/x`, `b/t`, `+`), default mode behavior, and text/binary exclusivity (done).
- Buffering: `buffering=0/1/n/-1` semantics (binary-only unbuffered, line buffering in text, default sizes, flush behavior) (partial: line buffering + unbuffered text guard in place; default size + buffering strategy pending).
- Text layer: encoding/errors/newline handling, universal newlines, and `newline=None/'\\n'/'\\r'/'\\r\\n'` parity (partial: newline handling + utf-8/utf-8-sig/cp1252/cp437/cp850/cp860/cp862/cp863/cp865/cp866/cp874/cp1250/cp1251/cp1253/cp1254/cp1255/cp1256/cp1257/koi8-r/koi8-u/iso8859-2/iso8859-3/iso8859-4/iso8859-5/iso8859-6/iso8859-7/iso8859-8/iso8859-10/iso8859-15/mac-roman/ascii/latin-1/utf-16/utf-32 decode/encode; other codecs pending; encode error handlers include namereplace+xmlcharrefreplace).
- File object API: `read`, `readinto`, `write`, `writelines`, `readline(s)`, `seek`, `tell`, `truncate`, `flush`, `close`, `fileno`, `isatty`, `readable`, `writable`, `seekable`, `name`, `mode`, `closed`, `__iter__`/`__next__` (partial: core methods/attrs implemented; Windows isatty pending).
- Context manager: `__enter__`/`__exit__` semantics, close-on-exit, exception propagation, idempotent close (done).
- Capability gating: enforce `fs.read`/`fs.write` and error surfaces per operation (done).
- Native + WASM parity: file APIs and error messages aligned across hosts (pending: open parity tests + wasm host parity coverage).
  (TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial): align file handle type names in error/AttributeError messages with CPython _io.* wrappers.)

Test plan (sign-off):
- Differential tests: `tests/differential/basic/file_open_modes.py`, `file_buffering_text.py`,
  `file_text_encoding_newline.py`, `file_iteration_context.py`, `file_seek_tell_fileno.py` (move to verified subset on parity).
- Pytest unit tests: invalid mode/buffering/encoding/newline combos, fd-based `open`, `closefd`/`opener` errors, path-like objects.
- WASM parity: harness tests for read/write/line iteration using temp files via Node/WASI host I/O.
- Security/robustness: fuzz mode strings + newline values, and validate close/idempotency + leak-free handles.
- Windows parity: newline translation + path handling coverage in CI.
- Differential suite is now split by ownership lane: core/builtin semantics in `tests/differential/basic/`, stdlib module/submodule coverage in `tests/differential/stdlib/`, and wasm-focused scaffolds in `tests/wasm_planned/` until wasm parity lands.

Sign-off criteria:
- All above tests pass on 3.12/3.13/3.14 + wasm parity runs; matrices + STATUS updated; no capability bypass.

## Stdlib
- Partial: importable `builtins` module binding supported builtins (attribute gaps tracked in the matrix).
  (TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): fill `builtins` module attribute coverage.)
- Partial: asyncio shim (`run`/`sleep` lowered to runtime with delay/result semantics; `wait`/`wait_for`/`shield` + basic `gather` supported; `set_event_loop`/`new_event_loop` stubs); loop/task APIs still pending (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): asyncio loop/task API parity).
- Implemented: asyncio TaskGroup/Runner cancellation fanout lowered through
  intrinsic batch cancellation (`molt_asyncio_cancel_pending`) plus intrinsic
  gather-based drain paths.
- Implemented: asyncio lock/condition/semaphore/barrier/queue waiter fanout +
  cancellation-removal loops now route through Rust intrinsics
  (`molt_asyncio_waiters_notify`, `molt_asyncio_waiters_notify_exception`,
  `molt_asyncio_waiters_remove`,
  `molt_asyncio_barrier_release`) to keep hot synchronization paths off
  Python-side list/deque loops.
- Implemented: asyncio future transfer and event waiter-teardown callbacks now
  lower through Rust intrinsics (`molt_asyncio_future_transfer`,
  `molt_asyncio_event_waiters_cleanup`), shrinking Python callback logic in
  `Task.__await__`/`wrap_future` and token cleanup paths.
- Implemented: asyncio TaskGroup done-callback error fanout and event-loop
  ready-queue draining now lower through Rust intrinsics
  (`molt_asyncio_taskgroup_on_task_done`, `molt_asyncio_ready_queue_drain`),
  reducing Python task-scan/callback loops in cancellation/error and
  ready-dispatch hot paths.
- Partial: shims for `warnings`, `traceback`, `types`, `inspect`, `ast`, `ctypes`, `uuid`, `urllib.parse`, `fnmatch`, `copy`, `pickle` (protocol 0 only), `pprint`, `string`, `struct`, `typing`, `sys`, `os`, `json`, `asyncio`, `shlex` (`quote`), `threading`, `weakref`, `bisect`, `heapq`, `functools`, `itertools`, `zipfile`, `zipimport`, and `collections` (capability-gated env access).
- Partial: `decimal` shim backed by Rust intrinsics (contexts/traps/flags, quantize/compare/normalize/exp/div, `as_tuple`, `str`/`repr`/float conversions) with native Rust backend when vendored `libmpdec` sources are unavailable.
  (TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete Decimal arithmetic + formatting parity (add/sub/mul/pow/sqrt/log/ln, quantize edge cases, NaN payloads).)
- Implemented: strict intrinsics registry + removal of CPython shim fallbacks in tooling/tests; JSON/MsgPack helpers now use runtime intrinsics only.
- Implemented: `tools/check_stdlib_intrinsics.py` now enforces fallback-pattern bans across all stdlib modules by default (strict all-stdlib mode); opt-down to intrinsic-backed-only scope is explicit via `--fallback-intrinsic-backed-only`.
- Implemented: `tools/check_stdlib_intrinsics.py` now enforces CPython 3.12/3.13/3.14 union coverage for both top-level stdlib names and `.py` submodule names (missing-name failures, required-package shape checks, and duplicate module/package mappings).
- Implemented: stdlib coverage stubs are synchronized by `tools/sync_stdlib_top_level_stubs.py` and `tools/sync_stdlib_submodule_stubs.py` against the generated baseline in `tools/stdlib_module_union.py` (`tools/gen_stdlib_module_union.py`).
- Implemented: probe-only and python-only buckets are currently zero; union coverage is complete by name (`320` top-level names, `743` submodule names), with remaining work concentrated in intrinsic-partial burn-down.
- Implemented: non-CPython stdlib top-level extras are now constrained to `_intrinsics` and `test` only.
- Implemented: Molt-specific DB client shim moved from stdlib (`molt_db`) to `moltlib.molt_db`, with `molt.molt_db` compatibility shim retained.
- Implemented: `ast.parse` / `ast.walk` / `ast.get_docstring` now route through Rust intrinsics (`molt_ast_parse`, `molt_ast_walk`, `molt_ast_get_docstring`) with Python wrappers reduced to constructor wiring and argument forwarding.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): extend Rust ast lowering to additional stmt/expr variants and full argument shape parity; unsupported nodes currently raise RuntimeError immediately.
- Implemented: `os` fd I/O lowering for compiled binaries (`molt_os_pipe`, `molt_os_read`, `molt_os_write`) with differential coverage (`os_pipe_basic.py`, `os_read_write_basic.py`, `os_read_write_errors.py`) in intrinsic-only runs.
- Implemented: threading stdlib parity lane is green (`tests/differential/stdlib/threading_*.py` -> `24/24` pass) under intrinsic-only compiled runs with RSS profiling enabled.
- Implemented: importlib namespace/distribution path discovery now lowers through runtime intrinsics (`molt_importlib_namespace_paths`, `molt_importlib_metadata_dist_paths`) and `importlib.metadata` file reads now lower via `molt_importlib_read_file` (no Python-side dist-info scan/open fallback).
- Implemented: `importlib.resources` traversable stat/listdir shaping now lowers through runtime payload intrinsic (`molt_importlib_resources_path_payload`), and resources open/read helpers now use intrinsic-backed reads (`molt_importlib_read_file`) without Python file-open fallback.
- Implemented: `importlib.resources` loader-reader `resource_path` now enforces filesystem-only results across direct/traversable/roots fallback lanes; archive-member paths are filtered to `None` and continue through intrinsic byte-open flows.
- Implemented: `importlib.metadata` header + entry-point parsing now lowers through runtime payload intrinsic (`molt_importlib_metadata_payload`), leaving wrappers as cache/object shapers.
- Implemented: `importlib.util.find_spec` now uses a runtime payload intrinsic (`molt_importlib_find_spec_payload`) for builtin/source spec shaping + bootstrap search-path resolution; Python wrappers no longer run a separate filesystem probe path.
- Implemented: `importlib.import_module` now falls back to the intrinsic-backed spec/loader flow (`find_spec` + `module_from_spec` + loader `exec_module`) when direct runtime import returns a non-module payload, preserving dynamic `sys.path` package imports without host-Python fallback.
- Implemented: `importlib.resources.files` package root/namespace resolution now lowers through runtime payload intrinsic (`molt_importlib_resources_package_payload`) rather than Python namespace scanning.
- Implemented: `importlib.resources` loader-reader discovery now falls back from `module.__spec__.loader` to `module.__loader__` inside runtime intrinsic `molt_importlib_resources_loader_reader`, keeping custom reader lookup fully runtime-owned.
- Implemented: `importlib.machinery.SourceFileLoader.exec_module` now sources decoded module text through runtime payload intrinsic (`molt_importlib_source_exec_payload`) before intrinsic restricted execution (`molt_importlib_exec_restricted_source`), removing Python-side source decode fallback logic.
- Implemented: `importlib.machinery` extension/sourceless intrinsic execution now continues candidate probing after unsupported restricted-shim parser candidates, then raises deterministic `ImportError` only after all intrinsic candidates are exhausted.
- Implemented: restricted shim execution in runtime now includes `from ... import *` semantics (`__all__` validation + underscore fallback export rules), reducing extension/sourceless shim divergence without host fallback.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): close parity gaps for `ast`, `ctypes`, `urllib.parse`, and `uuid` (see stdlib matrix).
  (TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): advance partial shims to parity per matrix.)
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand zipfile/zipimport with bytecode caching + broader archive support.
- TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P3, status:partial): process model integration for `multiprocessing`/`subprocess`/`concurrent.futures` (spawn-based partial; IPC + lifecycle parity pending).
- TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:divergent): Fork/forkserver currently map to spawn semantics; implement true fork support.
- Partial: capability-gated `socket`/`select`/`selectors` backed by runtime sockets + io_poller with intrinsic-backed selector objects (`poll`/`epoll`/`kqueue`/`devpoll`) and backend selector classes; native + wasmtime host implemented. Node/WASI host bindings are wired in `run_wasm.js`; browser host supports WebSocket-backed stream sockets + io_poller readiness while UDP/listen/server sockets remain unsupported.
  (TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P0, status:partial): expand browser socket coverage (UDP/listen/server sockets) + parity tests.)
- Implemented: wasm/non-Unix socket host ABI now carries ancillary payload buffers + recvmsg `msg_flags` for `socket.sendmsg`/`socket.recvmsg`/`socket.recvmsg_into`; wasm runtime paths no longer hardcode `msg_flags=0`.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete cross-platform ancillary parity for `socket.sendmsg`/`socket.recvmsg`/`socket.recvmsg_into` (`cmsghdr`, `CMSG_*`, control message decode/encode); wasm-managed stream peer paths now transport ancillary payloads (for example `socketpair`), while unsupported non-Unix routes still return `EOPNOTSUPP` for non-empty ancillary control messages.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): `json` shim parity (Encoder/Decoder classes, JSONDecodeError details, runtime fast-path parser).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): continue `re` parser/matcher lowering into Rust intrinsics; literal/any/char-class advancement, char/range/category matching, anchor/backref/scoped-flag matcher nodes, group capture/value materialization, and replacement expansion are intrinsic-backed, while remaining lookaround variants, verbose parser edge cases, and full Unicode class/casefold parity are pending.
- Implemented: `queue` now lowers `LifoQueue` and `PriorityQueue` construction/ordering through runtime intrinsics (`molt_queue_lifo_new`, `molt_queue_priority_new`) on top of existing intrinsic-backed queue operations.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): complete queue edge-case/API parity (task accounting corners, comparator/error-path fidelity, and broader CPython coverage).
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand advanced hashlib/hmac digestmod parity tests.
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:partial): `struct` intrinsics cover `pack`/`unpack`/`calcsize` + `pack_into`/`unpack_from`/`iter_unpack` across the CPython 3.12 format table (including half-float) with C-contiguous nested-memoryview windows; remaining gaps are exact CPython diagnostic-text parity on selected edge cases.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand `time` module surface (`timegm`) + deterministic clock policy.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand locale parity beyond deterministic runtime shim semantics (`setlocale` catalog coverage, category handling, and host-locale compatibility).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement gettext translation catalog/domain parity (filesystem-backed `.mo` loading and locale/domain selection).
- TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P1, status:partial): wire local timezone + locale data for `time.localtime`/`time.strftime` on wasm hosts.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): tighten `weakref.finalize` shutdown-order parity (including `atexit` edge cases) against CPython.
- Implemented: `abc.update_abstractmethods` now uses runtime intrinsic `molt_abc_update_abstractmethods`; Python-side abstractmethod scanning logic was removed from `abc.py`.
- TODO(stdlib-compat, owner:runtime, milestone:TC1, priority:P2, status:partial): codec error handlers (surrogateescape/backslashreplace/etc) pending; blocked on surrogate-capable string representation.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): `codecs` module parity (registry/lookup + encodings package + incremental/stream codecs + error-handler registration); base encode/decode intrinsics are present.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): `pickle` protocol 1+ and broader type coverage (bytes/bytearray, memo cycles).
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): finish remaining `math` intrinsics (determinism policy); predicates, `sqrt`, `trunc`/`floor`/`ceil`, `fabs`/`copysign`, `fmod`/`modf`/`frexp`/`ldexp`, `isclose`, `prod`/`fsum`, `gcd`/`lcm`, `factorial`/`comb`/`perm`, `degrees`/`radians`, `hypot`/`dist`, `isqrt`/`nextafter`/`ulp`, `tan`/`asin`/`atan`/`atan2`, `sinh`/`cosh`/`tanh`, `asinh`/`acosh`/`atanh`, `log`/`log2`/`log10`/`log1p`, `exp`/`expm1`, `fma`/`remainder`, and `gamma`/`lgamma`/`erf`/`erfc` are now wired in Rust.
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): fill out `types` shims (TracebackType, FrameType, FunctionType, coroutine/asyncgen types, etc).
- TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): replace placeholder iterator/view types (`object`/`type`) so ABC registration doesn't need guards.
- TODO(tests, owner:runtime, milestone:SL1, priority:P1, status:partial): expand native+wasm codec parity coverage for binary/floats/large ints/tagged values + deeper container shapes.
- TODO(tests, owner:stdlib, milestone:SL1, priority:P2, status:planned): wasm parity coverage for core stdlib shims (`heapq`, `itertools`, `functools`, `bisect`, `collections`).
- Import-only allowlist expanded for `binascii`, `unittest`, `site`, `sysconfig`, `collections.abc`, `importlib`, and `importlib.util`; planned additions now cover the remaining CPython 3.12+ stdlib surface (see [docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md](docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md) Section 3.0b), including `annotationlib`, `compileall`, `configparser`, `difflib`, `dis`, `encodings`, `tokenize`, `trace`, `xmlrpc`, and `zipapp` (API parity pending; TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:planned): add import-only stubs + tests).

## Compatibility Matrix Execution Plan (Next 8 Steps)
1) Done: TC2 iterable unpacking + starred targets in assignment/for targets (tests + spec/status updates).
2) TC2: remaining StopIteration semantics (sync/async) with differential coverage (StopIteration.value propagation done).
3) TC2: builtin conversions (`bool`, `str`) with hook/error parity.
- Implemented: `str(bytes, encoding, errors)` decoding for bytes-like inputs (matches `bytes.decode` codec/handler coverage).
4) Done: TC2 async comprehensions lowering + runtime support with parity tests.
5) TC2/TC3: reflection builtins, CPython `hash` parity (`PYTHONHASHSEED`) + `format`/rounding; update tests + docs.
   Implemented: object-level `__getattribute__`/`__setattr__`/`__delattr__` builtins.
6) SL1: `functools` (`lru_cache`, `partial`, `reduce`) with compile-time lowering and deterministic cache keys; `cmp_to_key`/`total_ordering` landed.
7) SL1: `itertools` + `operator` intrinsics plus `heapq` fast paths; `bisect`/`heapq` shims landed (fast paths now wired).
8) SL1: finish `math` intrinsics beyond `log`/`log2`/`exp`/`sin`/`cos`/`acos`/`lgamma` and trig/hyperbolic (remaining: determinism policy), plus deterministic `array`/`struct` layouts with wasm/native parity tests.

## Offload / IPC
- Partial: `molt_accel` v0 scaffolding (stdio framing + client + decorator) with auto cancel-check detection, payload/response byte metrics, and shared demo payload builders; `molt_worker` stdio shell with demo handlers and compiled dispatch (`list_items`/`compute`/`offload_table`/`health`), plus optional worker pooling via `MOLT_ACCEL_POOL_SIZE`.
  (TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): finalize accel retry/backoff + non-demo handler coverage.)
- Implemented: compiled export loader + manifest validation (schema, reserved-name filtering, error mapping) with queue/timeout metrics.
- Implemented: worker tuning via `MOLT_WORKER_THREADS` and `MOLT_WORKER_MAX_QUEUE` (CLI overrides).
- TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): propagate cancellation into real DB tasks; extend compiled handlers beyond demo coverage.

## DB
- Partial: `molt-db` pool skeleton (bounded, sync), feature-gated async pool primitive, SQLite connector (native-only; wasm parity pending), and async Postgres connector with statement cache; `molt_worker` exposes `db_query`/`db_exec` for SQLite + Postgres (TODO(wasm-db-parity, owner:runtime, milestone:DB2, priority:P1, status:partial): wasm DB parity).
- Top priority: wasm parity for DB connectors before expanding DB adapters or query-builder ergonomics.
- Implemented: wasm DB client shims + parity test (`molt_db` async helper) consume response streams and surface bytes/Arrow IPC; Node/WASI host adapter forwards `db_query`/`db_exec` to `molt-worker` via `run_wasm.js`.

## Parity Cluster Plan (Next)
- 1) Async runtime core: Task/Future APIs, scheduler, contextvars, and cancellation injection into awaits/I/O. Key files: `runtime/molt-runtime/src/lib.rs`, `src/molt/stdlib/asyncio/__init__.py`, `src/molt/stdlib/contextvars.py`, [docs/spec/STATUS.md](docs/spec/STATUS.md). Outcome: asyncio loop/task parity for core patterns. Validation: new unit + differential tests; `tools/dev.py test`.
- 2) Capability-gated async I/O: sockets/SSL/selectors/time primitives with cancellation propagation. Key files: [docs/spec/areas/web/0900_HTTP_SERVER_RUNTIME.md](docs/spec/areas/web/0900_HTTP_SERVER_RUNTIME.md), [docs/spec/areas/runtime/0505_IO_ASYNC_AND_CONNECTORS.md](docs/spec/areas/runtime/0505_IO_ASYNC_AND_CONNECTORS.md), `runtime/molt-runtime/src/lib.rs`. Outcome: async I/O primitives usable by DB/HTTP stacks. Validation: I/O unit tests + fuzzed parser tests + wasm/native parity checks.
- Implemented: native host-level websocket connect hook for `molt_ws_connect` with capability gating for production socket usage.
- 3) DB semantics expansion: implement `db_exec`, transactions, typed param mapping; add multirange + array lower-bound decoding. Key files: `runtime/molt-db/src/postgres.rs`, `runtime/molt-worker/src/main.rs`, [docs/spec/areas/db/0700_MOLT_DB_LAYER_VISION.md](docs/spec/areas/db/0700_MOLT_DB_LAYER_VISION.md), [docs/spec/areas/db/0701_ASYNC_PG_POOL_AND_PROTOCOL.md](docs/spec/areas/db/0701_ASYNC_PG_POOL_AND_PROTOCOL.md), [docs/spec/areas/db/0915_MOLT_DB_IPC_CONTRACT.md](docs/spec/areas/db/0915_MOLT_DB_IPC_CONTRACT.md). Outcome: production-ready DB calls with explicit write gating and full type decoding. Validation: dockerized Postgres integration + cancellation tests.
- 4) WASM DB parity: define WIT/host calls for DB access and implement wasm connectors in molt-db. Key files: `wit/molt-runtime.wit`, `runtime/molt-runtime/src/lib.rs`, `runtime/molt-db/src/lib.rs`, [docs/spec/areas/wasm/0400_WASM_PORTABLE_ABI.md](docs/spec/areas/wasm/0400_WASM_PORTABLE_ABI.md). Outcome: wasm builds can execute DB queries behind capability gates. Validation: wasm harness tests + native/wasm result parity.
- 5) Framework-agnostic adapters: finalize `molt_db_adapter` + helper APIs for Django/Flask/FastAPI with shared payload builders. Key files: `src/molt_db_adapter/`, [docs/spec/areas/db/0702_QUERY_BUILDER_AND_DJANGO_ADAPTER.md](docs/spec/areas/db/0702_QUERY_BUILDER_AND_DJANGO_ADAPTER.md), `demo/`, `tests/`. Outcome: same IPC contract across frameworks with consistent error mapping. Validation: integration tests in sample Django/Flask/FastAPI apps.
- 6) Production hardening: propagate cancellation into compiled entrypoints/DB tasks, add pool/queue metrics, run bench harness. Key files: `runtime/molt-worker/src/main.rs`, `bench/scripts/`, [docs/spec/areas/demos/0910_REPRO_BENCH_VERTICAL_SLICE.md](docs/spec/areas/demos/0910_REPRO_BENCH_VERTICAL_SLICE.md). Outcome: stable P99/P999 and reliable cancellation/backpressure. Validation: `bench/scripts/run_stack.sh` + stored JSON results.

## Tooling
- Keep type facts + `ty` validation wired into build/lint flows and surface regressions early.
- Implemented: CLI wrappers for `run`/`test`/`diff`/`bench`/`profile`/`lint`/`doctor`/`package`/`publish`/`verify`,
  plus determinism/capability checks and vendoring materialization (publish supports local + HTTP(S) registry targets).
- Implemented: initial cross-target native builds (Cranelift target + zig link); next: cross-linker configuration,
  target capability manifests, and runtime cross-build caching (TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:partial): cross-target ergonomics).
- CLI Roadmap (plan):
  - Build cache clarity: `--cache-report` by default in `--json`, `molt clean --cache`, and cache hit/miss summaries with input fingerprints.
  - Build UX polish: stable `--out-dir` defaults (`$MOLT_HOME/build/<entry>`), explicit `--emit` artifacts, and `--emit-ir` + `--emit-json` dumps.
  - Profiles + metadata: `--profile {dev,release}` consistency across backend/runtime, and JSON metadata with toolchain hashes.
  - Config introspection: `molt config` shows merged `molt.toml`/`pyproject.toml` plus resolved build settings.
  - Cross-target ergonomics: cache-aware runtime builds, target flag presets, and capability manifest helpers.
- Implemented: Cranelift 0.128 backend tuning tranche in `runtime/molt-backend` with profile-safe defaults and explicit knobs:
  - release default `log2_min_function_alignment=4` (16-byte minimum function alignment),
  - dev default `regalloc_algorithm=single_pass` for faster local compile loops,
  - opt-in overrides via `MOLT_BACKEND_REGALLOC_ALGORITHM`, `MOLT_BACKEND_MIN_FUNCTION_ALIGNMENT_LOG2`, and `MOLT_BACKEND_LIBCALL_CALL_CONV`.
- Track complex performance work in [OPTIMIZATIONS_PLAN.md](OPTIMIZATIONS_PLAN.md) before large refactors.
- TODO(runtime-provenance, owner:runtime, milestone:RT1, priority:P2, status:planned): replace pointer-registry locks with sharded or lock-free lookups once registry load is characterized.
- TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): remove legacy `.molt/` clean-up path after MOLT_HOME/MOLT_CACHE migration is complete.
- TODO(tooling, owner:release, milestone:TL2, priority:P2, status:planned): formalize release tagging (start at `v0.0.001`, increment thousandth) and require super-bench stats for README performance summaries.

## Django Demo Path (Draft, 5-Step)
- Step 1 (Core semantics): close TC1/TC2 gaps in [docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md](docs/spec/areas/compat/0014_TYPE_COVERAGE_MATRIX.md) for Django-heavy types (dict/list/tuple/set/str, iter/len, mapping protocol, kwargs/varargs ordering per docs/spec/areas/compat/0016_ARGS_KWARGS.md, descriptor hooks, class `__getattr__`/`__setattr__`).
- Step 2 (Import/module system): package resolution + module objects, `__import__`, and a deterministic `sys.path` policy; unblock `importlib` basics.
- TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:partial): project-root build discovery (namespace packages + PYTHONPATH roots done; remaining: deterministic graph caching + `__init__` edge cases).
- Step 3 (Stdlib essentials): advance [docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md](docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md) for `functools`, `itertools`, `operator`, `collections`, `contextlib`, `inspect`, `typing`, `dataclasses`, `enum`, `re`, and `datetime` to Partial with tests.
- Step 4 (Async/runtime): production-ready asyncio loop/task APIs, contextvars, cancellation injection, and long-running workload hardening.
- Step 5 (I/O + web/DB): capability-gated `os`, `sys`, `pathlib`, `logging`, `time`, `selectors`, `socket`, `ssl`; ASGI/WSGI surface, HTTP parsing, and DB client + pooling/transactions (start sqlite3 + minimal async driver), plus deterministic template rendering.
- TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): close remaining `pathlib` parity gaps (glob edge cases, hidden/root_dir semantics, symlink nuances, and broader PurePath/PurePosixPath API surface) after intrinsic splitroot-aware `isabs`/`parts`/`parents` parity work.
- Cross-framework note: DB IPC payloads and adapters must remain framework-agnostic to support Django/Flask/FastAPI.
