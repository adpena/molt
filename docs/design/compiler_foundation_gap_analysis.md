# molt Compiler Foundation — comprehensive gap analysis & build program

Status: MASTER ROADMAP (2026-06-03). Synthesizes a 7-lane parallel gap analysis (IPO,
memory/alias, type specialization, loops/value-analysis, effects/EH/coroutines, WPO/PGO/cost/
verify, SIMD/GPU/ABI/backends). Each lane mapped molt's coverage vs world-class (LLVM/MLIR/
Julia/Codon/Mojo/GCC/Polly/V8) and adversarially self-reviewed. This is the dependency-correct
build program for "Mojo/Julia but better; Codon but more flexible; Nuitka but way faster."
Builds on [[optimizer_foundation]]; supersedes the narrow generator_fusion framing.

## Where molt actually is (don't underestimate it)
Already present and non-trivial: fixpoint type inference w/ IV-seeding + oscillation detection +
guard propagation (type_refine.rs); a single-source-of-truth `Repr` lattice across native/LLVM/WASM
(representation_plan.rs, Phase 0/1 landed); Perceus reuse analysis (reuse_analysis.rs); 6-strategy
refcount elimination incl. Deutsch-Bobrow + unique-ownership (refcount_elim.rs); SBBV block
versioning (block_versioning.rs); a deopt skeleton (tir/deopt.rs + object/deopt.rs); an egg-based
e-graph PoC (egraph_simplify.rs, feature-gated); a Lean 4 formalization of ~15 passes (formal/lean/,
~73 open sorries); an MLIR backend with a real pass manager; a 26-primitive GPU stack; SCCP/GVN/LICM/
DCE/canonicalize/strength-reduction/BCE/check_exception_elim. This is advanced for an early compiler.

The gaps are the LOAD-BEARING ones — the passes/substrates that make everything else compound.

---

## TIER 0 — Foundational substrate (everything depends on these; build first)
These six are absent and are prerequisites cited repeatedly across lanes.

- **S1. Pass + Analysis Manager** (LLVM new-PM/AnalysisManager analog). Today `run_pipeline`
  (passes/mod.rs:72) is a monolithic linear per-function sequence; dominators are recomputed
  independently in gvn/licm/verify/verify_lir/bce (~5×/run, O(n²)); no analysis caching, no
  invalidation, no fixpoint scheduling, no per-function pipeline customization. Build: an analysis
  context threaded through passes with lazy compute + cache + invalidation; fixpoint outer loop.
  Unblocks: cheaper compiles + every future pass. Also fixes the redundant-dominator gap.
- **S2. Unified cost model / TargetTransformInfo**. Every profitability threshold is a magic constant
  (inline 30 ops, unroll trip≤8, vector width=2, tile=32, LICM register-pressure-blind). Build a
  shared `TargetInfo` (latencies, vector widths, cache hierarchy, branch-mispredict, call overhead)
  consulted by all passes. Unblocks: consistent, tunable, target-aware decisions.
- **S3. Unified effects/alias oracle**. The legacy three-list hazard has been structurally reduced:
  DCE/LICM consume generated opcode effect/purity facts, and GVN consumes a generated numbering-role
  table for always-numberable, type-gated, and value-keyed-constant families. Remaining S3 work is the
  richer per-op memory/effect tag model (`reads/writes-memory[region]`, allocates, exception-flag
  reads/writes) that can derive load/store and MemorySSA legality without ad-hoc pass predicates.
  Unblocks: correctness + CSE/LICM/DCE of pure ops; the substrate for S5.
- **S4. Call graph + whole-program (module) pass phase**. `run_pipeline` is strictly per-function;
  `TirModule` (function.rs:135) exists but is unused; the driver holds the full set only in SimpleIR
  (main.rs:~449, wasm.rs:~2155). Build `run_module_pipeline(&mut TirModule)` + a call graph (the DFE
  BFS at cli.py:17019 already builds one — factor it out). Unblocks: the ENTIRE IPO tier (inliner,
  IPSCCP, IP-escape, monomorphization, CHA, purity inference). The single biggest architectural unlock.
- **S5. Alias analysis + Memory SSA**. NO first-class alias analysis (4 ad-hoc barrier lists); NO
  memory versioning → no store-to-load forwarding, no redundant-load elim, LICM can't hoist loads
  (licm.rs:51 excludes LoadAttr), GVN can't dedup loads, cross-block DSE impossible (dead_store_elim.rs:50
  documents the limit). Build alias analysis (points-to + region/TBAA-style) then MemorySSA (memory phis
  via the existing dominance-frontier machinery). Unblocks: SROA on heap, redundant-load elim,
  cross-block DSE, LICM-of-loads, CoroElide. The memory-opt keystone (Julia's allocation-elision engine).
- **S6. Value-range / ScalarEvolution / LazyValueInfo**. NO closed-form recurrence (SCEV) → no
  trip-count for dynamic bounds, no IV strength reduction, no general BCE, no polyhedral; NO interval/
  range analysis (LVI) → BCE limited to range()/BuildList, the MaybeBigInt→RawI64Safe promotion can't
  prove 47-bit fit. Build a SCEV-style recurrence representation + a per-edge RangeFact lattice from
  guards+IVs. Unblocks: general BCE, IV strength reduction, loop transforms, Repr Phase 2 promotion.

---

## TIER 1 — Correctness bugs (fix early; some are memory-safety; independent of the foundation)
- **C1 (MEMORY-SAFETY): BCE `collect_loop_body` is unsound for multi-block loops** (bce.rs:617 —
  ascending-BlockId scan that breaks at first back-edge). A false `bce_safe` mark on a `StoreIndex`
  → out-of-bounds write with NO panic. Fix: use the dominator-based natural-loop collection from
  licm.rs. HIGH PRIORITY (silent OOB).
- **C2 (CORRECTNESS): needs_exception_stack polarity trap.** `_function_needs_exception_stack`
  (frontend:2964) opts functions out of EH bookkeeping by a syntactic scan, but raising callees set
  the pending flag regardless → a `needs_exception_stack=False` lambda with a raising call (e.g.
  `lambda: int("x")`) leaves an unobserved pending exception, returns None → silent wrong propagation.
  Only the iterator-consumer manifestation was patched (LOOP_BREAK_IF_EXCEPTION, b8ebc7703/6cb05b104).
  Structural fix: make the per-op exception-routing decision effect-oracle-driven (S3), OR default
  needs_exception_stack=True and lean on check_exception_elim to drop redundant checks.
- **C3: async `*_poll` "TIR roundtrip emitted invalid labels" panic** (simple_backend.rs:2526). A
  CFG pass (check_exception_elim×copy_prop/dce) drops a handler block carrying a check_exception label.
  Fix: block-elimination reachability must preserve blocks that are check_exception label targets.
- **C4: block_versioning arg-remap** (block_versioning.rs:577-598) — verify cloned-block block-args
  other than the guarded value are correctly threaded from predecessors (possible dangling ValueId).
- **C5: LirRepr::for_type two-tier correctness** (lower_to_lir.rs:31) — the analysis-free path
  (ReprOverride=None, used by lower_tir_to_wasm) maps I64→I64 raw, bypassing the MaybeBigInt floor.
  Thread the override consistently so there's one correctness class.

---

## TIER 2 — The engine (built on Tier 0)
- **E1. TIR function inliner** (needs S4). Today only a SimpleIR inliner (passes.rs:313, ≤30/80 ops,
  no loops/try/yield) — it inlines BEFORE SSA so the merged body skips the whole TIR pipeline. Build a
  TIR-level inliner (ValueId/BlockId remap + splice + re-run pipeline on the merged body), bottom-up
  over the call graph, recursion-safe, cost-model-gated (S2). THE keystone: kills call overhead +
  unlocks cross-call opt, monomorphization, CoroElide, generator fusion. (Measure call_internal vs
  call_func ratio first — dynamic calls need devirt to become inlinable.)
- **E2. SROA / object-field promotion** (needs S5; partial without it for NoEscape single-block).
  tuple_scalarize (deforestation.rs) only handles BuildTuple+immediate-unpack. Build general field
  promotion: a NoEscape `ObjectNewBoundStack` with known field offsets → fields become SSA values,
  LoadAttr/StoreAttr removed. Kills the bench_struct memory cliff; prerequisite for CoroElide.
- **E3. Interprocedural escape + purity summaries** (needs S4). escape_analysis.rs marks every `Call`
  arg GlobalEscape (line 338) and only builtins have effects; user fns are opaque-impure. Build
  bottom-up callee summaries (does_not_capture_param[i], is_pure) → stack-alloc across calls + CSE/LICM/
  DCE of pure user calls. Implement the declared-but-dead `ArgEscape` lattice slot.
- **E4. IPSCCP + whole-program constant propagation** (needs S4). SCCP seeds params Bottom; module-
  level constants don't cross fn boundaries. Seed constant call-site args; propagate module globals.
- **E5. Function monomorphization / specialization** (needs S4+E1) — THE Julia engine. One body per
  function today; params enter DynBox unless annotated. Build: clone+specialize a function per proven
  arg-repr tuple, run the pipeline on the clone, dispatch call sites to the specialization, keep the
  generic as fallback. + return-type backpropagation (E4) + union splitting (exploit TirType::Union).
- **E6. MemorySSA-driven memory opts** (needs S5): store-to-load forwarding, redundant-load elim
  (MemGVN), cross-block DSE, LICM-of-loads.

---

## TIER 3 — Consequences (fall out of the engine)
- **D1. Coroutine-frame elision + generator inlining** (needs E1+E2; escape_analysis.rs:526 marks
  AllocTask GlobalEscape today). = LLVM CoroElide + Codon generator-inlining. Eliminates the heap
  frame + the per-yield (value,done) tuple. → **os.walk-as-CPython-Python-generator** (the original
  thread) fast via fusion; retire the native os.walk + itertools iterators. The proving ground.
- **D2. Reuse-analysis backend actualization** (analysis exists, reuse_analysis.rs; NO backend emits
  molt_reuse_token/alloc — dead since line-15 TODO). Wire into native/WASM/LLVM. Real alloc win, low risk.
- **D3. Unboxed call ABI** (needs Repr + E1) — proven-RawI64Safe call sites pass raw i64, no box/unbox
  round-trip. repr_by_value is ready; call lowering ignores it.
- **D4. Deopt wired end-to-end** (skeleton exists; nothing emits DeoptState/molt_deopt_transfer).
  Enables aggressive speculation with re-optimizable fallback (needs E5 + guard infra).
- **D5. Polymorphic/megamorphic ICs** (today monomorphic-only — object/inline_cache.rs single
  (type_id,offset,version); a 2-type site permanently misses). 2–4-entry + megamorphic fallback.

---

## TIER 4 — Loops / vectorize / parallel / GPU (the Mojo axis; needs S6)
- **L1. IV canonicalization + complete strength reduction** (strength_reduction.rs only x*2/x**2;
  shift/mask deferred for lack of a block-mutation API; NO IV strength reduction `i*stride→+=stride`).
- **L2. REAL SIMD codegen.** vectorize.rs is annotation-only and the hint is a DEAD read
  (lowering.rs:2698 `let _ = has_attr`); native/WASM emit zero SIMD. Build: LLVM loop-vectorize
  metadata (llvm-sys LLVMSetMetadata), Cranelift SIMD, WASM simd128. Populate the empty `Dialect::Simd`.
- **L3. REAL polyhedral.** polyhedral.rs is a stub (fires on wrong ops post-devirt, tile=32 hardcoded,
  zero transforms). Build dependence analysis + tiling/interchange/fusion (ISL-style) or wire Polly.
- **L4. Loop transform family** — fusion/fission/interchange/rotation/peeling/unroll-and-jam (none exist);
  generalize loop_unroll beyond constant-trip≤8.
- **L5. Auto-parallelism** — `Dialect::Par` empty, GIL-removal Phase 1 only, refcounts NON-ATOMIC.
  PREREQUISITE: thread-safe refcounting (atomic RC or epoch GC) before any parallel object sharing.
- **L6. GPU codegen** — today @gpu.kernel is CPU simulation on native; no TIR→GPU-IR (MSL/WGSL/PTX/
  SPIR-V); fusion.py is CPU fallback. Honor tinygrad/DFlash fidelity (hard contract). Long arc.

---

## TIER 5 — Whole-program & feedback (biggest real-workload levers)
- **W1. PGO** — ENTIRE FAMILY MISSING (infra is dead code: PgoProfileIR + llvm pgo.rs exist but
  pgo_branch_weights is always None at lowering.rs:8751; no instrument/collect/feedback). Typically
  10–30% on dynamic-dispatch-heavy Python. The single largest missing optimization family. Build:
  instrumented build → counter collection → profile merge → profile-directed inline/layout/devirt/
  unroll/vector-width. The hot_functions + apply_profile_order hooks already wait for it.
- **W2. CHA + speculative devirtualization** — no class-hierarchy analysis; only range/iter syntactic
  devirt. Build a type-hierarchy graph → resolve `x.foo()` to concrete impls → speculative inline.
- **W3. Dead-field elim / per-attribute DCE** — the real <2MB + startup lever (make_function is an
  unconditional DCE root, cli.py:17043). Prune never-read module attrs from the static graph.
- **W4. LTO wiring** — LtoMode::{Thin,Full} declared but never wired (llvm_backend/mod.rs:79); emit_bitcode
  exists, uncalled. Cross-module inlining for the LLVM target.

---

## TIER 6 — Correctness infrastructure (continuous, gates everything)
- **V1. Translation validation TIR→SimpleIR→native** — the riskiest gap: function_compiler.rs (~36k
  lines) has NO semantic-equivalence guarantee; every runtime bug to date originated in TIR-vs-codegen
  drift. Build a structural translation validator for TIR→SimpleIR (extend tools/translation_validator.py
  beyond pass-to-pass) + Alive2-style SMT equivalence for the LLVM path.
- **V2. Per-pass verification + e-graph integration** — run verify after each pass (not just post-
  pipeline) in debug; integrate the egraph_simplify PoC into the pipeline for algebraic saturation.
- **V3. Lean 4 proof completion** — close the ~73 sorries on the P1 critical path (dceSim, sccpSim,
  fullPipeline); extend to the new Tier-0/2 passes as they land.

---

## Dependency-correct build order (the program)
1. **Tier 1 correctness first** (C1 memory-safety is non-negotiable; C2/C3 are hang/wrong-result).
   These are independent of the foundation and several are small.
2. **Tier 0 substrate**, in order: S3 (effects oracle, small, fixes a hazard) → S1 (analysis manager)
   → S4 (call graph + module phase) → S5 (alias + MemorySSA) → S6 (SCEV/LVI) → S2 (cost model, woven in).
3. **Tier 2 engine**: E3 (IP escape/purity, cheap, unblocks immediately) → E1 (inliner, keystone) →
   E2 (SROA) → E4 (IPSCCP) → E5 (monomorphization) → E6 (MemSSA opts).
4. **Tier 3 consequences**: D2 (reuse actualization, low-risk win) → D1 (CoroElide+generator fusion →
   os.walk-as-Python) → D3 (unboxed ABI) → D5 (poly ICs) → D4 (deopt).
5. **Tier 4 loops/vector/GPU** (needs S6/S2): L1 → L2 → L4 → L3 → L5(after atomic RC) → L6.
6. **Tier 5 feedback**: W1 (PGO) is high-leverage and largely independent — can parallel the engine
   work; W2/W3/W4 follow.
7. **Tier 6** runs continuously; V1 gates risky lowering work.

## Discipline (non-negotiable, per CLAUDE.md + user mandate)
No stopgaps; each phase lands complete or leaves a clean baton. Conservative-correct first cuts that
EXPAND by measurement; absence of an optimization is a perf bail, NEVER a miscompile. Every phase:
research → design → implement → iterate → recursive adversarial senior-engineer review → differential-
test vs CPython on every shape → perf-gate (≥ CPython on every benchmark × target × profile) before
landing. Build the substrate (Tier 0) before the passes that need it — no per-function workarounds.

## Per-lane detail
Full gap lists with file:line live in the 7 lane reports (this session's research agents). Key
single-most-important findings: IPO=no call graph/TIR inliner; Memory=no alias analysis/MemorySSA;
Type=no monomorphization + deopt unwired; Loops=no SCEV + BCE unsound (C1) + vectorize dead; EH=
needs_exception_stack trap (C2) + no CoroElide; WPO=PGO is a stub + no cost model + no analysis cache;
SIMD/GPU=zero SIMD codegen + GPU is CPU sim + uniform boxed ABI.
