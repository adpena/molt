<!-- Integrated parallel build program — architect-swarm synthesis (wf_18b24759-006, 2026-06-04) -->

# molt Compiler Foundation — Integrated Build Program

## Status Legend

- **DONE** — landed on main, verified
- **HELD** — implementation complete, driver wiring pending
- **OUTSTANDING** — not yet started

---

## 1. Dependency DAG

```
TIER 0 (substrate)
  S1 AnalysisManager          DONE  ef284d182
  S2 TargetInfo/cost-model    DONE  9ff5d2e00
  S3 effects oracle            DONE  8b6b88286
  S4 call-graph + module-phase DONE  7915b29a0
  S5-ph1 alias analysis        DONE  fb574b289
  S6 SCEV + ValueRange         DONE  cd66f365e

TIER 1 (correctness)
  C1 BCE natural loops         DONE  850077e7f
  C2 needs_exception_stack     DONE  430e09793
  C3 async _poll panic         DONE  29cd7765b

TIER 2 (engine)
  E1-a+b  inliner core        DONE  f14b196ce (hardened 951938075)
  E1-c    obs-only EH inlining DONE  6d9962a98
  E1-d    cost/fixed-pt        OUTSTANDING  (unblocked)
  E1-e    ACTIVATION           OUTSTANDING  (driver wiring, held patch exists)
    └─ requires: E1-a/b/c (DONE)
  E2/S5-ph2..5  MemorySSA + SROA  OUTSTANDING
    └─ requires: S5-ph1 (DONE), S1 (DONE)
  E3  IP escape+purity summaries  OUTSTANDING
    └─ requires: S4 (DONE)
  E4  IPSCCP                   OUTSTANDING
    └─ requires: S4, E1-e (activation needed for IPO context)
  E5  monomorphization         OUTSTANDING
    └─ requires: S4, E1-e (clone+run infrastructure)
  E6  MemGVN+store-fwd         OUTSTANDING (part of S5-ph2..5 arc)

TIER 3 (consequences)
  D1  generator fusion / CoroElide  OUTSTANDING
    └─ requires: E1-e (active inliner), E2 (SROA for frame slot promotion)
  DX  build-speed (crate split, LTO, generated.rs split)  OUTSTANDING
    └─ independent of correctness; phase 1a can land immediately

TIER 4 (loops/SIMD)
  L4-ph1  gate fix (loop_unroll/block_versioning/type_guard_hoist)  OUTSTANDING
    └─ requires: nothing (pure conservative narrowing, zero deps)
  L4-ph2  range_devirt ordering fix                                  OUTSTANDING
    └─ requires: L4-ph1
  L4-ph3  IV strength reduction                                       OUTSTANDING
    └─ requires: L4-ph2, S6 (DONE)
  L2  real SIMD codegen                                               OUTSTANDING
    └─ requires: S2 (DONE), S5-ph1 (DONE), S6 (DONE), L4-ph2 (range_devirt)
  L1  IV canonical + FloorDiv/Mod SR                                  OUTSTANDING
    └─ requires: L4-ph3

TIER 5 (whole-program feedback)
  W1  PGO core data flow        OUTSTANDING  (independent, can parallel engine)
    └─ requires: S2 (DONE), S4 (DONE), E1-e (for hot-callee budget wiring)
  W3  per-attribute DCE         OUTSTANDING  (independent of engine)
    └─ requires: nothing (SimpleIR layer only)

TIER 6 (verification)
  V1  translation-validation     OUTSTANDING
    └─ gates L2/E5/D1 risky lowering arcs
```

### E1-e DORMANCY ROOT CAUSE (the keystone blocking multiple arcs)

`CheckException` sets `has_exception_handling = true` (lower_from_simple.rs:319-330) which causes `is_inlineable` to refuse every real function. The correct gate is `has_exception_handlers()` (function.rs:153), which E1-c already landed for the observation-only inlining path. E1-e activation and L4 gate fixes are the two immediate structural unlocks.

---

## 2. Parallel Track Assignment

### Build Cap Constraint
- Max 2 concurrent build-triggering agents (CLAUDE.md). Production-codegen changes (E1-e, backend paths) must be serially validated — one at a time through the daemon socket.
- 3 tracks run simultaneously; the third slot is held for the critical path (E1-e) when it conflicts with active work.

### Track A — Critical Path: E1-e Activation + E3 Summaries (serial, highest leverage)
Files: `simple_backend.rs`, `wasm.rs`, `passes.rs`, `ip_summary.rs`, `escape_analysis.rs`

Phase sequence:
1. **E1-e1**: native Cranelift path restructure (parallel TIR loop → TirModule → run_module_pipeline → back-convert → delete compute_leaf_functions_via_call_graph)
2. **E1-e2**: WASM path restructure (mirror native)
3. **E1-e3**: LLVM activation (apply held patch at `memory/phase_e_e1_llvm_driver_wiring.patch`, strip diagnostics)
4. **E1-e4**: delete `passes::inline_functions` + `is_inlineable_with_limit` (the dual path)

Steps e1–e4 are ONE ATOMIC ARC (CLAUDE.md: no dual path in intermediate commits unless each is independently complete and batoned).

5. **E3-A**: extend `FunctionSummary` with `does_not_capture_param` + `is_pure` + `return_repr`; populate bottom-up in `ModuleSummaries::compute`
6. **E3-B**: bottom-up callee-summary propagation (purity through call chains)
7. **E3-C**: wire `Option<Arc<ModuleSummaries>>` into `escape_analysis::analyze`; replace `OpCode::Call → GlobalEscape` unconditional with summary-gated `ArgEscape`
8. **E3-D**: migrate `compute_return_alias_summaries` from SimpleIR to TIR-native in `ip_summary.rs`

Track A owns: `tir/module_phase.rs`, `native_backend/simple_backend.rs`, `wasm.rs`, `passes.rs`, `tir/passes/ip_summary.rs`, `tir/passes/escape_analysis.rs`

### Track B — Memory Foundation: S5-ph2..5 MemorySSA + SROA (serially validated, no prod-path conflict during construction)
Files: `tir/passes/memory_ssa.rs` (new), `tir/passes/mem_gvn.rs` (new), `tir/passes/sroa.rs` (new), `tir/passes/dead_store_elim.rs`, `tir/passes/licm.rs`, `tir/analysis/mod.rs`, `tir/pass_manager.rs`

Phase sequence:
1. **S5-2a**: `MemorySsaResult` types + `compute_standalone` + 7 unit tests — standalone, no consumers, no behavior change
2. **S5-2b**: `MemGVN` pass (store-to-load forwarding + redundant-load elim), insert into pipeline after `dead_store_elim`
3. **S5-2c**: cross-block DSE via MemorySSA in `dead_store_elim::run`
4. **S5-2d**: SROA pass (field promotion on NoEscape objects)
5. **S5-2e**: LICM-of-loads (extend `licm::is_hoistable` with MemorySSA gate)

Track B is independent of Track A during construction phases S5-2a/2b/2c (no overlapping files). S5-2d and S5-2e interact with the pipeline `pass_manager.rs` which Track A also touches — coordinate merge order.

Track B also needs `AnalysisId::MemorySSA` added to `analysis/mod.rs`. This file is NOT modified by Track A.

### Track C — Loop Gates + DX + W3 (independent, no backend conflicts)

**Sub-track C1 — L4 gate fixes (zero build conflicts with A or B)**:
1. **L4-ph1**: add `TerminatorOnlyPredMap` to `analysis/mod.rs`; change `func.has_exception_handling` to `func.has_exception_handlers()` in `loop_unroll.rs:250`, `block_versioning.rs:378`, `type_guard_hoist.rs:90`; switch those two passes to `TerminatorOnlyPredMap`; delete stale comments. One atomic commit.
2. **L4-ph2**: diagnose and fix `range_devirt` pipeline ordering (swap `iter_devirt` before `range_devirt` in `pass_manager.rs:290-293`)
3. **L4-ph3**: new `tir/passes/iv_strength_reduction.rs` pass

**Sub-track C2 — W3 per-attribute DCE (SimpleIR only, zero TIR conflicts)**:
1. `collect_module_attr_write_map` in `passes.rs` (no behavior change)
2. Two-phase BFS augmentation in `eliminate_dead_functions`; mirror in `cli.py:_reachable_function_names_for_stdlib_cache`; bump schema version

**Sub-track C3 — DX build speed (config + structural)**:
1. `Cargo.toml`: `lto = "thin"` in `release-fast`, add `release-output` with fat LTO
2. `cli.py`: shared `.sccache/` directory
3. `function_compiler.rs` module split (8 sub-modules in `native_backend/`)

Sub-track C3 phase 1 (LTO + sccache) has zero source file conflicts. The `function_compiler.rs` split conflicts with Track A's `simple_backend.rs` work only if both edit the same lines; schedule the fc split AFTER E1-e lands on Track A.

---

## 3. Critical Path

The longest dependency chain:

```
E1-e1 → E1-e2 → E1-e3 → E1-e4   (E1 activation, ~3 days)
  → E3-A → E3-B → E3-C → E3-D    (IP summaries, ~2 days)
    → E5-A → E5-B → E5-C          (monomorphization, ~5 days)
      → D1-A → D1-B → D1-C        (generator fusion, ~5 days)
        → D1-E (os.walk)           (os.walk as Python, ~2 days)
```

Total critical path: ~17 days serial. Shortening it:

1. **E1-e** is the first unmovable gate. Apply the held patch (`memory/phase_e_e1_llvm_driver_wiring.patch`) for the LLVM leg; native + WASM legs require restructuring the parallel TIR loops. This is 2–3 days of focused work on Track A. Nothing on the critical path moves until this lands.

2. **E3-C** (IP escape threading) adds `Option<Arc<ModuleSummaries>>` to `escape_analysis::analyze`. This is the site where SROA (Track B) and the generator fusion (D1) both benefit — landing it early on Track A lets Track B's SROA fire on cross-call patterns immediately.

3. **S5-ph2 (MemorySSA)** on Track B is independent of E1-e and can land in parallel. It does not shorten the critical path but is the prerequisite for SROA (E2) which D1 (generator fusion) needs for frame slot promotion. Running Track B at full speed during Track A's E1-e work closes that gap.

4. **L4-ph1** (gate fixes) is 4 hours of work with zero conflicts. It immediately unlocks loop unrolling and type guard hoisting on the vast majority of user functions. Do this first, before anything else, on Track C.

---

## 4. Landing Order Per Track

### Track A Landing Order

| Step | Arc | Files | Perf Gate |
|------|-----|-------|-----------|
| A1 | E1-e1..e4 (atomic) | `simple_backend.rs`, `wasm.rs`, `passes.rs` | bench_sum ≥ CPython, no regression on 882 tests |
| A2 | E3-A (summaries schema) | `ip_summary.rs`, `target_info.rs` | no behavior change, unit tests |
| A3 | E3-B (bottom-up chain) | `ip_summary.rs` | purity propagates through call chains |
| A4 | E3-C (escape threading) | `escape_analysis.rs`, `pass_manager.rs` | `stack_alloc_across_call.py` differential passes |
| A5 | E3-D (return alias migration) | `ip_summary.rs`, `passes.rs` | byte-identical codegen all 3 backends |

All of A1 is one atomic commit. A2–A5 are each independently completable.

### Track B Landing Order

| Step | Arc | Files | Perf Gate |
|------|-----|-------|-----------|
| B1 | S5-2a MemorySSA standalone | `memory_ssa.rs` (new), `analysis/mod.rs` | 7 unit tests pass, no behavior change |
| B2 | S5-2b MemGVN forwarding | `mem_gvn.rs` (new), `pass_manager.rs` | struct_field_forwarding.py differential |
| B3 | S5-2c cross-block DSE | `dead_store_elim.rs` | struct_cross_block_dse.py |
| B4 | S5-2d SROA | `sroa.rs` (new), `pass_manager.rs` | bench_struct ≥ 1.0× CPython native |
| B5 | S5-2e LICM-of-loads | `licm.rs` | struct_loop_licm.py, bench_field_licm ≥ CPython |

B1 can land concurrently with A1 (no file overlap). B2 requires `AnalysisId::MemorySSA` from B1. B4 is the bench_struct proving ground — it is the primary perf gate for Track B.

### Track C Landing Order

| Step | Arc | Files | Perf Gate |
|------|-----|-------|-----------|
| C1 | L4-ph1 gate fixes | `loop_unroll.rs`, `block_versioning.rs`, `type_guard_hoist.rs`, `analysis/mod.rs` | counted_loop unrolls, TIR_OPT_STATS shows fires |
| C2 | L4-ph2 range_devirt ordering | `pass_manager.rs` | range_devirt: 1 values_changed confirmed |
| C3 | L4-ph3 IV strength reduction | `iv_strength_reduction.rs` (new), `pass_manager.rs` | stride_sum improvement ≥ 10% |
| C4 | W3-ph1 attr-write-map | `passes.rs` | no behavior change |
| C5 | W3-ph2 two-phase BFS | `passes.rs`, `cli.py` | empty.py ≤ 3.5 MB |
| C6 | DX-ph1a LTO split | `Cargo.toml` | clean build time drops ≥ 30s |
| C7 | DX-ph1b sccache shared | `cli.py` | cross-agent hit rate ≥ 80% |
| C8 | DX-ph2 fc split | `native_backend/fc_*.rs` (8 new) | `fc_arith.rs` edit incremental ≤ 30s |

C1 MUST be first (the highest ROI per hour on Track C). C6/C7 can land any time — zero behavior risk. C8 schedules after A1 (E1-e) to avoid merge conflicts in `simple_backend.rs`.

---

## 5. Highest-Leverage Next 3 Arcs

### Arc 1: L4-ph1 — Loop Gate Fixes (implement immediately, Track C)

**Rationale**: 4 hours of work. Three passes (loop_unroll, block_versioning, type_guard_hoist) produce zero work on virtually all real user functions today because `has_exception_handling = true` from `CheckException` (C2 universal observation). The fix is 3 line changes + 1 new analysis registration + deletion of 3 stale comments. No new logic, just un-blocking existing logic. Zero risk of miscompile — pure conservative narrowing.

**Independence**: No file conflicts with Track A or B. Can proceed immediately while E1-e is being designed/implemented.

**Leverage**: Immediately enables loop unrolling for counted loops, TypeGuard hoisting for polymorphic loops, and block versioning for type-specialized paths. These feed into SCCP folding and the SIMD pre-requisite (range_devirt canonicalization).

Specific changes:
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/tir/analysis/mod.rs`: add `TerminatorOnlyPredMap` variant to `AnalysisId`, add to `ALL`, implement `Analysis` trait with `CfgEdgePolicy::TerminatorOnly`
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/tir/passes/loop_unroll.rs:250`: `func.has_exception_handling` → `func.has_exception_handlers()`
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/tir/passes/block_versioning.rs:378`: same gate; `line 382`: `PredMap` → `TerminatorOnlyPredMap`; delete stale comment lines 373-377
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/tir/passes/type_guard_hoist.rs:90`: same gate; `line 100`: `PredMap` → `TerminatorOnlyPredMap`; delete stale comment lines 94-98
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/tir/pass_manager.rs`: add `TerminatorOnlyPredMap` to `assert_analyses_fresh` macro body

### Arc 2: E1-e1..e4 — Inliner Activation (Track A, critical path gate)

**Rationale**: The inliner has been dormant since `f14b196ce`. Every downstream IPO arc (E3, E4, E5, D1) and most of the benchmarked perf wins depend on the inliner actually firing in production codegen. The held patch covers the LLVM leg. The native and WASM legs require restructuring the parallel TIR loop in `simple_backend.rs` and `wasm.rs`. This is a structural arc with no shortcut — landing it is the single most load-bearing session of work in the entire program.

**Independence**: Conflicts with DX-ph2 (fc split) — schedule DX-ph2 after this lands. No conflict with Track B or L4-ph1.

**Leverage**: Activates the entire IPO tier. Eliminates the SimpleIR inliner dual path. Makes `bench_sum` and all helper-function benchmarks measurably faster. Enables E3 (IP escape summaries) to actually reduce allocations across call boundaries.

**Key correctness check**: After landing, `MOLT_DISABLE_INLINING=1` must still work as a rollback. The `apply(f, 1<<60, 7)` bigint oracle differential test must pass byte-identical to CPython on all targets (the repr safety argument in §5.3 of the E1-e blueprint guarantees this but must be verified).

### Arc 3: W3-ph1..ph2 — Per-Attribute DCE (Track C, binary size + startup)

**Rationale**: Currently `empty.py` links 4.31 MB because `func_new` in `molt_init_builtins` / `molt_init_sys` is an unconditional DCE root. Every builtin (`filter`, `sorted`, `zip`, `enumerate`, `vars`, `dir`, etc.) links into every program regardless of use. The fix is entirely in `passes.rs:2005-2146` and `cli.py:17057` — no new IR constructs, no backend changes, no risk of miscompile (fail-closed: unrecognized patterns → old behavior).

**Independence**: Zero conflicts with any other track. Operates on `FunctionIR`/`OpIR` structs before TIR lifting. Can proceed concurrently with A1 and B1.

**Leverage**: Expected 650 KB–1.1 MB binary size reduction. Directly attacks the `<2 MB` target. Also reduces cold-start by reducing code page faults on first use. The W3 blueprint's two-phase BFS (attr-write collection + attr-read-gated BFS) is the correct structural fix — not a per-module allowlist, not a manual annotation scheme.

---

## 6. Cross-Cutting Risks

### Risk 1: E1 Dormancy Cascades into D1 and E5

The `CheckException`-causes-`has_exception_handling` finding means the inliner phases a+b+c are dormant on real code. E1-e activation is the fix for the production path. But E5 (monomorphization) clones functions and runs the pipeline on clones — if `is_inlineable` refuses all clones due to the same `CheckException` flag, monomorphization's inlined specializations will be unoptimized. The L4-ph1 gate fix uses `has_exception_handlers()` not `has_exception_handling` for loop passes; E5's `is_specializable` predicate must do the same. Add this check explicitly when implementing E5.

### Risk 2: MemorySSA + SROA + Repr Safety

When SROA promotes a field to an SSA value (replaces `LoadAttr(obj, offset)` with `Copy(stored_val)`), the forwarded value inherits the repr of the `stored_val`. If `stored_val` is `MaybeBigInt`, the forwarded value must remain `MaybeBigInt`. The `repr_by_value_for` call in `representation_plan.rs` must run fresh on the post-SROA function (after `run_pipeline`) to recompute repr on the promoted SSA values. The SROA precondition gate (no `GenericHeap` MemoryDef between store and load) prevents miscompile; the repr floor ensures no trusted-unbox regression. The `apply(f, 1<<60, 7)` oracle covers this.

### Risk 3: SimpleIR Inliner Deletion (E1-e4) — Cascading Test Failures

`passes::inline_functions` has call sites in `simple_backend.rs` (native) and `wasm.rs`. After E1-e4 deletes it, any test that exercised the SimpleIR inliner path exercises the TIR inliner instead. The behavioral difference is that the TIR inliner is MORE conservative (respects exception observation, cost model, SSA verification). Tests whose pass criteria depend on the SimpleIR inliner's aggressive-but-structurally-unsound behavior (no handler check, no SSA, no cost model) will fail. These are not regression failures — they reveal test specifications that encoded the old behavior. Fix the tests to encode correct behavior.

### Risk 4: W3 Stdlib Cache Coherence

The Python BFS (`cli.py:_reachable_function_names_for_stdlib_cache`) and the Rust DFE BFS (`passes.rs:eliminate_dead_functions`) must produce identical live sets or the cache will be stale. Both must be updated atomically in the W3 commit. A schema version bump (`_SHARED_STDLIB_CACHE_SCHEMA_VERSION`) is mandatory to force cache invalidation on the first post-W3 build. Missing this causes silent over-linking for cached builds.

### Risk 5: DX Phase 3 (Crate Extraction) + E1-e Ordering

`molt-backend-native` crate extraction moves `native_backend/simple_backend.rs` — the exact file E1-e modifies. If E1-e is in-progress when DX-ph3 is attempted, there will be a 3-way merge conflict. **Resolution**: E1-e MUST land and be pushed before DX-ph3 begins. The DX blueprint's phasing already documents this dependency.

### Risk 6: L2 SIMD — Cranelift SIMD Type Availability

`types::I64X2` and `types::F64X2` are available in Cranelift 0.131 on aarch64 (Apple Silicon via NEON) and x86_64 (SSE2). But `I64X4` (AVX2) requires querying the ISA builder for `has_avx2`. The `SimdCaps::detect_host()` path (already in `simple_backend.rs:2544`) provides the right width. L2 must not hardcode AVX2 widths — always query `tti.vector_width_i64`. Additionally, the existing 4x scalar unroll at `function_compiler.rs:30815-31000` that L2 deletes is the ONLY current "vectorization". Deleting it before L2 is fully wired would regress `bench_sum_list`. The deletion and the new SIMD emission are ONE ATOMIC ARC (phases 1b). Do not split them.

### V1 Translation-Validation Gate

Per the gap analysis (Tier 6), `function_compiler.rs` (~38K lines) has no semantic-equivalence guarantee. The riskiest lowering arcs — L2 (SIMD codegen into Cranelift), E5 (function cloning + pipeline re-run), D1 (generator frame restructuring) — each introduce new TIR→SimpleIR→native paths with no formal validation.

**The V1 gate should precede L2 and E5**: extend `tools/translation_validator.py` beyond pass-to-pass TIR checking to also validate TIR→SimpleIR round-trips. Specifically: for each function that the SIMD pass (L2) or monomorphization (E5) transforms, run the TIR→SimpleIR→back-to-TIR round-trip and assert structural equivalence (same ops modulo SSA renaming). This is not a complete Alive2-level proof, but it catches the class of bugs that have manifested in past sessions (wrong opcode mapping, missing loop metadata transfer, truncated BigInt paths).

The V1 work is ~2 days and belongs on Track C between L4-ph3 and L2. Landing L2 or E5 without V1 is accepted risk but should be documented in the commit as "pre-V1 — requires differential test matrix to substitute."

---

## 7. Module-Level Parallel Execution Map (Concurrent Agent Assignment)

Given the 2-build-agent cap:

**When E1-e is the active Track A build**:
- Agent 1: Track A (E1-e1..e4 — requires daemon rebuild)
- Agent 2: Track B (S5-2a MemorySSA construction — does NOT require daemon, only `cargo test`)
- Track C manual work: L4-ph1 gate fix (4 hours, no build trigger needed to write, single build to verify)

**When E1-e has landed and Track A moves to E3**:
- Agent 1: Track A (E3-A/B/C — incremental builds)
- Agent 2: Track B (S5-2b..d MemGVN+SROA — each requires a build)
- Track C: W3-ph1 (no build conflict during construction, one build to verify)

**When S5 is on S5-2d (SROA)**:
- This is the bench_struct proving ground. Reserve the third build slot for perf measurement.

**When both E1-e and S5 are landed**:
- Agent 1: Track A (E5 monomorphization — pure construction with periodic test-builds)
- Agent 2: Track C (L2 SIMD — new file construction, periodic test-builds)
- DX-ph2 (fc split): can now proceed on a third agent since E1-e is stable

---

## 8. File-Level Conflict Table

| File | Track A | Track B | Track C |
|------|---------|---------|---------|
| `tir/analysis/mod.rs` | — | B1 (MemorySSA variant) | C1 (TerminatorOnlyPredMap) |
| `tir/pass_manager.rs` | — | B2 (insert mem_gvn) | C1 (check! macro), C2 (ordering) |
| `tir/module_phase.rs` | A1 (E5/E3 call), later E5 | — | — |
| `tir/passes/escape_analysis.rs` | A4 (E3-C) | — | — |
| `tir/passes/ip_summary.rs` | A2/A3/A5 (E3) | — | — |
| `native_backend/simple_backend.rs` | A1 (E1-e) | — | C8 (fc split, AFTER A1) |
| `wasm.rs` | A1 (E1-e) | — | — |
| `passes.rs` | A5 (delete return_alias legacy) | — | C4/C5 (W3) |
| `tir/passes/loop_unroll.rs` | — | — | C1 |
| `tir/passes/block_versioning.rs` | — | — | C1 |
| `tir/passes/type_guard_hoist.rs` | — | — | C1 |
| `tir/passes/licm.rs` | — | B5 (MemorySSA gate) | — |
| `tir/passes/dead_store_elim.rs` | — | B3 | — |
| `Cargo.toml` | — | — | C6 (LTO) |
| `cli.py` | — | — | C5 (W3), C7 (sccache) |

Conflicts exist at `tir/analysis/mod.rs` (both B1 and C1 add to `AnalysisId`) and `tir/pass_manager.rs` (both B2 and C2 modify the pipeline). **Resolution**: land C1 (L4 gate fix, 4 hours) before B1 starts, so B1's `analysis/mod.rs` edit includes the already-landed `TerminatorOnlyPredMap` variant. For `pass_manager.rs`, C2 (ordering fix) is a 1-line swap; land it in the same commit as C1. B2 then edits a clean baseline.

---

## 9. Build Sequence — Complete Phased Checklist

### Immediate (before any session build starts)

- [ ] **C1a** — Add `TerminatorOnlyPredMap` to `AnalysisId` in `/Users/adpena/Projects/molt/runtime/molt-backend/src/tir/analysis/mod.rs`
- [ ] **C1b** — Change `func.has_exception_handling` → `func.has_exception_handlers()` in `loop_unroll.rs:250`, `block_versioning.rs:378`, `type_guard_hoist.rs:90`
- [ ] **C1c** — Switch `block_versioning.rs:382` and `type_guard_hoist.rs:100` from `PredMap` to `TerminatorOnlyPredMap`; delete stale comments at `block_versioning.rs:373-377` and `type_guard_hoist.rs:94-98`
- [ ] **C1d** — Add `TerminatorOnlyPredMap` arm to `assert_analyses_fresh` macro in `pass_manager.rs`
- [ ] **C2** — Swap `iter_devirt` before `range_devirt` in `pass_manager.rs:290-293`
- [ ] Verify: `cargo test -p molt-backend --features native-backend` ≥ 882 tests, `TIR_OPT_STATS=1` on `sum_range(4)` shows `loop_unroll: ops_added > 0`
- [ ] Commit atomically: `"L4: re-enable loop_unroll/block_versioning/type_guard_hoist on CheckException-bearing functions + range_devirt ordering fix"`

### Phase 1: E1 Activation (Track A, 1 build slot)

- [ ] **A1a** — Add `lower_functions_to_tir_module` to `/Users/adpena/Projects/molt/runtime/molt-backend/src/tir/lower_from_simple.rs`
- [ ] **A1b** — Restructure native parallel TIR loop in `simple_backend.rs` to produce `TirModule` → `run_module_pipeline` → back-convert → `module_analysis.leaf_functions()`
- [ ] **A1c** — Delete `compute_leaf_functions_via_call_graph` from `simple_backend.rs`; delete `inline_functions(...)` call at line ~2625
- [ ] **A1d** — Mirror WASM path in `wasm.rs`: per-function TIR → TirModule → `run_module_pipeline` → back-convert; move `prepare_lir_wasm_fast_output` to post-module-phase; delete `crate::inline_functions(...)` at `wasm.rs:2159-2162`
- [ ] **A1e** — Apply held patch `memory/phase_e_e1_llvm_driver_wiring.patch` to LLVM branch in `simple_backend.rs`; remove `TEMP DIAGNOSTIC` block
- [ ] **A1f** — Delete `passes::inline_functions` and `passes::is_inlineable_with_limit` from `passes.rs`
- [ ] Verify: 882+ tests, `apply(mul, 1<<60, 7)` bigint oracle correct, `bench_sum` ≥ CPython
- [ ] Commit atomic E1-e1..e4: `"E1-e: activate TIR inliner in all 3 backends, retire SimpleIR inliner dual path"`

### Phase 2: MemorySSA Foundation (Track B, concurrent with A1 refine)

- [ ] **B1** — Create `/Users/adpena/Projects/molt/runtime/molt-backend/src/tir/passes/memory_ssa.rs` with `MemVersion`, `MemAccess`, `MemorySsaResult`, `compute_standalone`; add `AnalysisId::MemorySSA` (10 → 11); 7 unit tests; no behavior change
- [ ] **B2** — Create `mem_gvn.rs`; add `"mem_gvn"` pass after `"dead_store_elim"` in pipeline; `struct_field_forwarding.py` differential passes
- [ ] **B3** — Extend `dead_store_elim::run` with `run_cross_block_dse`; `struct_cross_block_dse.py` passes
- [ ] **B4** — Create `sroa.rs`; add `"sroa"` pass after `"mem_gvn"`; `bench_struct ≥ 1.0× CPython native`
- [ ] **B5** — Extend `licm::run` with `am.get::<MemorySSA>` gate in `is_hoistable_with_mem`; `struct_loop_licm.py` passes

### Phase 3: IP Summaries (Track A, after E1-e landed)

- [ ] **A2** — Extend `FunctionSummary` with `does_not_capture_param`, `is_pure`, `return_repr`; populate in `ModuleSummaries::compute`
- [ ] **A3** — Bottom-up callee-summary purity propagation
- [ ] **A4** — Add `summaries: Option<&ModuleSummaries>` to `escape_analysis::analyze`; replace `OpCode::Call → GlobalEscape` unconditional arm; wire through `PassManager.module_summaries`; differential `stack_alloc_across_call.py`
- [ ] **A5** — Migrate `compute_return_alias_summaries` to TIR-native in `ip_summary.rs`

### Phase 4: Binary Size + DX (Track C, concurrent with A2-A5)

- [ ] **C4-C5** — W3 per-attribute DCE: `collect_module_attr_write_map` + two-phase BFS in `passes.rs`; mirror in `cli.py`; bump schema version; empty.py ≤ 3.5 MB
- [ ] **C6-C7** — `Cargo.toml` LTO split; shared `.sccache/` dir; verify `bench_fib` does not regress below CPython (thin LTO risk point)
- [ ] **C8** — `function_compiler.rs` split into 8 sub-modules (after E1-e is stable on main); `fc_arith.rs` incremental rebuild ≤ 30s

### Phase 5: Monomorphization (Track A, after E3 landed)

- [ ] **E5-A** — Create `tir/passes/specializer.rs`; add `specialization_budget = 0` field to `TargetInfo`; `run_specializer` call in `module_phase.rs` gated behind `is_specialization_enabled()`; all tests pass, zero behavior change
- [ ] **E5-B** — Implement `compute_call_site_repr_key`; activate `specialization_budget = 4` on `native_release_fast`; `specialize_bigint_boundary.py` differential must pass
- [ ] **E5-C** — WASM + LLVM activation with domain-appropriate budgets

### Phase 6: Real SIMD + Generator Fusion (after V1 gate + E1-e + S5-2d)

- [ ] **V1** — Extend `tools/translation_validator.py` to validate TIR→SimpleIR structural equivalence for transformed functions; integrate as `--verify-lowering` flag
- [ ] **L2-ph1a** — Add 9 SIMD opcodes to `ops.rs`; classify in `effects.rs`, `alias_analysis.rs`
- [ ] **L2-ph1b** — Create `vectorize_lower.rs`; delete 4x scalar unroll from `function_compiler.rs:30815-31000` (atomic with new SIMD emission); `bench_sum_list ≥ CPython`
- [ ] **D1-A** — Extract `clone_function_body_with_fresh_ids` from `inliner.rs` to shared location; create `generator_fusion.rs`; wire into `module_phase.rs` after E1; `gen_simple_for.py` passes
- [ ] **D1-B** — Extend to observation-only CheckException poll functions
- [ ] **D1-C** — Multi-yield generators (while + yield)
- [ ] **D1-D** — Lazy scandir primitive
- [ ] **D1-E** — os.walk as Python generator; delete `molt_os_walk` intrinsic AFTER perf gate passes

### Phase 7: PGO (Track C, largely independent)

- [ ] **W1-a** — Extend `PgoProfileIR` with `call_counts`, `loop_counts`; `ProfileData::from_pgo_ir` constructor; bridge `ir.profile → TargetInfo.with_profile_data` in `simple_backend.rs` + `wasm.rs`; delete dual `pgo_hot: BTreeSet` from `passes.rs:303-320`; add `pgo_annotate` ReadOnly no-op pass
- [ ] **W1-b** — `block_order_hint` in `TirFunction`; Cranelift hot-block ordering
- [ ] **W1-c** — LLVM `!prof` metadata via `llvm-sys` unsafe call at `lowering.rs:5144`
- [ ] **W1-d** — Instrumented binary → `molt_pgo.profdata` counter collection

---

## 10. The Three Parallel Workstreams Right Now

**Starting immediately (before any build)**:

1. **Track C/L4-ph1+ph2** — 4-hour zero-risk unlock: loop gate fixes + range_devirt ordering. No build agent needed during writing. One build to verify. This is the single highest-leverage-per-hour move available.

2. **Track C/W3-ph1** — Write `collect_module_attr_write_map` in `passes.rs`. No behavior change, one build to verify. ~3 hours.

3. **Track C/DX-ph1a** — Edit `Cargo.toml` `release-fast` profile: `lto = "thin"`. Add `release-output` profile. ~30 minutes.

**In parallel with E1-e design (requires one dedicated build agent)**:

Track B/S5-2a — Write the MemorySSA types and 7 unit tests. Uses `cargo test` only, no daemon needed. Can run while the E1-e build occupies the daemon slot.

**After L4-ph1 and W3-ph1 land**:

Start Track A/E1-e1..e4. This is the critical path. Reserve both build slots for it.
