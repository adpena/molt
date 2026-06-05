# RC Ownership & Drop Insertion Substrate (Design 20)

**Document status**: Implementation-ready design.
**Scope**: All refcounting backends — native/Cranelift, LLVM, WASM. Luau is GC-managed (no-op). This is a complete structural arc, not a partial fix.

---

## Executive Summary

Every expression-result heap object created in a molt-compiled function is allocated with `ref_count = 1` (`object/mod.rs:1228` for `alloc_object`, `:1155` for `alloc_object_zeroed`) and **never decremented**. The runtime's `dec_ref` machinery, the TIR `DecRef` opcode, and the `molt_dec_ref_obj` C-ABI function all exist and are correct in isolation; what is missing is the compiler pass that *inserts* `DecRef` ops for expression temporaries in the first place.

The consequence is a whole-program memory leak on every refcounting backend. The evidence is unambiguous:

- 1M-iteration BigInt accumulator loop: 3,000,635 allocations, 0 deallocations, 297 MB RSS at exit, current_rss == peak_rss (the dealloc path of `dec_ref_ptr` at `object/mod.rs:1812` never fires for expression temps).
- 30M-iteration string concat: OOM at 512 MB cap.
- The native-backend `function_compiler.rs` contains a partial per-loop-variable dec-ref heuristic (lines 3566-3628) that fires only on loop-body variable *reassignment*, guarded by an ad-hoc exclusion list (`rc_skip_dec`). This handles `a = b = ...` reassignment within loops but is not an ownership model: it is a symptom-suppressor for one narrow shape and is invisible to the TIR pipeline.

The fix is a first-class **TIR-level `DropInsertion` pass** that runs post-optimization and emits `DecRef` ops at every value's last use, with representation-aware filtering (raw scalar lanes carry no refcount), exception-edge correctness (a value live on both the normal and exception successor must be dropped on each path exactly once), loop-carried ownership (phi-joined owned values must be dropped at the loop exit, not on back-edge), and suspension-point survival (generator/async frames own their live values across yields). After insertion, the existing `refcount_elim` pass (tir/passes/refcount_elim.rs) handles elision.

**Performance contract**: the raw scalar lane (overflow-peel fast loop, RawI64Safe accumulators) adds zero DecRef ops. The overhead on hot boxed-int loops is one `molt_dec_ref_obj` call per dead temp, which the elim pass and borrow inference can reduce to zero when the value is never heap-exposed. The expected steady-state cost on `bench_sum` is zero (fast loop carries RawI64Safe); on `bench_fib` it is one inlined tag-check + branch per iteration (the intermediate BigInt temp).

---

## 1. Ownership Model Specification

### 1.1 Fundamental Invariant

Every operation that returns a new heap reference returns it with `ref_count += 1` relative to the caller's view. This is CPython's convention and molt's runtime implements it consistently: `molt_add`, `molt_mul`, string concat, `alloc_object`, `alloc_list`, `bigint_bits`, all return *owned* references. The callee never decrefs its arguments (it borrows them). This means:

- **Owned**: the current SSA value-holder is responsible for exactly one dec-ref before it goes out of scope.
- **Borrowed**: the value was not newly allocated by this operation and the holder has no dec-ref obligation unless it inc-refs first.

### 1.2 Ownership Table by OpCode

The following table specifies the ownership state of the **result** of each major opcode class, and the ownership treatment of its **operands** (borrow = no transfer, takes-ownership = decrefs its argument internally).

| OpCode class | Result | Operands |
|---|---|---|
| `Add`, `Sub`, `Mul`, `Div`, `FloorDiv`, `Mod`, `Pow`, `Neg`, `Pos`, `InplaceAdd`, `InplaceSub`, `InplaceMul` | Owned (fresh allocation when BigInt/str result) | Borrowed |
| `CheckedAdd` | Raw i64 pair (RawI64Safe, no heap) | Borrowed |
| `Eq`, `Ne`, `Lt`, `Le`, `Gt`, `Ge`, `Is`, `IsNot`, `In`, `NotIn` | Owned if result is a heap bool, else inline | Borrowed |
| `BitAnd`, `BitOr`, `BitXor`, `BitNot`, `Shl`, `Shr` | Owned (BigInt result possible) | Borrowed |
| `And`, `Or`, `Not`, `Bool` | Owned (the Python `True`/`False` objects are immortal; inline bool is unboxed; DynBox bool is borrowed-and-inc'd by the runtime op) | Borrowed |
| `Alloc`, `ObjectNewBound` | Owned (heap-allocated, rc=1) | N/A |
| `ObjectNewBoundStack` | Stack slot, no RC | N/A |
| `StackAlloc` | Stack, no RC | N/A |
| `Free` | None | Takes-ownership (frees unconditionally — only emitted by `refcount_elim` Step 6 for proven-unique values) |
| `LoadAttr`, `Index`, `ModuleGetAttr`, `ModuleImportFrom`, `ModuleGetGlobal`, `ModuleGetName`, `ModuleCacheGet` | Owned (runtime ops inc-ref before returning) | Borrowed |
| `StoreAttr`, `StoreIndex`, `ModuleSetAttr`, `ModuleCacheSet` | None | Borrowed (the container inc-refs the value it stores; the caller keeps its own ref) |
| `DelAttr`, `DelIndex`, `ModuleDelGlobal`, `ModuleDelGlobalIfPresent`, `ModuleCacheDel` | None | Borrowed |
| `Call`, `CallMethod`, `CallBuiltin` | Owned | Borrowed (callee borrows args per ABI) |
| `Import`, `ImportFrom`, `ModuleImportFrom` | Owned | Borrowed |
| `BuildList`, `BuildDict`, `BuildTuple`, `BuildSet`, `BuildSlice` | Owned | Elements are *inc-ref'd by the container*; the builder still holds its own ref and must dec-ref |
| `GetIter`, `IterNext`, `IterNextUnboxed`, `ForIter` | Owned (new iterator or next-value allocation) | Borrowed |
| `AllocTask`, `StateSwitch`, `StateTransition`, `StateYield`, `ChanSendYield`, `ChanRecvYield` | Varies (see §1.3 generators) | Borrowed |
| `ClosureLoad` | Owned (runtime inc-refs before returning) | Borrowed |
| `ClosureStore` | None | Borrowed (cell inc-refs the stored value) |
| `Yield`, `YieldFrom` | None (sends value out) | Borrows arg — but see §1.3 |
| `Raise` | None | Borrowed (exception system takes ownership) |
| `CheckException`, `ExceptionPending` | Inline bool, no RC | Borrowed |
| `ConstInt`, `ConstFloat`, `ConstBool`, `ConstNone` | Inline (no heap) | N/A |
| `ConstStr`, `ConstBytes`, `ConstBigInt` | Owned (materialized at entry; see §1.4) | N/A |
| `Copy` | Borrowed alias (same bits, no new ref) | Borrowed |
| `BoxVal` | Owned (allocs if needed) | Borrowed |
| `UnboxVal` | Inline (strips the box, no new ref) | Consumed (the unboxed value takes over the ref — treated as Owned by the consumer) |
| `TypeGuard` | Borrowed alias | Borrowed |
| `IncRef` | None | Owned-now (op increments the ref; result remains a borrowed alias but is now safe to hold across a barrier) |
| `DecRef` | None | Releases ownership |
| `OrdAt` | Inline i64 (no heap) | Borrowed |
| `WarnStderr`, `Deopt` | None | Borrowed |
| `ScfIf`, `ScfFor`, `ScfWhile`, `ScfYield` | Varies by region | Borrowed |

### 1.3 Generator and Async Suspension Points

`StateYield`, `ChanSendYield`, `ChanRecvYield`, `Yield`, `YieldFrom` are suspension points. At a suspension, all SSA values that are live *across* the yield (used after the next resume) must be treated as escaping into the coroutine frame. The coroutine frame owns those references while suspended. Consequently:

- Live-across-yield values must be inc-ref'd before the yield and dec-ref'd on frame teardown (gen.close()/forced drop), not at the next use.
- Values used only *before* the yield are still dropped at their last use before the yield.
- The suspension opcodes themselves are `is_rc_barrier` (alias_analysis.rs already classifies them as such) and `is_heap_exposing` in `refcount_elim.rs:79`.

Frame teardown (`AllocTask` frame with gen.close()) must dec-ref all live frame slots. This is handled by the existing coroutine finalizer path in `async_rt/generators.rs`; the compiler must ensure the frame *has* those refs at suspension — which the IncRef-before-yield rule above guarantees.

### 1.4 Constant String/Bytes/BigInt Materialization

`ConstStr`, `ConstBytes`, `ConstBigInt` ops materialize a new heap object on each call to the generated function. They produce owned references (rc=1 at birth). If the constant is used once and dropped, that is one alloc + one dec-ref = zero net leak. If the function caches the materialized constant across calls (the natural optimization), the cached slot must be accounted for.

The correct long-term treatment is to intern constants as immortal (the existing `HEADER_FLAG_IMMORTAL` mechanism) at module init. For the initial implementation, treat `ConstStr`/`ConstBytes`/`ConstBigInt` results as Owned and let the drop pass insert dec-refs as for any other value; the SROA/SCCP passes will hoist/CSE repeated uses to one live copy. A follow-up pass (not in scope here) converts hot constants to immortal module-level statics.

### 1.5 Runtime Call Convention Table

The following summarizes the C-ABI that generated code and the runtime both commit to. This table is the contract; both sides must honor it:

| Call site | Args | Return |
|---|---|---|
| `molt_add(a, b)` through `molt_mod(a, b)` | Borrowed (caller keeps refs) | Owned (+1 ref to caller) |
| `molt_inc_ref_obj(bits)` | Takes bits by value, no ownership change to caller (caller still owns) | void |
| `molt_dec_ref_obj(bits)` | Releases one reference to `bits`; may free | void |
| `molt_get_attr_name(obj, name)` | Borrowed | Owned |
| `molt_store_attr_name(obj, name, val)` | Borrowed | void |
| `molt_object_new_bound(class_bits)` | Borrowed (class is module-resident) | Owned |
| Compiled function call `f(a, b, ...)` | Borrowed (callee borrows all args) | Owned |
| `molt_iter_next(iter)` | Borrowed | Owned |
| Generator `_poll(frame, send_val)` | Borrowed | Owned |

---

## 2. The Insertion Algorithm

### 2.1 Insertion Point Choice

Drop insertion runs as a TIR pass **post-optimization, pre-lowering**, in the `build_default_pipeline` ordering after `check_exception_elim` and `dce` but before the `lower_to_simple` round-trip. This position guarantees:

1. SSA is stable (no further CFG or ops mutations).
2. Representation facts (`repr_by_value`) are computed (the `ValueRange`/`Repr` analyses ran during optimization).
3. Liveness analysis over the final SSA is sound.
4. The result (IncRef/DecRef ops) round-trips through `lower_to_simple` (tir/lower_to_simple.rs:1898-1907 already maps `OpCode::IncRef`/`DecRef` to `"inc_ref"`/`"dec_ref"` SimpleIR kinds).
5. The downstream `refcount_elim` pass, which runs *during* optimization (currently pass 12 in the 28-pass sequence), will subsequently be moved to *also* run post-insertion to elide the ops the inserter places redundantly (the existing elim pass is fully correct and will handle this).

Insertion point (b) is confirmed as the right choice. Options (a) (frontend pre-optimization) and (c) (per-backend) are rejected: (a) requires all optimization passes to maintain RC invariants under transformations — a large unsound surface, (c) triplicates logic and is the root cause of the current state.

### 2.2 Analysis Dependencies

The `DropInsertion` pass consumes:
- `ImmediateDoms` and `PredMap` (from `AnalysisManager`, tir/analysis/mod.rs) — for dominator-aware liveness backpropagation.
- `LoopForest` — to identify back-edges and loop-exit edges where loop-carried phis must be dropped.
- `AliasAnalysis` (tir/passes/alias_analysis.rs) — for `is_rc_barrier` queries that bound where a value is safe to hold across, and for `escape_state` to know which values have stack-only lifetime.
- `ValueRange` / `Repr` information threaded from `representation_plan.rs` — to filter out raw scalar values (`Repr::RawI64Safe`, `Repr::Bool`, `Repr::FloatUnboxed`) that carry no heap reference.

The pass is `Mutates::OpsOnly` because it only inserts `DecRef`/`IncRef` ops within blocks and never changes the block set, edges, or terminators.

**Critical note**: `IncRef`/`DecRef` opcodes are already listed as `opcode_is_side_effecting` in `effects.rs:171-172`. The `OpsOnly` constraint in `pass_manager.rs:66-68` explicitly states that `OpsOnly` passes must NOT add/remove ops that carry exception edges. `DecRef`/`IncRef` do not carry exception edges (they are not `CheckException`/`TryStart`/`TryEnd`/`StateBlock*`), so inserting them is sound under `OpsOnly`. However, because they are side-effecting, DCE will not remove them after insertion; this is correct.

### 2.3 Liveness Computation

Compute per-value liveness over the final SSA using a standard backward dataflow. The algorithm:

```
LiveOut[B] = ⋃ { LiveIn[S] | S is a successor of B }
LiveIn[B] = (LiveOut[B] \ Kill[B]) ∪ Use[B]
```

where:
- `Use[B]` = set of values used by ops in B before any definition in B (including block args).
- `Kill[B]` = set of values defined by ops in B (results of non-phi ops).
- Terminator branch args contribute to `Use` of the current block.
- Block args of successors that receive a value from this block's terminator contribute to `Use`.

Representation filter: when computing `Use` and `LiveOut`, exclude any value whose `Repr` is `RawI64Safe`, `Bool`, or `FloatUnboxed`. Raw scalar values carry no heap reference; inserting DecRef for them would be a type error and generate invalid code.

For blocks with multiple successors, a value is live-out if it is live-in in any successor. This is standard and over-approximates; the elim pass will remove any provably-redundant drops.

### 2.4 Drop Placement — Straight-Line Code

For each basic block B, work forward through its ops. Track the set of currently-live owned values at each program point. After the last op that uses a value V (V is in `Use[B]` but not used after position I), insert `DecRef(V)` immediately after op I if:
- V is not live-out of B (would be redundant if dropped at a successor).
- V's repr is not a raw scalar.
- V is not a block argument that flows to a successor (handled at the edge).
- V is not a StackAlloc/ObjectNewBoundStack result (stack, no RC).

A value is "last-used at op I" when it appears in I's operands and does not appear in any op's operands at positions I+1...N, and does not appear in the terminator's branch arguments.

### 2.5 Edge-Carried Ownership and Phi Joins

When control-flow joins (block B has multiple predecessors), a value that is live-in to B may arrive from different predecessors with different live/dead status. The rule is:

- If V is live-in to B (i.e., live in B's argument set or used early in B), then each predecessor that does NOT use V after its last op and does NOT pass V as a branch argument to B must insert `DecRef(V)` on the edge (i.e., at the end of that predecessor block, before the terminator).
- If V IS passed as a branch argument, the ownership is "transferred" through the SSA phi: no drop needed on that edge.

This ensures each path drops V exactly once. The CondBranch case:

```
bb1: x = molt_add(a, b)     // x is Owned
     CondBranch(cond, bb2[x], bb3)   // x transferred to bb2 but NOT bb3
     // → Insert DecRef(x) on the edge to bb3
```

In TIR's MLIR-style block-argument encoding, "insert on the edge to bb3" means inserting `DecRef(x)` at the end of bb1 on the bb3 path. Because TIR has no explicit edge blocks (the CondBranch is the last op in bb1), the insertion must be done by splitting the edge: insert a new intermediate block `bb1_exit_bb3` containing `DecRef(x)` and retargeting the CondBranch's else-branch to `bb1_exit_bb3` which unconditionally branches to bb3. Edge-block splitting is a CFG mutation (`Mutates::Cfg`) and must be done before the final `OpsOnly`-only phase; alternatively, if both then and else arms of the CondBranch drop the same set, the drops can be inserted just before the terminator in bb1 (common-prefix hoisting — the refcount_elim loop-invariant pass handles this).

**Implementation choice**: to keep the initial pass simpler, emit drops at the *beginning* of successor blocks for values that die on entry rather than splitting edges. This keeps the pass `OpsOnly` (no block creation). The elim pass then handles the common case where both successors drop the same value by hoisting the drop to the predecessor. The edge-split form (cleaner, avoids redundant drops on hot paths) is the Phase 3 upgrade.

### 2.6 Exception Edges

C2 (commit `430e09793`) made exception observation universal: every potentially-throwing op is followed by `CheckException(→ handler_label, → normal_label)`. Values that are live at the throw site must be dropped on BOTH the normal and exception continuation paths if they are dead after the check.

The algorithm handles this naturally: `CheckException` is `is_rc_barrier` (alias_analysis.rs: yes, it is — it observes and potentially modifies the exception state). When computing the last use of V in a block, if V is used before a `CheckException` and not used after, the drop must be inserted on both successor paths (normal continuation and handler). If V is used after the `CheckException` (i.e., only on the normal path), the drop goes only on the normal path; the handler path must also drop V because V is live at the throw point.

Concretely: V is live at the `CheckException` op if V appears in any op at or before the `CheckException` and in at least one op after the `CheckException` on the normal path. If V is live-in to the handler block, it must be dropped there.

The existing `refcount_elim` pass already handles the common case (adjacent IncRef+DecRef across barriers) and will elide pairs the inserter emits redundantly.

### 2.7 Loop-Carried Ownership

A loop-carried phi value is a block argument of a loop header that is updated on the back-edge. Example:

```
entry: total = ConstInt(0)           // inline, no heap
loop_header(total):
  v1 = molt_add(total, step)         // v1 is Owned (new heap object when BigInt)
  DecRef(total)                       // total from previous iteration no longer needed
  loop (back-edge passes v1 as total)
loop_exit:
  DecRef(total_final)                 // loop exit value must be dropped
```

The rule: for each back-edge `→ loop_header(new_val)`, if `old_val` (the phi register for the preceding iteration) is not used after this point in the body, insert `DecRef(old_val)` just before the back-edge branch. This is the "consumer releases the slot" rule — equivalent to CPython's `STORE_FAST` dec-ref on overwrite.

The existing partial implementation in `function_compiler.rs:3566` (`loop_reassign_old_val`) does exactly this for the SimpleIR codegen path. The TIR drop pass supersedes it structurally: the SimpleIR path's ad-hoc dec-ref must be disabled/guarded when the TIR drop pass is active (Phase 4 cleanup).

At loop *exit*, any loop-carried phi that is not returned or stored must be dropped. This is the "dead on exit" case handled by the straight-line placement rule above.

### 2.8 Representation-Aware Filtering

Before inserting any `DecRef(V)`:
1. Obtain V's `Repr` from `representation_plan::repr_by_value` (or `Repr::default_for(&type_of_V)` for values not in the map).
2. If `Repr::RawI64Safe` → skip (bare i64 register, no heap ref).
3. If `Repr::Bool` → skip (inline bool tag, no heap ref).
4. If `Repr::FloatUnboxed` → skip (bare f64 register, no heap ref).
5. If `Repr::MaybeBigInt` or `Repr::DynBox` → insert the DecRef. The runtime's `molt_dec_ref_obj` fast-paths non-pointer tags (`ops.rs:7087-7090`), so inserting a DecRef for a value that turns out to be inline at runtime is safe but wasteful. The inline tag-check in `emit_dec_ref_obj` (`simple_backend.rs:1086-1103`) already short-circuits this at the Cranelift level.
6. `Repr::Never` → dead value, no insert needed.

This filtering ensures that the `overflow_peel` fast loop's raw-i64 accumulators receive zero RC ops — the performance contract is preserved structurally.

### 2.9 Suspension Point Survival

For each `StateYield`, `ChanSendYield`, `ChanRecvYield`, `Yield`, `YieldFrom` op:
1. Compute the set of values live-across-this-yield (used after the matching resume point or in a post-yield block).
2. For each live-across value V that is `Owned`:
   - Insert `IncRef(V)` immediately before the yield op (the frame now holds its own reference to V while suspended).
   - The *existing* reference remains live in the frame; the yielded value itself is a borrow to the caller.
3. On resume: no additional action — the IncRef'd reference is consumed at the point of last use post-resume.
4. On generator close (teardown): the frame's coroutine finalizer is responsible for dropping all alive frame slots. The finalizer already walks the GEN frame slots and calls `dec_ref_bits` for each (async_rt/generators.rs). The IncRef-before-yield above ensures the frame slot has a valid reference for the finalizer to release.

This is the minimal correct model. The Perceus optimization (reuse_analysis) handles the common case where the frame slot and the resume-local alias are fused, eliminating the IncRef/DecRef pair.

---

## 3. The Elision and Optimization Layer

Drop insertion produces a correct but potentially un-elided set of RC ops. The pipeline then applies optimizations in this order, using existing passes with targeted extensions:

### 3.1 refcount_elim (existing, tir/passes/refcount_elim.rs)

Already implements:
- Intra-block adjacent IncRef+DecRef pair elimination (Steps 2a/2b).
- StackAlloc value RC removal (Step 2a).
- Cross-block dominator-edge elimination (Step 3).
- Loop-invariant IncRef+DecRef elimination (Step 4).
- Deferred-RC: values with no heap exposure have all their RC ops removed (Step 5).
- Unique-ownership DecRef→Free promotion (Step 6).

After drop insertion, `refcount_elim` runs again (a second invocation is added to the post-insertion pass sequence). The new insertion supplies the ops that the elim pass was previously starved of — now it can prove more elisions.

### 3.2 Borrow Inference (new, part of DropInsertion)

During the drop insertion phase, when computing whether a value requires an IncRef before passing to a function call, apply borrow inference:

- If a Call/CallMethod/CallBuiltin op borrows V and V is immediately DecRef'd after the call returns (V is dead after the call), the IncRef+DecRef pair is a no-op and neither is emitted. The callee borrows V for the call's duration; the call returns before the drop; the net refcount change is zero.
- Formally: if V's last use IS the call operand, do not insert `IncRef(V)` before the call and do not insert `DecRef(V)` after. The existing refcount convention (callee borrows, caller drops at last use) is exactly this rule applied correctly.

This eliminates the dominant pattern: `result = f(x); ...use result...; // x is dead → no IncRef/DecRef for x around the call`.

### 3.3 reuse_analysis Integration (existing, tir/passes/reuse_analysis.rs)

After drop insertion, reuse_analysis has a richer set of `DecRef` → `Alloc` pairs to work with. The Perceus-style reuse credit means the drop of an old BigInt can be fused with the allocation of the new one (same size class: both are `TYPE_ID_BIGINT`), eliminating one alloc+free pair per iteration in BigInt accumulator loops. The reuse pass already produces `ReuseCandidate` annotations; Phase 2 of this substrate (future, not in this arc) implements the runtime reuse-token emission (`molt_reuse_token` / `molt_reuse_alloc`).

### 3.4 Expected Overhead on Hot Benchmarks

| Benchmark | Current | Post-drop-insertion | Notes |
|---|---|---|---|
| `bench_sum` (RawI64Safe) | Baseline | 0 overhead | Fast loop carries raw i64, Repr filter eliminates all drops |
| `bench_fib` (BigInt at large n) | Leaks | ~1 inlined tag-check/branch per iteration | `emit_dec_ref_obj` fast-path; Perceus fuses alloc/free |
| `bench_sieve` (list-int, no boxed int) | Baseline | 0 overhead | `FlatListInt` repr, no heap-alloc int temps |
| string concat loop | OOM | Bounded by 1 drop/iter | Must drop old string before alloc of new one |
| BigInt accumulator (n=1M) | 297 MB leak | O(1) RSS | Drop closes the loop on each iteration's temp |

---

## 4. Backend Lowering

### 4.1 Native / Cranelift

`OpCode::DecRef` already lowers in the SimpleIR path via `emit_dec_ref_obj` (`simple_backend.rs:1076`), which emits an inlined tag check + conditional `molt_dec_ref_obj` call. The TIR→SimpleIR round-trip maps `DecRef` to `"dec_ref"` (`lower_to_simple.rs:1903`), which the Cranelift backend handles at `function_compiler.rs:3630` in the `match op.kind.as_str()` handler.

No new native-backend code is needed for DecRef emission — the mechanism already exists. What changes is that the TIR pass now *populates* those ops; the backend transparently lowers them.

The existing loop-body reassignment dec-ref in `function_compiler.rs:3566-3628` must be **disabled** once the TIR drop pass is live for that function, to avoid double-drops. The disable condition: if the function's TIR was processed by the drop insertion pass (detectable by a function-level attr `"drop_inserted": true` set by the pass), skip the `loop_reassign_old_val` path in the SimpleIR backend. This is the Phase 4 cleanup task; it is not a structural blocker for Phase 1 correctness because the loop-reassign path only fires on a narrow subset and the TIR drop pass inserts the same DecRef, but it WILL cause double-free if both paths fire simultaneously. **Phase 1 must include this guard from the start.**

> **ACTIVATION FINDING (2026-06-05, RC activation session) — §4.1 understated the native RC overlap; it is an ACTIVATION PREREQUISITE, not a Phase-4 cleanup.** Two things were discovered when DropInsertion was wired into `build_default_pipeline`:
>
> 1. **The activation-blocker abort was a borrow-alias double-drop, NOT carrier resolution.** The lowered loop loads its carried accumulator via `load_var`→`Copy` every iteration; the per-SSA-value drop pass dropped EACH copy of the one live object → refcount underflow → premature free → `invalid object header before dec_ref` UAF at n≥50k. **Fixed**: liveness (`liveness.rs`) and drop placement (`drop_insertion.rs`) now operate in **alias-root space** (a `Copy`/`TypeGuard` borrow alias — §1.2 — shares its root's single ownership obligation; build the union-find via `alias_analysis::build_alias_union_find`). Each heap object is dropped exactly once. A `loop_slot_accumulator_no_double_drop` regression asserts the invariant. Also: the `drop_inserted` marker now round-trips losslessly through `lower_from_simple` (it re-sets the func attr after stripping the transport op — the native module-phase re-lift previously lost it), and DropInsertion is idempotent on a re-lifted function.
>
> 2. **The native backend runs its OWN value-tracking RC that NEGATES the TIR drops on loop-carried accumulators.** `function_compiler.rs` tracks heap results in `tracked_obj_vars` and releases them via `drain_cleanup_tracked_dedup` at exits (Swift-ARC: retain-at-store, release-at-scope-exit), with loop-var `last_use` extended to function-end. For `s = s + "x"` / `total = total + 1`, this tracking keeps the carried object alive so the TIR `DecRef(old)` only brings its refcount 2→1, never to 0 → the **headline leak case (loop accumulators) is NOT closed by activation alone** (measured: string-concat 0/n freed; bigint-accumulator only the 2 dead intermediates/iter freed; O(n) residual RSS). The two ad-hoc loop paths (`loop_reassign_old_val` dec-side, store_var `inc_ref(new)` inc-side) are now guarded by `!drop_inserted`, but the **broad value-tracking system is not** — and gating only those two is insufficient. **Activation prerequisite (Phase 5, expanded):** for `drop_inserted` functions (which never include exception-handler functions — the drop pass bails on those, so the marker is never set there), the entire native value-tracking RC must be suppressed so the TIR drops are the SOLE RC authority: skip heap-result registration into `tracked_*`, skip every `drain_cleanup_tracked_dedup` call (~18 sites at ret/label/check_exception/loop boundaries), and drop the func-end `last_use` extension. This is a multi-site change with real double-free risk and MUST be verified per-site against the corpus under `MOLT_ASSERT_NO_LEAK=1`. Until it lands, the passes stay dormant in `build_default_pipeline` (wiring them ships the O(n) residual leak).

### 4.2 LLVM Backend

`OpCode::DecRef` lowers in `llvm_backend/lowering.rs:1275-1287` to `molt_dec_ref_obj`. Already wired, no new code needed.

### 4.3 WASM Backend

The WASM backend goes through the TIR→LIR→lower_to_wasm pipeline. `OpCode::DecRef` must be wired in `tir/lower_to_wasm.rs`. This is the only backend that requires new code for the DecRef opcode (the LIR path currently does not list DecRef in its opcode lowering). The LIR already carries TIR ops through (`LirOp::tir_op` in `lir.rs:52-55`); the WASM lowering must emit a call to `molt_dec_ref_obj` for `DecRef` ops in the LIR stream.

### 4.4 Luau Backend (no-op)

Luau is GC-managed. All `DecRef` ops are no-ops on the Luau target. The Luau lowering path must recognize `OpCode::DecRef`/`OpCode::IncRef` and emit no instructions. This prevents "unknown opcode" panics if the Luau backend encounters TIR produced by the common pipeline.

---

## 5. Verification and Observability Layer

### 5.1 DEALLOC_COUNT Runtime Counter

Add a `DEALLOC_COUNT: AtomicU64` static to `runtime/molt-runtime/src/constants.rs` alongside the existing `ALLOC_COUNT` (line 88). Also add `DEALLOC_BYTES_TOTAL: AtomicU64` and per-type `DEALLOC_<TYPE>_COUNT` entries mirroring the alloc counters.

The dealloc counter must be incremented in `dec_ref_ptr` in `object/mod.rs` at line 1812, inside the `if prev == 1 { ... }` block (the zero-transition — this is the actual deallocation path). Specifically, after `MoltRefCount::acquire_fence()` at line 1821, before the type-dispatch match at line 1887:

```rust
// In dec_ref_ptr, after acquire_fence() at the prev==1 transition:
profile_hit(py, &DEALLOC_COUNT);
profile_hit_bytes(py, &DEALLOC_BYTES_TOTAL, total_size_from_header_fields(header_size_class, header_cold_idx) as u64);
profile_dealloc_type(py, type_id);
```

### 5.2 End-of-Process Leak Report

Under `MOLT_PROFILE=1`, the existing profile dump (`state/lifecycle.rs` or wherever `ALLOC_COUNT` is printed) must emit:

```
[MOLT_PROFILE] alloc_count=N dealloc_count=M live_objects=(N-M) alloc_bytes=X dealloc_bytes=Y live_bytes=(X-Y)
[MOLT_PROFILE] LEAK WARNING: (N-M) objects not freed at process exit (expected_live=<bootstrap_constant>)
```

`expected_live` is the count of immortal bootstrap objects (module dict, builtin types, etc.) that legitimately survive to process exit. This value is measured at the end of Phase 1 testing and encoded as a test constant.

### 5.3 Differential Harness Mode

Add a `MOLT_ASSERT_NO_LEAK=1` environment variable that at process exit asserts:

```rust
dealloc_count + expected_live_constant == alloc_count
```

If this assertion fails, print the per-type breakdown and abort. This gate is used in the regression test suite.

### 5.4 Regression Test Corpus

Create `tests/differential/memory/` with the verified repro cases from the bug evidence:

- `bigint_accumulator.py` — BigInt accumulator loop, n=1000, assert RSS < 5 MB (run via `safe_run.py --rss-mb 10`).
- `string_concat.py` — string concat loop, n=10000, assert RSS < 20 MB.
- `fib_bigint.py` — fib(20000), assert RSS < 50 MB.
- `list_comprehension.py` — `[x*2 for x in range(10000)]`, assert RSS < 5 MB.
- Each test also sets `MOLT_ASSERT_NO_LEAK=1` and asserts exit code 0.

These tests become continuous gates: any regression in drop insertion or refcount_elim that re-introduces a leak will OOM the `safe_run.py` cap and produce an exit code 137 (RSS cap hit) or fail the `MOLT_ASSERT_NO_LEAK` assertion.

---

## 6. Phase-by-Phase Implementation Plan

Each phase is a complete structural piece. No phase may be committed in a state that leaves the codebase with more leak categories than before. The invariant: after each phase, `MOLT_ASSERT_NO_LEAK` passes on the phase's test corpus.

### Phase 1: Runtime Observability and Test Infrastructure

**Scope**: No compiler changes. Runtime-only. Sets up the measurement layer so all subsequent phases are verifiable.

Files to create/modify:

- `runtime/molt-runtime/src/constants.rs:88` — add `DEALLOC_COUNT`, `DEALLOC_BYTES_TOTAL`, `DEALLOC_OBJECT_COUNT`, `DEALLOC_BIGINT_COUNT`, `DEALLOC_STRING_COUNT`, `DEALLOC_DICT_COUNT`, `DEALLOC_TUPLE_COUNT`.
- `runtime/molt-runtime/src/object/mod.rs:1821` — in `dec_ref_ptr`, at the `prev==1` branch (just before the `maybe_run_object_finalizer` call at `:1883`), increment `DEALLOC_COUNT` and `DEALLOC_BYTES_TOTAL` using `profile_hit`.
- Add `profile_dealloc_type` function mirroring `profile_alloc_type` (`:1250`), called from the same point.
- Wherever `MOLT_PROFILE` output is printed (search for the profile dump in `state/lifecycle.rs` or `lib.rs`), add the leak report section.
- Add `MOLT_ASSERT_NO_LEAK` check at process exit in the same lifecycle site.
- Create `tests/differential/memory/` directory and the four test files above.

**Test specification (Phase 1 gates)**:
- `cargo test -p molt-backend -- memory` — all four differential/memory tests pass with `safe_run.py --rss-mb 20` cap (they will OOM or trip `MOLT_ASSERT_NO_LEAK` before this pass is built — that is expected and is the measured baseline).
- `MOLT_PROFILE=1 molt run tests/differential/memory/bigint_accumulator.py` prints `alloc_count` and `dealloc_count`. At this stage `dealloc_count` is near-zero; this is documented as the pre-fix baseline.
- No regression in any existing test (0 new failures).

**Phase 1 is complete when**: the counters are in place, leak reporting prints, and the test corpus is checked in (tests will fail until Phase 3; that is acceptable — but see the project skipped-test policy: the corpus lands WITH Phase 3 if red tests cannot land).

### Phase 2: TIR Liveness Analysis Primitive

**Scope**: Implement `TirLiveness` as a new `Analysis` registered with `AnalysisId::Liveness` in `tir/analysis/mod.rs`. This is a read-only analysis; it does not modify IR.

Files to create/modify:

- `runtime/molt-backend/src/tir/analysis/mod.rs` — add `AnalysisId::Liveness` to the enum and `ALL` array. Implement `Analysis for TirLiveness`.
- `runtime/molt-backend/src/tir/passes/liveness.rs` (new file) — implement `TirLiveness`:
  - Struct fields: `pub live_in: HashMap<BlockId, HashSet<ValueId>>`, `pub live_out: HashMap<BlockId, HashSet<ValueId>>`.
  - Compute using backward dataflow: iterate until fixpoint; seed with `LiveOut[exits] = {}`.
  - Exclude `Repr::RawI64Safe`/`Bool`/`FloatUnboxed` values from the live sets.
  - `CFG_SENSITIVE`: `true`.
  - Public query: `fn is_live_in(&self, block: BlockId, val: ValueId) -> bool`.
  - Public query: `fn last_use_in_block(&self, block: &TirBlock, val: ValueId) -> Option<usize>` — returns the index of the last op that uses `val` in this block (or `None` if not used in this block at all).
- `runtime/molt-backend/src/tir/passes/mod.rs` — add `pub mod liveness;`.

**Test specification (Phase 2 gates)**:
- Unit tests in `liveness.rs`:
  - Straight-line block: value used at op I and not after → `last_use_in_block` returns `Some(I)`.
  - Value used in both branches of a CondBranch and live-out → stays in live_out of the block.
  - Loop carried value: live-in at header, live-out via back-edge.
  - Raw i64 value: excluded from live sets even when used.
  - Generator yield: value used after yield is live-in to resume block.
- `cargo test -p molt-backend -- liveness` — all unit tests pass.

### Phase 3: Core DropInsertion Pass

**Scope**: The main structural work. Implement `DropInsertion` as a `TirPass` with `Mutates::OpsOnly`.

Files to create/modify:

- `runtime/molt-backend/src/tir/passes/drop_insertion.rs` (new file):
  - `pub fn run(func: &mut TirFunction, am: &mut AnalysisManager, repr_map: Option<&HashMap<ValueId, Repr>>) -> PassStats`
  - Consumes: `TirLiveness`, `ImmediateDoms`, `PredMap`, `LoopForest`, `AliasAnalysis` (from am), repr_map parameter.
  - Straight-line placement: for each block, walk ops, identify last-use positions, insert `DecRef` after last use.
  - Successor-edge placement: for each block-exit edge where a value V is live-in to the predecessor but not live-in to the target successor AND V is not passed as a branch argument to that successor — insert `DecRef(V)` at the end of the current block before the terminator. When the CondBranch has two successors with different dead-value sets, use the "before-the-terminator" insertion for values that die on ALL successors (common-prefix), and for values that die only on one successor, insert after the terminator switch by placing them at the start of the successor block (this keeps the pass OpsOnly — no edge-splitting).
  - Loop-exit placement: detect loop exit edges using `LoopForest`. For phi values that are the back-edge carrier (last live use at the back-edge branch), insert `DecRef` before the loop-exit branch.
  - Suspension handling: for each `StateYield`/`ChanSendYield`/`ChanRecvYield`/`Yield`/`YieldFrom` op, for each value that is live-across-this-yield (in `LiveIn` of the resume continuation block), insert `IncRef(V)` immediately before the yield op.
  - Stack filter: values produced by `StackAlloc` or `ObjectNewBoundStack` — never insert DecRef.
  - Borrow inference: if V's only remaining use after the drop candidate is as an operand to a `Call`/`CallMethod`/`CallBuiltin` where V is dead after the call, and no IncRef is needed (no heap-exposing barrier between definition and call), skip the IncRef+DecRef pair entirely.
  - Set `func.attrs.insert("drop_inserted", AttrValue::Bool(true))` when any ops are inserted.
  - Mutation class: `OpsOnly`.
- `runtime/molt-backend/src/tir/passes/mod.rs` — add `pub mod drop_insertion;`.
- `runtime/molt-backend/src/tir/pass_manager.rs` — add two new pass slots at the end of `build_default_pipeline`:
  1. `pass("drop_insertion", OpsOnly, ...)` — inserts DecRef/IncRef.
  2. `pass("refcount_elim_post", OpsOnly, ...)` — runs refcount_elim again after insertion.
- `runtime/molt-backend/src/native_backend/function_compiler.rs:3577` — inside the `loop_reassign_old_val` computation, add the guard:
  ```rust
  && !func_ir.attrs.contains_key("drop_inserted")
  ```
  This disables the ad-hoc loop-body dec-ref path for functions processed by the TIR drop pass, preventing double-drop.

**Test specification (Phase 3 gates)**:
- Unit tests in `drop_insertion.rs`:
  - Straight-line temp: `v1 = Add(a, b); v2 = Add(v1, c); Return(v2)` → `DecRef(v1)` inserted after `v2`'s definition (v1 is dead after that point).
  - Loop accumulator: verify `DecRef(old_total)` inserted before back-edge branch and `DecRef(total_final)` at loop exit.
  - Exception path: value live at throw site is dropped on both normal and handler paths.
  - Raw i64 value: zero DecRef ops inserted.
  - StackAlloc: zero DecRef ops inserted.
  - Generator yield: `IncRef(V)` inserted before yield for live-across value.
  - Borrow inference: last-use-is-call-arg → no IncRef+DecRef pair.
- Integration tests:
  - `tests/differential/memory/bigint_accumulator.py` with `MOLT_ASSERT_NO_LEAK=1` — passes.
  - `tests/differential/memory/string_concat.py` with `safe_run.py --rss-mb 20` — exits 0, not 137.
  - All existing backend lib tests pass.
  - All 46 compliance tests pass byte-identically.
  - `MOLT_PROFILE=1` shows `dealloc_count ≈ alloc_count - expected_live` on each test.
- Performance smoke test: `bench_sum` runtime within 5% of pre-patch baseline (raw loop, zero new ops).

### Phase 4: WASM Backend Wiring

**Scope**: Wire `OpCode::DecRef` (and `IncRef`) in `tir/lower_to_wasm.rs`. Wire `OpCode::DecRef`/`IncRef` as Luau no-ops.

Files to create/modify:

- `runtime/molt-backend/src/tir/lower_to_wasm.rs` — add handling for `OpCode::DecRef`: emit a call to `molt_dec_ref_obj` (the WASM import already exists for it). Add `OpCode::IncRef`: emit `molt_inc_ref_obj`. The LIR path already carries TIR ops in `LirOp::tir_op`; the WASM lowering function must pattern-match them.
- Luau lowering (wherever OpCode→Luau emission happens): recognize `OpCode::DecRef`/`IncRef` and emit nothing (comment "GC-managed target").
- Remove/disable the SimpleIR native-backend loop-reassign dec-ref path for `drop_inserted` functions (if not already done in Phase 3 — move here if Phase 3 scoping deferred it).

**Test specification (Phase 4 gates)**:
- `tests/differential/memory/bigint_accumulator.py` on WASM target: `MOLT_ASSERT_NO_LEAK=1` passes (requires WASM profile counter support; if not available, use RSS cap).
- All existing WASM-backend lib tests pass.
- `tests/differential/memory/string_concat.py` on WASM: no OOM.

### Phase 5: Legacy SimpleIR Drop Path Unification

**Scope**: Delete the ad-hoc loop-body-reassignment dec-ref in `function_compiler.rs:3566-3628` (the `loop_reassign_old_val` path) and the `compute_rc_coalesce_skips` SimpleIR-level pass in `passes.rs:845`. Replace them entirely with the TIR drop pass. Ensure the `rc_coalescing` SimpleIR pass (`passes.rs:997`) is also retired or subsumed.

This phase requires verifying that every shape previously handled by the SimpleIR paths is now covered by the TIR pass. The test suite (the full differential corpus) is the validation.

Files to modify:

- `runtime/molt-backend/src/native_backend/function_compiler.rs` — delete `loop_reassign_old_val` block and all references to `rc_skip_dec`.
- `runtime/molt-backend/src/passes.rs` — delete `compute_rc_coalesce_skips` and `rc_coalescing`.
- Update all call sites of deleted functions; ensure no compilation errors.

**Test specification (Phase 5 gates)**:
- All backend lib tests pass after deletion (no regression).
- All differential/memory tests still pass.
- `dealloc_count ≈ alloc_count - expected_live` holds across the test corpus — the TIR pass covered every shape the old paths handled.
- No new `clippy` warnings (the deleted code had `#[allow(dead_code)]` suppressors that must be removed).

---

## 7. New File / Modification Map

| File | Action | Key change |
|---|---|---|
| `runtime/molt-runtime/src/constants.rs` | Modify | Add `DEALLOC_COUNT`, `DEALLOC_BYTES_TOTAL`, per-type dealloc counters after line 95 |
| `runtime/molt-runtime/src/object/mod.rs` | Modify | Increment dealloc counters at line 1821 (the `prev==1` branch in `dec_ref_ptr`) |
| `runtime/molt-backend/src/tir/passes/liveness.rs` | Create | `TirLiveness` analysis — backward dataflow liveness |
| `runtime/molt-backend/src/tir/analysis/mod.rs` | Modify | Add `AnalysisId::Liveness`, register `TirLiveness` |
| `runtime/molt-backend/src/tir/passes/drop_insertion.rs` | Create | `DropInsertion` pass — core of this substrate |
| `runtime/molt-backend/src/tir/passes/mod.rs` | Modify | Add `pub mod liveness; pub mod drop_insertion;` |
| `runtime/molt-backend/src/tir/pass_manager.rs` | Modify | Append `drop_insertion` + `refcount_elim_post` to `build_default_pipeline` |
| `runtime/molt-backend/src/tir/lower_to_wasm.rs` | Modify | Wire `OpCode::DecRef`/`IncRef` |
| `runtime/molt-backend/src/native_backend/function_compiler.rs` | Modify | Guard `loop_reassign_old_val` with `!drop_inserted`; Phase 5: delete the block |
| `runtime/molt-backend/src/passes.rs` | Modify (Phase 5) | Delete `compute_rc_coalesce_skips`, `rc_coalescing` |
| `tests/differential/memory/*.py` | Create | Four regression tests |

---

## 8. Data Flow — Complete Path for a BigInt Accumulator

```
Python source:
  total = 0
  while i < n:
      total = total + i

TIR (pre-drop-insertion, post-optimization):
  bb0 (entry):
    total_init = ConstInt(0)          // inline, no heap
    branch bb_header(total_init)

  bb_header(total: DynBox/MaybeBigInt):
    cond = Lt(i, n)
    CondBranch(cond, bb_body, bb_exit)

  bb_body:
    new_total = Add(total, i)         // Owned (new BigInt allocation)
    // i incremented...
    branch bb_header(new_total)       // old total no longer needed here

  bb_exit:
    Return(total)

TIR (post-drop-insertion):
  bb_body:
    new_total = Add(total, i)         // Owned
    DecRef(total)                     // total from previous iteration — last use was Add
    branch bb_header(new_total)

  bb_exit:
    Return(total)                     // consumed by the return ABI; caller dec-refs
```

The per-iteration flow: each `Add` creates a new owned BigInt, the previous one is DecRef'd before the back-edge, so at most one BigInt is alive per iteration. RSS is bounded by O(size-of-one-BigInt) regardless of iteration count.

---

## 9. Risk Register

### R1: Double-drop from loop_reassign_old_val

**Risk**: The SimpleIR-level loop-reassign dec-ref (`function_compiler.rs:3566`) fires on the same value as the TIR drop pass, producing two `DecRef`s on the same object → refcount underflow → use-after-free or abort at `dec_ref_ptr:1764`.

**Treatment**: The guard `!drop_inserted` (Phase 3, `function_compiler.rs:3581`) disables the SimpleIR path for TIR-processed functions. This guard must be in place before the TIR drop pass is enabled in the pipeline. The check is on `func_ir.attrs.contains_key("drop_inserted")` where `func_ir` is the SimpleIR representation — the attr must round-trip through `lower_to_simple`. Verify the attr survives the TIR→SimpleIR conversion by checking that `lower_to_simple.rs` copies function-level attrs.

**Residual risk**: If `lower_to_simple` does not copy function attrs, the guard fails silently. Mitigation: add an explicit attr passthrough in `lower_to_simple`, add a test asserting the attr round-trips.

### R2: Exception-path double-ownership

**Risk**: A value V is dropped on the normal path but not the exception path (or vice versa), leaving a dangling reference or a leak on the exception path.

**Treatment**: The liveness analysis includes exception successor edges. `CheckException` op at position I has two successors: the normal continuation block and the handler block. V is live-in to the handler if V is used in any handler-reachable op. The drop pass inserts DecRef for V on each path where V dies. Validated by the exception-path unit test in Phase 3.

### R3: Overflow-peel fast loop receives spurious DecRef

**Risk**: The overflow-peel fast loop (tir/passes/overflow_peel.rs) emits `CheckedAdd` ops whose results are `RawI64Safe`. The repr filter should prevent any DecRef insertion, but if the filter misclassifies a value, a raw i64 register gets passed to `molt_dec_ref_obj` → type confusion / crash.

**Treatment**: The repr filter is based on `repr_map.get(&val)` (which returns `Some(Repr::RawI64Safe)` for overflow-peel fast-loop accumulators) or `Repr::default_for` for values not in the map. `CheckedAdd` results are promoted to `RawI64Safe` by the representation plan; the filter correctly excludes them. Validated by the `bench_sum` performance smoke test in Phase 3 (zero new ops).

### R4: Generator frame missing inc-ref before yield

**Risk**: A live-across-yield value is not inc-ref'd, so the frame slot holds a stale borrowed reference. On resume the value may have been freed by the caller, causing use-after-free.

**Treatment**: The suspension handling logic (§2.9) explicitly inserts IncRef before each yield for live-across values. Validated by the generator/async unit test in Phase 3 and by the existing compliance tests which include generator parity cases.

### R5: ConstBigInt materialization leaks on multiple calls

**Risk**: `ConstBigInt` materializes a BigInt heap object on each function call. If the function is called in a loop and the constant is dead after each call, each call leaks one BigInt.

**Treatment**: `ConstBigInt` results are classified Owned (§1.4) and the drop pass inserts a DecRef at their last use. This is correct but suboptimal — the ideal is to hoist the constant to an immortal module-level static. The suboptimal-but-correct path is acceptable for Phase 3; the immortal-constant optimization is a follow-up.

### R6: Stack-allocated objects incorrectly dropped

**Risk**: `ObjectNewBoundStack` produces a stack slot (`StackAlloc` variant). If the drop pass misidentifies the repr and inserts a DecRef, the generated code passes a stack address to `molt_dec_ref_obj`, which will interpret it as a NaN-boxed value and either no-op (if the tag check treats the stack address bits as a non-pointer) or crash.

**Treatment**: The drop pass explicitly checks `op.opcode == OpCode::StackAlloc || op.opcode == OpCode::ObjectNewBoundStack` and skips all RC insertion for those values. Additionally, `escape_analysis.rs:680` already removes any IncRef/DecRef on stack-allocated values, and `refcount_elim.rs:126-136` (Step 2a) eliminates any that survive. Triple-redundant defense.

### R7: matches!-oracle silent miscompile for new opcodes

**Risk**: The lessons from `ModuleImportFrom` apply: `effects.rs::opcode_may_throw` and `opcode_is_side_effecting` use `matches!` which defaults to `false` for unlisted opcodes. `DropInsertion` does not add new opcodes, so this risk is zero for the current arc. However, `AnalysisId::Liveness` is a new analysis that must be registered in `AnalysisId::ALL` and handled in `assert_analyses_fresh` to avoid the verification skip.

**Treatment**: Add `AnalysisId::Liveness` to `AnalysisId::ALL` (analysis/mod.rs:89). Add the `Liveness` arm to every `match AnalysisId` in the analysis manager. Build failure if not done (exhaustive match).

---

## 10. Non-Goals and Deferred Items

### 10.1 Reference Cycle Collection

CPython has a cycle garbage collector that handles reference cycles (`a.next = a`). Molt's design is RC-only: values in reference cycles will not be freed by the drop-insertion substrate. This is a known limitation.

**Explicit stance**: Reference cycles are out of scope for this substrate. The common Python patterns that create cycles (closures over `self`, linked lists, graphs) will leak in molt until a cycle-GC substrate is added. This must be documented in the runtime user guide.

**Follow-up pointer**: Design document for the cycle collector is future work (design 21). The drop-insertion substrate is a prerequisite.

### 10.2 Immortal Constant Optimization

`ConstStr`/`ConstBytes`/`ConstBigInt` results are treated as Owned and dropped at last use. The optimization of promoting them to immortal module-level statics (eliminating one alloc+drop pair per call) is deferred to a follow-up.

### 10.3 Perceus Reuse-Token Emission

The `reuse_analysis` pass produces `ReuseCandidate` annotations but does not yet emit runtime reuse tokens. Enabling the actual reuse (eliminating one alloc+free pair per iteration on BigInt accumulator loops) requires adding `molt_reuse_token`/`molt_reuse_alloc` to the runtime ABI and wiring them in the lowering path. This is design 22 (follow-up to this substrate).

### 10.4 InterProcedural RC Analysis

The drop pass operates per-function. Whole-program RC elision (e.g., a function that always returns a unique owned value could transfer ownership to the caller without the caller inc-ref'ing) requires inter-procedural analysis. This is future work after the inliner activation (E1 phase-e).

### 10.5 Incremental Pipeline Integration with run_module_pipeline

The TIR inliner (E1) activates via `run_module_pipeline`, which is currently test-only. When E1 activates production codegen through the inlined `TirModule`, the drop insertion pass must run over the *inlined* function (not the pre-inline stubs). The pass ordering: `run_module_pipeline` → `drop_insertion` → `lower_to_simple`. This ordering is already correct if the drop pass is in `build_default_pipeline` (which runs inside `run_pipeline`, called per-function). No special handling needed — the per-function pipeline runs on whatever TirFunction it receives, whether inlined or not.

---

## Key file anchors

- Runtime alloc birth: `runtime/molt-runtime/src/object/mod.rs:1155,1228`
- Runtime dealloc zero-transition (dealloc counter insertion point): `runtime/molt-runtime/src/object/mod.rs:1812-1821`
- Dealloc counter statics (add alongside): `runtime/molt-runtime/src/constants.rs:88-95`
- IncRef/DecRef opcodes: `runtime/molt-backend/src/tir/ops.rs:123-125`
- refcount_elim pass (existing, to run post-insertion): `runtime/molt-backend/src/tir/passes/refcount_elim.rs`
- effects.rs side-effect oracle (DecRef already listed): `runtime/molt-backend/src/tir/passes/effects.rs:171-172`
- alias_analysis is_rc_barrier: `runtime/molt-backend/src/tir/passes/alias_analysis.rs`
- TIR AnalysisManager (add Liveness): `runtime/molt-backend/src/tir/analysis/mod.rs:66-100`
- Pass pipeline build (add drop_insertion + refcount_elim_post): `runtime/molt-backend/src/tir/pass_manager.rs:282`
- LLVM DecRef lowering (already wired): `runtime/molt-backend/src/llvm_backend/lowering.rs:1275-1287`
- SimpleIR DecRef round-trip (already wired): `runtime/molt-backend/src/tir/lower_to_simple.rs:1903`
- loop_reassign_old_val guard (Phase 3 modification): `runtime/molt-backend/src/native_backend/function_compiler.rs:3577-3628`
- emit_dec_ref_obj (Cranelift inline tag-check, already correct): `runtime/molt-backend/src/native_backend/simple_backend.rs:1076-1103`
- Repr lattice (filter raw scalars): `runtime/molt-backend/src/representation_plan.rs:78-133`
- reuse_analysis (Perceus, follow-up optimization): `runtime/molt-backend/src/tir/passes/reuse_analysis.rs`
