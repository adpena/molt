# molt Compiler Foundation — comprehensive gap analysis & build program

Status: MASTER ROADMAP (2026-06-03). Synthesizes a 7-lane parallel gap analysis (IPO,
memory/alias, type specialization, loops/value-analysis, effects/EH/coroutines, WPO/PGO/cost/
verify, SIMD/GPU/ABI/backends). Each lane mapped molt's coverage vs world-class (LLVM/MLIR/
Julia/Codon/Mojo/GCC/Polly/V8) and adversarially self-reviewed. This is the dependency-correct
build program for "Mojo/Julia but better; Codon but more flexible; Nuitka but way faster."
Builds on [[optimizer_foundation]]; supersedes the narrow generator_fusion framing.
The original June tier proposals below are a dependency/design inventory, not
current absence or completion claims. The dated substrate inspection and
current comparison qualify the live architectural state; the findings ledger
alone owns open work and release acceptance. Verify the existing owner before
implementing a proposal.

## Where molt actually is (don't underestimate it)
Already present and non-trivial: fixpoint type inference w/ IV-seeding + oscillation detection +
guard propagation (type_refine.rs); a single-source-of-truth `Repr` lattice across native/LLVM/WASM
(representation_plan.rs, Phase 0/1 landed); 6-strategy
refcount elimination incl. Deutsch-Bobrow + unique-ownership (refcount_elim.rs); SBBV block
versioning (block_versioning.rs); the prior dead deoptimization skeleton has been deleted rather
than left as unsafe dormant ABI; a Lean 4 formalization of ~15 passes (formal/lean/,
~73 open sorries); an MLIR backend with a real pass manager; a 26-primitive GPU stack; SCCP/GVN/LICM/
DCE/canonicalize/strength-reduction/BCE/check_exception_elim. This is advanced for an early compiler.

The gaps are the LOAD-BEARING ones — the passes/substrates that make everything else compound.

---

## TIER 0 — Foundational substrate (everything depends on these; build first)
These dependency families now have live implementations. The June absence
statements were superseded by the following source inspection on 2026-10-14;
implementation presence is not completeness or release qualification. Reuse
these owners, preserve the build order below, and close their actual consumer
and precision gaps instead of building parallel substrates.

- **S1. Pass + Analysis Manager.** `molt-passes/src/tir/pass_manager.rs` owns
  pass order and mutation classes; `analysis/mod.rs` owns lazy analyses with
  CFG/operation invalidation. Main and terminal-drop pipelines consume it.
  `MOLT_VERIFY_ANALYSIS` compares cached analyses with fresh recomputation.
  Arbitrary fixpoint scheduling and every consumer's preservation claim still
  require their own evidence.
- **S2. Unified cost model / TargetTransformInfo.**
  `molt-ir/src/tir/target_info.rs` owns typed capabilities, lowering policy,
  costs and profiles; the pass manager/inliner consume it and cache identity
  includes its target contract. Remaining work is measured calibration and
  consistent profitability through actual consumers and supported cells.
- **S3. Unified effects/alias oracle.** `op_kinds.toml` generates operation
  facts; `op_semantics.rs` supplies instance-sensitive semantics and
  `molt-passes/src/tir/passes/effects.rs` supplies optimizer legality.
  Callback, exception, memory-region and reference-release effects remain
  distinct. Preserved Copy spellings and scalar annotations do not establish
  purity, exact type, or reference-count neutrality.
- **S4. Call graph + whole-program module phase.** `call_graph.rs`, call
  facts/sites and `module_phase.rs` feed inlining, generator fusion and module
  promotion through `pipeline_cache.rs`. Address-taken functions, initializer
  roots and cross-batch declarations must retain their existing custody;
  they are not equivalent to ordinary direct-call edges.
- **S5. Alias analysis + MemorySSA.** `passes/alias_analysis/` and
  `passes/memory_ssa/` are live, including memory defs/uses/phis over implicit
  exception edges. Memory GVN, dead-store elimination and SROA consume the
  shared AnalysisManager. Generic heap operations remain conservative;
  broader precision requires site-specific alias, escape and ownership proof.
- **S6. Value ranges / ScalarEvolution.** `passes/scev/`, `value_range/` and
  shared representation range queries are live. Unsupported recurrences remain
  Unknown. Python integer exactness does not establish a raw machine carrier;
  inline-window, full-i64, overflow and boxed-BigInt contracts remain explicit.

Final-fact custody remains a convergence obligation. Native now retains
optimized TIR through final transformations and projects emitted operations
and representation/lifetime facts together; LLVM consumes owned final TIR
directly. WASM now uses the same owned module pipeline, eliminates dead typed
operations before release insertion and projects each final body with its
scalar facts and demanded LIR plan. Reachability retires whole pairs; escaped
callables change admission without rewriting the body. The next native and
WASM migration carries typed control flow, ownership, roots and
callable/linkage metadata through machine lowering and retires its string/name
projection. Qualify every normal, skipped-pass and batch consumer separately.
This extends [Century section 4.4](CENTURY_SYSTEMS_PLAN.md#44-compiler-runtime-abi-and-backends);
[the IR contract](../spec/areas/compiler/0100_MOLT_IR.md) owns the current transport
and [the findings ledger](../agent/V1_HANDOFF_FINDINGS.md) owns release status.
[The current leading-project comparison](foundation/77_architecture_convergence.md#current-comparison)
distinguishes remaining consumer gaps from mechanisms already implemented.

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
  Only the iterator-consumer manifestation was patched (LOOP_BREAK_IF_EXCEPTION, 813f789cb/3f8a457e5).
  Structural fix: make the per-op exception-routing decision effect-oracle-driven (S3), OR default
  needs_exception_stack=True and lean on check_exception_elim to drop redundant checks.
- **C3: async `*_poll` "TIR roundtrip emitted invalid labels" panic** (simple_backend.rs:2526). A
  CFG pass (check_exception_elim×copy_prop/dce) drops a handler block carrying a check_exception label.
  Fix: block-elimination reachability must preserve blocks that are check_exception label targets.
- **C4: block_versioning arg-remap** (block_versioning.rs:577-598) — verify cloned-block block-args
  other than the guarded value are correctly threaded from predecessors (possible dangling ValueId).
- **C5: LirRepr::for_type two-tier correctness** (lower_to_lir.rs) — RESOLVED in the
  value-Repr authority cut. `lower_tir_to_wasm` now calls the value-keyed
  `lower_function_to_lir` path, which computes `repr_by_value_for(tir_func,
  Some(&vr))`; the analysis-floor lowering is explicitly named
  `lower_function_to_lir_for_repr_fact_extraction` and is not the production
  WASM carrier path.

---

## TIER 2 — The engine (built on Tier 0)
- **E1. TIR function inliner** (needs S4). Today only a SimpleIR inliner (passes.rs:313, ≤30/80 ops,
  no loops/try/yield) — it inlines BEFORE SSA so the merged body skips the whole TIR pipeline. Build a
  TIR-level inliner (ValueId/BlockId remap + splice + re-run pipeline on the merged body), bottom-up
  over the call graph, recursion-safe, cost-model-gated (S2). THE keystone: kills call overhead +
  unlocks cross-call opt, monomorphization, CoroElide, generator fusion. (Measure call_internal vs
  call_func ratio first — dynamic calls need devirt to become inlinable.)
- **E2. SROA / object-field promotion** (needs S5). Shared SROA removes complete
  unobserved, callback-free raw allocations. General Python class scalar
  replacement additionally requires class/destruction/owned-edge proofs;
  nonescape and known field offsets are insufficient. The unproved class-frame
  opcode is retired. Designs 20/49 own the lifetime and field contracts.
- **E3. Interprocedural capture + effect summaries** (needs S4). Build
  identity-bound bottom-up summaries for parameter aliases, retention, callbacks
  and exceptions, consumed by the shared analyses. Noncapture does not prove
  frame placement or RC freedom. Design 03 owns this proposal; generated
  operation effects are the current authority, not builtin-name hand tables.
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
- **D2. Reuse/FBIP design and implementation** — no executable reuse pass or runtime-token ABI is
  kept in-tree. A future implementation must land end-to-end with lowering consumers, ownership and
  finalizer/weakref proofs, and allocation-count evidence; annotation-only authority is not useful.
- **D3. Unboxed call ABI** (needs Repr + E1) — proven-RawI64Safe call sites pass raw i64, no box/unbox
  round-trip. repr_by_value is ready; call lowering ignores it.
- **D4. Deoptimization wired end-to-end** (no implementation skeleton is kept in-tree).
  Enables aggressive speculation with re-optimizable fallback; rebuilding it requires a first-class
  TIR fact, liveness-backed state materialization, and one ABI shared by every backend/runtime path.
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
- **W1. PGO.** The LLVM instrumentation/profile helpers exist, but production
  `try_lower_tir_to_llvm` passes no profile; helper tests do not establish a
  connected instrument/collect/use path. Join the existing producer and
  lowering consumers before claiming profile-directed optimization. Select
  workloads and measure gains rather than assigning an expected multiplier.
  HF-222 separately owns promoted, persistent performance baselines; a gate
  reporting UNARMED cannot detect a regression.
- **W2. CHA + speculative devirtualization** — no class-hierarchy analysis; only range/iter syntactic
  devirt. Build a type-hierarchy graph → resolve `x.foo()` to concrete impls → speculative inline.
- **W3. Dead-field elim / per-attribute DCE** — the real <2MB + startup lever (make_function is an
  unconditional DCE root, cli.py:17043). Prune never-read module attrs from the static graph.
- **W4. LTO wiring** — not started. An unwired `LtoMode` declaration and an uncalled
  `LlvmBackend::emit_bitcode` were deleted on 2026-10-14; the lane builds bitcode emission with
  its first caller, and keeps the generated-object ABI witness in it. Cross-module inlining for the
  LLVM target, including the runtime's hot helpers.

---

## TIER 6 — Correctness infrastructure (continuous, gates everything)
- **V1. Translation validation.** The shared pass manager already has translation
  validation. Qualify backend lowering equivalence separately: pass-level
  checks do not establish emitted-code semantics. Extend that existing owner
  with independent counterexamples instead of creating a second validator.
- **V2. Per-pass verification + e-graph integration.** Reuse the existing pass
  manager's verification and analysis invalidation. New rewrite integration
  needs its own legality, semantic and cost evidence.
- **V3. Lean 4 proof completion** — close the ~73 sorries on the P1 critical path (dceSim, sccpSim,
  fullPipeline); extend to the new Tier-0/2 passes as they land.

---

## Codon CIR comparison (2026-10-14)

Source: Codon `develop` at 45261e03, read against Molt at e578a709d. Codon is fast for one reason:
every fact it needs is proven at compile time, so it emits direct calls, fixed field offsets, raw
loop counters and no exception checks outside `try`. It gets those proofs by excluding Python
dynamism (64-bit `int`, fixed fields, compile-time `__getattr__`, no rebinding). Molt keeps CPython
semantics, so it cannot exclude the dynamism. It can give every fact three tiers instead:

- **Proven:** emit Codon-style code (direct call, fixed offset, unboxed I64/F64, no check).
- **Guarded:** one inline version compare at region entry; on failure, exit to a generic block
  version (`block_versioning.rs` already versions blocks; Molt has no interpreter to deopt to).
- **Unknown:** the full dynamic protocol, with a per-site inline cache as its fast path.

Each guard names the effects that invalidate it (class layout and the global type version today),
so a call whose summary may mutate types forces a re-check.

Adopt, in order of expected gain:
1. Direct calls for module functions that are never rebound, decided as a TIR call fact, with a
   module-dictionary version guard when not proven. The LLVM backend never takes a direct path for
   `call_guarded` (`llvm_backend/lowering/call_lowering.rs`), and the native guard makes five
   runtime calls per Python call (HF-185).
2. Typed field ops (`FieldLoad{class, slot}`, `FieldStore`, `ClassVersionGuard`) from a
   class-hierarchy analysis that proves the receiver class and the absence of `__getattr__`,
   `__getattribute__` and data-descriptor overrides (design 68). The per-site field cache in
   `builtins/attributes/field_sites.rs` is the unknown-tier fast path.
3. Guarded specialization of `call_internal` callees per proven argument-type tuple (Codon's
   realization), keeping the generic body.
4. Exception checks as a generated "may raise" op fact, lowered to an inline sentinel compare on the
   returned bits; the pending flag is read only at handler entry.
5. A counted-loop region carried through TIR (Codon's `ImperativeForFlow`) instead of re-detected
   twice, with the induction value in a register and no per-iteration frame-home store unless an op
   that may raise needs the line.
6. Purity and per-argument capture summaries in `FunctionSummary`, generated from the intrinsics
   manifest.
7. Runtime hot helpers shipped as bitcode for ThinLTO (W4), so LLVM can inline tag checks, field
   access, reference counting and the pending-exception check.
8. Typed op identity: TIR folds about 20 spellings into one `OpCode` and keeps the spelling only as
   the `_original_kind` string, so every backend re-matches strings for the same decision.

Reject, because Molt keeps CPython semantics: wrapping 64-bit `int`, fixed fields with compile
errors for unknown attributes, compile-time-only `__getattr__`, Boehm GC (`__del__` and weakref
timing need reference counts), operators only as magic-method calls (Molt's opcodes encode the
`__radd__`/`NotImplemented` protocol), and Itanium unwinding everywhere (WASM and Luau are
co-equal targets).

## Dependency-correct build order (the program)
The Tier-0 substrate has landed (see `foundation/00_integrated_parallel_program.md`). The order for
the IR boundary, the runtime boundary and typed identity is in
`foundation/77_architecture_convergence.md`; it precedes further pass work because pass facts do
not reach codegen intact until backends consume typed IR.

1. **Tier 1 correctness first** (C1 memory-safety is non-negotiable; C2/C3 are hang/wrong-result).
   These are independent of the foundation and several are small.
2. **Tier 0 consumers**: reuse the implemented S1–S6 owners above and complete
   their actual precision, preservation and backend-consumer gaps; do not
   rebuild the substrate from the original absence statements.
3. **Tier 2 engine**: E3 (IP escape/purity, cheap, unblocks immediately) → E1 (inliner, keystone) →
   E2 (SROA) → E4 (IPSCCP) → E5 (monomorphization) → E6 (MemSSA opts).
4. **Tier 3 consequences**: D2 (reuse actualization, low-risk win) → D1 (CoroElide+generator fusion →
   os.walk-as-Python) → D3 (unboxed ABI) → D5 (poly ICs) → D4 (deopt).
5. **Tier 4 loops/vector/GPU** (needs S6/S2): L1 → L2 → L4 → L3 → L5(after atomic RC) → L6.
6. **Tier 5 feedback**: W1 (PGO) is high-leverage and largely independent — can parallel the engine
   work; W2/W3/W4 follow.
7. **Tier 6** runs continuously; V1 gates risky lowering work.

## Discipline (non-negotiable, per CLAUDE.md + user mandate)
No stopgaps; each contract lands with its complete consumer migration.
Absence of an optimization never licenses a miscompile. Research, reconcile the
existing owner, implement, review adversarially and prove the changed contract
with independent CPython/IR/runtime oracles. Measure affected load-bearing
costs and reuse unchanged evidence. Focused proof precedes broader execution;
the canonical release matrix still owns full conformance and CPython-floor
acceptance. A small repair does not require unrelated full-matrix reruns, and
one passing cell does not qualify the matrix.
