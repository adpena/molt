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
- The suspension opcodes themselves are `is_rc_barrier` (alias_analysis.rs already classifies them as such) and `refcount_heap_exposure_opcodes` in `op_kinds.toml`, consumed by `is_heap_exposing` in `refcount_elim.rs`.

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

**Current invariant (2026-06-18)**: a branch-argument transfer is not only an
immediate successor-entry exclusion. Once an owned root is clean-transferred
into a successor block argument, that block argument remains the release
authority for every reachable descendant block that can still reach a use,
return, release boundary, or onward branch-argument forwarding of that block
argument. The return-edge Python-lifetime cleanup path must use the same
path-aware transfer fact before inserting cleanup split blocks; otherwise a
pre-transfer source root can be released on a branch-to-return path and then the
transferred phi releases the same object at the return boundary. This is pinned
by `typing._load_collections_abc`: the list-comprehension result bound to
`missing` transferred through a block arg, and the old source root was released
on the `if not missing` return path before the `missing` phi release at
`typing.py:605`, causing `invalid object header before dec_ref`. The
implementation records `(source_root, phi, transfer_target)` and computes the
blocks reachable after that specific transfer that can still reach a phi
mention; a shared return target alone is not proof for unrelated non-transfer
edges.

**Current invariant (2026-06-18, Python lifetime authority)**: roots released by
Python lifetime machinery are not generic SSA edge-dying candidates. The
drop-insertion pass treats explicit `DelBoundary`/`DeleteVar`/pre-existing
`DecRef`, statement finalizer release, and `store_var` scope-exit cleanup as a
single `python_release_authority_roots` set. Edge-dying must skip those roots
because a successor-entry drop is path-insensitive, while the Python lifetime
boundary may be path-local or may intentionally run later at return cleanup.
This is pinned by `collections.namedtuple`: `_field_getter` is stored in a local
and used through a `copy_var` alias inside the intrinsic/fallback field loops.
SSA liveness made the alias appear dead at loop exit, but scope cleanup still
owned the function object until `return cls`; dropping at loop exit and again at
return produced `invalid object header before dec_ref` at
`collections/__init__.py:479`.

**Current invariant (2026-06-20, Python local epoch remapping)**: `store_var`
scope cleanup is keyed by the current local epoch, not by every source root that
was ever stored in the local. A source root that clean-transfers into block
arguments must be remapped through that block-arg chain; an explicit cleanup
`DecRef` of the final carrier releases the source epoch. A later same-slot
rebind closes the prior epoch only on paths through the rebind and must remove
the old source root from shared return/cleanup eligibility. Otherwise a local
such as tinygrad's `or_clause` can drop the current cleanup phi and then drop
the stale initial list source root after it has already been released on the
rebind path. Pinned by
`store_var_rebind_epoch_closes_old_scope_cleanup_candidate`.

**Current invariant (2026-06-20, origin carrier liveness)**: a `store_var`
source root that has clean-transferred into a later block-arg carrier remains
released by that carrier wherever the carrier is live, including descendant
return-cleanup blocks that do not pass the source root as a return-block
argument. Return-boundary planning must project transferred-phi liveness through
the Python origin map before deciding to edge-split or return-drop the stale
source root. Otherwise a local such as the tinygrad adapter's parsed `args`
namespace can be released once by the live carrier and again by the original
`parse_args()` source root. Pinned by
`store_var_origin_carrier_live_to_return_cleanup_suppresses_source_release`.

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

**Current invariant (2026-06-20, post-drop exception transfer barriers)**:
post-drop balanced-pair cleanup may remove `IncRef`/`DecRef` pairs only across
ops that execute on the same control-flow path. `Raise`, `CheckException`, and
`TryStart` are RC barriers in `AliasAnalysis::is_rc_barrier`: `Raise` does not
fall through, and `CheckException`/`TryStart` carry implicit handler edges whose
payload retains are consumed only on that exceptional path. In particular,
`IncRef(v); CheckException(v); DecRef(v)` is not a balanced same-path pair: the
handler edge skips the trailing `DecRef` and owns the retained payload until the
handler block releases it. This is pinned by
`post_drop_keeps_check_exception_edge_payload_retain_release`,
`post_drop_keeps_try_start_edge_payload_retain_release`, and
`exception_control_transfer_ops_are_rc_barriers`. `lower_to_simple` must also
materialize handler block-argument stores for both `CheckException` and
`TryStart` before emitting the transfer op; otherwise the retained payload would
survive TIR but disappear before native lowering. This protects
`functools.cached_property.__get__` native lowering from releasing the descriptor
owner while the handler path still needs `self`/`instance`.

**Current invariant (2026-06-20, insertion coverage)**: drop insertion, post-drop
refcount cleanup, and SimpleIR lowering all treat `CheckException` and
`TryStart` through the same exception-transfer-edge authority. The focused proof
chain is `exception_edge_borrowed_payload_retains_for_owned_handler_arg`,
`try_start_edge_borrowed_payload_retains_for_owned_handler_arg`,
`post_drop_keeps_check_exception_edge_payload_retain_release`,
`post_drop_keeps_try_start_edge_payload_retain_release`,
`check_exception_materializes_handler_arg_stores`, and
`try_start_materializes_handler_arg_stores`.

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

**Current invariant (2026-06-18)**: SimpleIR still transports full-RC authority
as a leading `drop_inserted` marker op because `FunctionIR` has no function-level
attrs. Any transform that extracts executable body functions from a drop-inserted
function must preserve that marker on the extracted body functions. Megafunction
splitting is the concrete case: every extracted chunk inherits the original
drop-fact markers so native preanalysis suppresses legacy value tracking while
lowering TIR-inserted `dec_ref` ops. The synthetic split stub is deliberately
not marked because it creates fresh split-frame carrier values after the drop
phase and must remain under normal native cleanup.

> **ACTIVATION FINDING (2026-06-05, RC activation session) — §4.1 understated the native RC overlap; it is an ACTIVATION PREREQUISITE, not a Phase-4 cleanup.** Two things were discovered when DropInsertion was wired into `build_default_pipeline`:
>
> 1. **The activation-blocker abort was a borrow-alias double-drop, NOT carrier resolution.** The lowered loop loads its carried accumulator via `load_var`→`Copy` every iteration; the per-SSA-value drop pass dropped EACH copy of the one live object → refcount underflow → premature free → `invalid object header before dec_ref` UAF at n≥50k. **Fixed**: liveness (`liveness.rs`) and drop placement (`drop_insertion.rs`) now operate in **alias-root space** (a `Copy`/`TypeGuard` borrow alias — §1.2 — shares its root's single ownership obligation; build the union-find via `alias_analysis::build_alias_union_find`). Each heap object is dropped exactly once. A `loop_slot_accumulator_no_double_drop` regression asserts the invariant. Also: the `drop_inserted` marker now round-trips losslessly through `lower_from_simple` (it re-sets the func attr after stripping the transport op — the native module-phase re-lift previously lost it), and DropInsertion is idempotent on a re-lifted function.
>
> 2. **The native backend runs its OWN value-tracking RC that NEGATES the TIR drops on loop-carried accumulators.** `function_compiler.rs` tracks heap results in `tracked_obj_vars` and releases them via `drain_cleanup_tracked_dedup` at exits (Swift-ARC: retain-at-store, release-at-scope-exit), with loop-var `last_use` extended to function-end. For `s = s + "x"` / `total = total + 1`, this tracking keeps the carried object alive so the TIR `DecRef(old)` only brings its refcount 2→1, never to 0 → the **headline leak case (loop accumulators) is NOT closed by activation alone** (measured: string-concat 0/n freed; bigint-accumulator only the 2 dead intermediates/iter freed; O(n) residual RSS). The two ad-hoc loop paths (`loop_reassign_old_val` dec-side, store_var `inc_ref(new)` inc-side) are now guarded by `!drop_inserted`, but the **broad value-tracking system is not** — and gating only those two is insufficient. **Activation prerequisite (Phase 5, expanded):** for `drop_inserted` functions (which never include exception-handler functions — the drop pass bails on those, so the marker is never set there), the entire native value-tracking RC must be suppressed so the TIR drops are the SOLE RC authority: skip heap-result registration into `tracked_*`, skip every `drain_cleanup_tracked_dedup` call (~18 sites at ret/label/check_exception/loop boundaries), and drop the func-end `last_use` extension. This is a multi-site change with real double-free risk and MUST be verified per-site against the corpus under `MOLT_ASSERT_NO_LEAK=1`. Until it lands, the passes stay dormant in `build_default_pipeline` (wiring them ships the O(n) residual leak).

> **ACTIVATION FINDING #2 (2026-06-05/06, Phase-5 native-RC retirement session) — the §4.1 native-RC retirement is DONE and verified; activation is blocked by a DIFFERENT, newly-surfaced drop-pass soundness gap. Passes remain DORMANT.**
>
> **(A) The Phase-5 native-RC retirement (finding #2 above) is COMPLETE and was verified GREEN while the passes were temporarily wired:** the native value-tracking RC is now suppressed for `drop_inserted` functions at its SINGLE source — heap-result *registration* into `tracked_*`/`block_tracked_*`/`entry_vars` is gated on `!drop_inserted` (that one gate makes every `drain_cleanup_tracked_dedup`/entry-drain/ret-cleanup site a no-op, since those lists stay empty) — PLUS four sibling gates the registration gate does NOT cover: the `slot_backed_join_slots` exit-teardown, BOTH func-end `last_use` extensions, the slot-backed join-slot **store** inc/dec (`emit_inc_ref_obj`+`dec_ref` around `stack_store`), the slot-backed join-slot **load** inc-on-read (two sites, `load_var`/`copy_var`), and the SimpleIR `compute_rc_coalesce_skips` (`rc_coalescing`) skip-set (returns empty under `drop_inserted` so the TIR `DecRef`s are not mis-paired and nulled). Search `design 20 §4.1` in `function_compiler.rs`. **Measured WHEN ACTIVE:** memory corpus 4/4 + 4 new per-site (`rc_sites_*`) + 2×30M leak repros all byte-identical, `MOLT_ASSERT_NO_LEAK=1` clean, within RSS caps — string-concat 0→**n** freed, bigint accumulator O(n)→**O(1)** RSS (1M-iter peak 8 MB), 30M-iter repros bounded at 8 MB; peel 9/9 native + 9/9 llvm with the raw lane carrying **zero** RC ops (0 % perf delta on a 30M-iter raw loop, 0.21 s vs 0.21 s); LLVM string/bigint/fib leak-closed too.
>
> **(B) Three drop-pass SOUNDNESS fixes landed (correct, unit-tested, behavior-neutral while dormant):**
> * `terminator_args_to_target` — a value passed as a branch ARG to a successor transfers ownership to that successor's block param and must NOT be edge-dropped at the successor's entry (the prior edge-dying rule double-freed it). Fixed a `while True: break` UAF. Test: `branch_arg_transfer_not_edge_dropped`.
> * `TirFunction::has_state_machine()` — the drop pass now ALSO bails on lowered coroutine `_poll` state machines (`StateSwitch`/`StateTransition`/`StateYield`/`Chan*Yield`/`AllocTask`), not just `StateBlockStart/End`. A generator can lower to a `_poll` body carrying `StateSwitch` without the block delimiters; the re-entrant state dispatch made the dominator-based liveness place a `DecRef` in a resume block BEFORE the value's def (an LLVM-verifier `dec_ref %v` before `%v = …` failure + a native double-free). Test: `state_machine_function_gets_no_drops`.
> * `refcount_elim::run` now honors the `drop_inserted` marker (falls back to the balance-preserving subset, like `run_post_drop`). The native/LLVM module phase RE-RUNS the whole per-function pipeline on already-`drop_inserted` functions (post-inline rebuild / module-slot promotion); on that re-run the FULL `refcount_elim` Step 5/6 was DELETING the lone ownership-release DecRefs (re-opening the leak — this is why LLVM string-concat leaked even though the drop pass had inserted the DecRef). The marker check closes it. (A `loop_carried_phi_dropped_on_backedge` test pins the real-phi loop-accumulator drop shape.)
>
> **(C) HISTORICAL / SUPERSEDED — this 2026-06-06 activation blocker text is preserved as audit trail, not current status.** Later same-file findings corrected the module-store hypothesis, cache-confound analysis superseded the broad stdlib-module-init claim, and the 2026-06-18 Python lifetime authority invariant above is the current drop-insertion rule. A 40-sample random sweep of `tests/differential/basic` on native (passes WIRED) showed **13/40 NEW `invalid object header before dec_ref` UAFs**: `args_kwargs_eval_order`, `bool_short_circuit_order`, `comprehension_nested_lambda_scope`, `nonlocal_and_class_closure`, `class_mro_entries_with_bases`, `method_find_custom_class`, `import_package_init`, `context_return_unwind_scope`, `dict_subclass_slots_weakref_ref`, `recursion_limit`, `call_arity_trampoline`, `async_generator_athrow_after_stop`, `asyncgen_hooks_api`. Minimal repro: a **module-global** list + a function that reads it (`log = []`; `def side(t,v): log.append(t); return v`; `side("a",False) and side("b",True)`) → UAF. Root cause class: the drop pass treats values whose single owning reference is held by a longer-lived container (the **module dict** for a global binding; likely also cell/closure storage) as ordinary dead temps and drops them, freeing an object another function still reaches via the global/cell — the drop is on a path where the object is NOT actually dead. The drop pass's ownership-transfer set is INCOMPLETE: it covers Return values and branch args, but NOT global/cell stores. **NOTE:** this is NOT the §1.2 "ModuleSetAttr/ModuleCacheSet = borrowed, caller drops at last use" case as written — the UAF means either that convention is violated by the runtime/codegen (the global store does not actually inc-ref, so the function's drop is the last ref) or the module-dict binding IS the sole owner (so the function must transfer, not drop). RESOLVING THIS REQUIRES auditing the `molt_module_cache_set` / global-load refcount contract end-to-end (drop pass ⨯ runtime), not a localized drop-pass tweak. **Until the drop pass is sound across the full `tests/differential/basic` corpus (native AND llvm) under `MOLT_ASSERT_NO_LEAK=1` with ZERO new UAFs, the two passes stay OUT of `build_default_pipeline`.** Everything in (A)/(B) is already on `main` behind the `drop_inserted` marker (inert while dormant), so activation is a 2-line pipeline append + restoring the +2 pinned-pass-name entries and the `stats.len()` 28→30 assertion.

> **ACTIVATION FINDING #3 (2026-06-06, ownership-audit + arg-cleanup session) — Finding #2(C)'s root-cause hypothesis is CORRECTED by a full runtime audit; ONE more native-RC gap was found and FIXED (inert behind the marker); the remaining blocker is now NARROWED to a drop-induced native-codegen bug in the closure-CALL path. Passes remain DORMANT.**
>
> **(A) The §1.2 store/load ownership table is CORRECT — the runtime was audited end-to-end and matches.** Finding #2(C) speculated the global-store convention might be violated. It is NOT. Verified against the runtime source (file:line):
> | op | runtime fn | contract | evidence |
> |---|---|---|---|
> | global-binding store (`module.x = v`) → `ModuleSetAttr` | `molt_module_set_attr` → `dict_set_in_place` | **Borrowed (container inc-refs)** | `object/ops.rs:9246,9273` inc_ref the heap value; `builtins/modules.rs:5126` |
> | module-object cache (`sys.modules`) → `ModuleCacheSet` | `molt_module_cache_set` | **Borrowed (inc-refs)** | `builtins/modules.rs:4302` `inc_ref_bits(module_bits)` |
> | dict/list element store → `StoreIndex`/`StoreAttr` | `dict_set_in_place` | **Borrowed (inc-refs)** | `object/ops.rs:9246,9273` |
> | closure cell store → `ClosureStore` | `molt_closure_store` | **Borrowed (inc-refs)** | `builtins/functions.rs:5324` |
> | function closure store | `function_set_closure_bits` | **Borrowed (inc-refs)** | `object/layout.rs:841` |
> | global LOAD (`x` in a function) → `ModuleGetName`/`ModuleGetGlobal` | `molt_module_get_attr` / `molt_module_get_global` | **Owned (+1)** | `builtins/modules.rs:4832,4863` `inc_ref_bits(val)` before return |
> | closure cell LOAD → `ClosureLoad` | `molt_closure_load` | **Owned (+1)** | `builtins/functions.rs:5305` |
> | subscript → `Index` (list/tuple/dict element) | `molt_index` | **Owned (+1)** | `object/ops.rs:4019` (list), `:3563` (dict) |
> So the drop pass IS correct to drop the local ref after a borrow-store, and to drop a loaded-owned value at last use. The §1.2 table needs no change. The module-global minimal repro (`log = []` + `def side`) NO LONGER UAFs after (B); several of the original 13 (`bool_short_circuit_order`, `args_kwargs_eval_order`, `comprehension_nested_lambda_scope`, `import_package_init`, `class_mro_entries_with_bases`, `call_arity_trampoline`, `context_return_unwind_scope`) now PASS byte-identical with the passes wired.
>
> **(B) FIXED — the SECOND un-gated native value-tracking RC source: per-call-site dead-argument release (`arg_cleanup`).** Finding #2(A) claimed the heap-result *registration* gate (`function_compiler.rs:24441`, `!drop_inserted`) is the SINGLE source feeding every drain site. That is true for the `tracked_*`/`block_tracked_*` lists — but the `"call"` op handler ALSO computes a SEPARATE `arg_cleanup` set DIRECTLY from the SimpleIR `last_use` map (`function_compiler.rs` ~line 15414), NOT from the tracked lists, and `local_dec_ref_obj`s every call argument that dies at its call. With the TIR drop pass active this DOUBLE-FREES every dead call arg (the TIR pass already emits `DecRef(arg)` after the call) → the exact heap-layout-dependent `invalid object header before dec_ref` / refcount-underflow abort. **Fix (this session): gate the `arg_cleanup` population on `!drop_inserted`** so for drop-inserted functions the TIR `DecRef`s are the sole authority (empty `arg_cleanup` → emit-loop no-op, root-filtered retains become identity, `already_decrefed` un-polluted). Verified WHEN ACTIVE: `recursion_limit` (-6 abort → exit 0 byte-identical), `method_find_custom_class` (intermittent -6 → deterministic pass), and the whole call-arg-double-free class fixed. The OTHER call handlers were audited: `"call_internal"`/`"call_guarded"`/`"call_func"`/`"call_method"` have NO un-gated `arg_cleanup` (only the `tracked_*` drains, already gated). This is a complete sub-piece of the Phase-5 native-RC retirement: when shared DropInsertion marks a function with `drop_inserted`, this native call-argument lane is suppressed and the TIR `DecRef`s are the release authority.
>
> **(C) THE REMAINING BLOCKER — a DROP-INDUCED native-codegen value-confusion in the closure-CALL path.** After (B), the residual native-corpus failures are closure / `nonlocal` shapes (e.g. `nonlocal_and_class_closure`) that fail as `TypeError: 'function' object is not subscriptable` (exit 1, a caught Python error, NOT an abort). **Tight isolation (verified):** `def f(): x=1; def inner(): return x; return inner()` (READ-ONLY capture, CALLED) FAILS with drops active but PASSES byte-identical on the dormant `main` backend → a genuine drop-INDUCED regression, not pre-existing. `def f(): x=1; def inner(): nonlocal x; x=5; return x` (closure created but NOT called) PASSES. So the trigger is **closure-create + closure-CALL + drop insertion**, independent of `nonlocal`/cell-write and of whether `x` is returned (a variant returning a constant after the call STILL fails). **Runtime ground truth** (`MOLT_DEBUG_SUBSCRIPT=1` + `MOLT_DEBUG_DECREF_ZERO=1` on the active binary): the indexed object is a LIVE function (`type_id=221`, freed only AFTER the subscript) — so this is a VALUE confusion, NOT a use-after-free: the SSA value that should hold the closure env tuple/cell holds the inner FUNCTION object at the `Index` site. **Drop-pass trace** (alias-root + producer, for `nlb__nonlocal_basic`): every inserted DecRef is individually RC-balanced — `DecRef(cell=list_new)`, `DecRef(closure_tuple=tuple_new)`, `DecRef(function=func_new_closure)` each exactly once; `func_new_closure` classifies as `FreshValue` (its result is its own alias root, correctly droppable). So the bug is NOT an RC imbalance in the drop pass; it is the inserted `DecRef`/`IncRef` ops perturbing the native backend's Cranelift variable/slot management for the closure-call's env-extraction (`call_guarded` reads `molt_function_closure_bits(func_obj)` at `function_compiler.rs:16289-16300`, prepends it as arg 0; `inner` then does `Index(closure, 0)`), such that `inner` receives the function object instead of its closure tuple. **NEXT (de-risked):** re-wire the two passes, build the read-only-capture-called repro, dump CLIF (`MOLT_DUMP_CLIF=1 MOLT_DUMP_CLIF_FUNC=...`) for `__inner` and the creating fn WITH drops vs the dormant CLIF, and find where the env-extraction operand or the function-object variable diverges — the fix is almost certainly a native `call_guarded`/closure-env slot-reuse guard that must respect the inserted RC ops (mirror the `!drop_inserted` slot-store/slot-load gates already in `function_compiler.rs`), NOT the (proven-balanced) drop pass.
>
> **WORKFLOW LESSONS (cost most of this session):** (1) the per-session backend daemon caches the compiled binary in memory — after EVERY `cargo build` you MUST `kill` the `target/sessions/<id>/release-fast/molt-backend --daemon` PID (verify it is YOUR session and not `codex`) so the next `molt build` reloads the new binary. (2) `cargo build` from the worktree with a RELATIVE `CARGO_TARGET_DIR` writes to the WORKTREE's `target/`, but `python3 -m molt` is editable-installed from the MAIN repo and reads `/Users/adpena/Projects/molt/target/sessions/<id>/` — build with an ABSOLUTE `CARGO_TARGET_DIR=/Users/adpena/Projects/molt/target/sessions/<id>` (+ `--manifest-path <worktree>/Cargo.toml`). (3) custom diagnostic env vars do NOT reach the codegen worker unless added to the CLI `_BACKEND_REQUEST_ENV_KNOBS` allow-list (`src/molt/cli.py:174`); use a sentinel FILE (`/tmp/...`) inside the pass, or an already-allow-listed var (`MOLT_DUMP_IR`, `MOLT_DEBUG_ARTIFACT_DIR`, `MOLT_DEBUG_SUBSCRIPT`, `MOLT_DEBUG_DECREF_ZERO`). Activation stays a 2-line `build_default_pipeline` append + the +2 pinned-pass-name entries + `stats.len()` 28→30 once (C) is fixed and the full corpus is clean (native AND llvm) under `MOLT_ASSERT_NO_LEAK=1`.

> **ACTIVATION FINDING #4 (2026-06-06, drop-induced call-lowering session) — HISTORICAL / SUPERSEDED.** Finding #5 below determined that the apparent third stdlib-module-init miscompile was the stale stdlib-cache confound, not a current compiler bug. Keep this block only as provenance for the two fixed bugs and the invalidated hypothesis. This session's wired-corpus delta on `tests/differential/basic`: finding-3C 13-case set went **9/13 → 10/13** with the two fixes below; `m2_modlevel.py` (the minimal `class C: def f(self): return 5; print(C().f())` method call) went **abort → pass**; `import contextlib` went **TypeError → pass**.
>
> **(A) FIXED — finding #3C's headline `invalid object header before dec_ref` on METHOD calls was a CallArgs-builder DOUBLE-FREE, NOT a closure-env confusion.** Root cause, CLIF-confirmed: the un-fused `obj.method(args)` idiom lowers (in `lower_to_simple`) to `b = callargs_new; r = call_bind(callee, b)`. `molt_call_bind_ic` (`call/bind.rs:3537`, via `PtrDropGuard::new(builder_ptr)`) **frees `b` internally**, regardless of normal/exception return. The TIR drop pass treated `b` as an ordinary dead temp (its last use is the `call_bind`) and inserted `DecRef(b)` after the call → a SECOND free of the `TYPE_ID_CALLARGS` object (`type_id=236`) → the abort. The runtime trace was unambiguous: `dec_ref_zero ptr=X type_id=236` (the call_bind's internal free) immediately followed by `invalid object header before dec_ref ptr=X` (the inserted DecRef hitting freed memory). **Why it surfaced now (and only via drops):** the inserted `DecRef(b)` is a SECOND read of `b`, which defeats `fuse_method_dispatch`'s single-use gate (`passes.rs:1660`, `fuse_count_value_reads(b)!=1`), so the fused alloc-free `call_method_ic` (which never materialises a builder) is NOT taken → the un-fused `call_bind` path runs WITH the double-free. **Fix (`tir/passes/drop_insertion.rs` `op_consumed_operand_root`):** a value whose last use is the CONSUMED callargs operand of `call_bind`/`call_indirect` (operand index 1, the last operand — matching the `molt_call_bind_ic(site, callee, builder)` ABI) transfers ownership to the op exactly like a `Return` value — no trailing `DecRef`. This is the §1.2 "Operands: takes-ownership" row, which the drop pass was missing (it only modeled Return/branch-arg transfer). `call_func`/`call_guarded`/`call`/`call_internal` build+consume their OWN builder internally (no pre-built builder operand) so they consume none of their TIR operands — verified. Regression test: `call_bind_callargs_operand_not_dropped`.
>
> **(B) FIXED — the `'object' object is not subscriptable` closure failures (`nonlocal_and_class_closure`, `import contextlib`) were a CROSS-BATCH closure-metadata REPLACE bug, exposed (not caused) by drops.** The native backend's per-call-site closure-env extraction (`function_compiler.rs` `call_guarded`/`call`/`call_internal`, "extract env from function object" → `molt_function_closure_bits(func_obj)` prepended as arg 0) fires only when `effective_closure_functions.contains(target)`. That set was computed by REPLACING the batch's local `func_new_closure` scan with `module_context.closure_functions` whenever a module context was present (`simple_backend.rs:3170`). But the module context is built ONCE per compilation unit — for the stdlib cache it is built from the STDLIB functions only (`main.rs:347` `stdlib_module_context`), so it does NOT contain a user/other-module closure. When a batch that DEFINES such a closure was compiled with that context set, the replace DROPPED the closure from the set → the call site skipped env extraction → the closure received a garbage/zero arg where its cell tuple belonged → `index(closure, 0)` raised. **Drops merely shifted function sizes enough to change which batch the code landed in / whether a module context was active** (dormant `import contextlib` lands the init in a `module_context=false` batch → local scan → works; wired it lands in a `module_context=true` batch → stdlib-only set → fails). `merge_function_arities`/`merge_function_has_ret` already UNION'd their maps for exactly this reason; the closure/task/leaf/return-alias sets did NOT — an asymmetry. **Fix (`simple_backend.rs` `merge_closure_functions`/`merge_task_kinds`/`merge_task_closure_sizes`/`merge_leaf_functions` + return-alias union):** every `effective_*` is now `module_context ∪ local_scan` (local wins on overlap). INERT for the existing dormant batching (the module context is already a superset there), so production-safe; it CLOSES the latent cross-batch closure bug regardless of drop activation. Regression test: `effective_metadata_unions_module_context_with_local_scan`. Verified: `import contextlib`/`m2`/the finding-3C closure shapes pass wired; `import asyncio`'s `nonlocal_basic` env extraction now resolves (`molt_function_closure_bits` returns the real `type_id=221` function).
>
> **(C) THE REMAINING BLOCKER — a SYSTEMIC, batch/context-sensitive drop-induced miscompile in stdlib MODULE-INIT.** With (A)+(B) wired, simple user programs (closures, classes, methods, decorators, properties, metaclasses, descriptors, `super().__new__`, `*args` wrappers, returned-and-called closures) ALL pass; but importing several real stdlib modules — `typing`, `warnings`, `re`, `collections`, `dataclasses`, `enum`, `string` (and therefore `asyncio`) — fails at IMPORT (module-init) time with the same `'object' object is not subscriptable`, key `0` (a closure cell-chain `index(x, 0)` reading an `object` whose `type_id` is GARBAGE and CHANGES per run = uninitialised/freed memory). `import contextlib`/`abc`/`_collections_abc`/`types`/`operator`/`keyword` PASS. **All of `typing`/`re`/`os`/`sys`/`_collections_abc`/`warnings` run their module-init through the drop pass (the stdlib-cache build runs the full TIR pipeline even with `skip_ir_passes=true`; ~893 functions).** **DISPROVEN hypotheses (do not re-chase):** NOT the CallArgs double-free (A fixed it, no abort here — this is a caught TypeError); NOT closure-env extraction (a `call_guarded` divergence audit on the failing `import string` showed ZERO sites with `in_closure_fns=true && in_local_envs=false`); NOT object stack-promotion (`object_new_bound_stack`→heap downgrade does NOT fix it); NOT the TIR inliner (historical inliner-disabled repro before rollback removal did not fix it); NOT a metadata REPLACE bug (all 7 `NativeBackendModuleContext` fields are now UNION'd). **Decisive evidence it is SYSTEMIC, not one bad function:** a clean drop-skip bisect over the full 893-function set — skipping the alphabetical FIRST half AND skipping the SECOND half BOTH still fail; no single function skip fixes `import string`, but skipping a whole MODULE prefix (`typing__`/`warnings__`) does. That signature = the drops change something GLOBAL (batch boundaries / leaf set / a cross-function invariant) so functions whose OWN drops are correct get miscompiled by a context shift. **NEXT (de-risked):** the bisect tooling is a 2-line revert away (a `/tmp/.../drop_skip.txt` substring-skip + a `drop_funcs.txt` dump at the top of `drop_insertion::run`). Reproduce reliably with `import warnings` or `import typing` ALONE (smallest failing units); ALWAYS `find ~/Library/Caches/molt -maxdepth 1 -name 'stdlib_shared_*' -delete` + kill the daemon before each build (the stdlib SHARED-OBJECT cache is NOT keyed on the backend binary and silently served stale drops — a major confound that cost much of this session; the per-function TIR cache `.molt_cache/<exe-mtime>/` IS keyed on the exe so it self-invalidates on rebuild). Then: dump the FAILING module-init function's final SimpleIR + post-opt CLIF WITH drops vs the dormant CLIF, and find the value-confusion — the prime suspect is a drop interacting with how a module-init closure cell (`list_new` cell → `tuple_new` closure → `func_new_closure`) is threaded across a megafunction-split chunk boundary or a batch boundary, such that a still-referenced cell/closure is stack-promoted or its slot reused. Until `import typing`/`warnings`/`re`/`collections`/`asyncio` are clean wired under `MOLT_ASSERT_NO_LEAK=1` (native AND llvm) across the full `tests/differential/basic` corpus, the two passes stay OUT of `build_default_pipeline`. Everything in (A)/(B) is on `main` behind the `drop_inserted` marker (inert while dormant); activation remains the 2-line pipeline append + the +2 pinned-pass-name entries + the `stats.len()` 28→30 assertion once (C) is fixed.

> **ACTIVATION FINDING #5 (2026-06-06, cache-confound-elimination session) — Finding #4(C)'s "SYSTEMIC stdlib-module-init miscompile" was 100% THE STALE-STDLIB-CACHE CONFOUND. It did NOT survive the cache fix. With trustworthy binary-keyed builds, `import typing`/`re`/`collections`/`warnings` ALL build+run byte-identical to CPython on native WIRED. NO real Finding-#4(C) miscompile exists.** Finding #4(C) itself flagged the confound ("the stdlib SHARED-OBJECT cache is NOT keyed on the backend binary and silently served stale drops — a major confound that cost much of this session") but treated it as a workflow nuisance to be hand-managed (`find ... -delete` + kill daemon before each build) rather than the *root cause* of the observed failures. It was the root cause.
>
> **(A) THE CONFOUND IS FIXED STRUCTURALLY (commit `fdbb51329`, this session's Piece 1 — independently valuable, committed alone).** The `stdlib_shared_<key>.o` cache filename (and the module/per-function `.o` keys that share `_build_cache_variant`) was derived from the backend *source-tree* fingerprint (`_cache_fingerprint`, `cli.py:27702`) but NOT the backend *binary* identity — by explicit design comment (`cli.py:27706`: "we intentionally do NOT hash the backend binary itself"). Exact-key immutable cache paths now replace the former mtime invalidation guard: `_validate_shared_stdlib_cache_contract` verifies sidecars/manifests without deleting artifacts on hot paths, so git/worktree mtime resets, concurrent-session target dirs, and A/B codegen toggles cannot create a second cache authority. **Fix:** bind the cache variant to the backend binary identity (`_backend_binary_identity` = `resolved_path|mtime_ns|size`, a `missing:` fail-safe sentinel when unbuilt), mirroring the exact convention the per-function TIR cache already uses (`tir/cache.rs:448` `backend_cache_dir_for` salts its namespace with exe path+mtime) and the intrinsic-symbol sidecar (size+mtime). New single-source-of-truth `_backend_features_for_target`/`_backend_features_for_build_target` (deletes the duplicated target->feature `if/elif/else` at the build-dispatch site) so the identity stamps the EXACT binary the daemon runs for this target/profile/llvm-feature. The CLI already exports the computed path+key to the daemon via `MOLT_STDLIB_OBJ`/`MOLT_STDLIB_CACHE_KEY`, so a binary change yields a NEW `stdlib_shared_<key>.o` path → daemon sees `!exists()` → recompiles with the new binary; the stale `.o` is orphaned, never linked. 4 new tests in `tests/cli/test_cli_shared_stdlib_cache.py` (18/18 in-file pass). VERIFIED: the daemon log shows fresh `first build — caching 885/1208 stdlib functions to stdlib_shared_<newhash>.o` after a drops-enabled rebuild — the stale `.o` is no longer served.
>
> **(B) WITH THE CONFOUND DEAD, RE-ESTABLISHED THE FAILING SET CLEANLY — IT WAS EMPTY (of real failures).** Native gate flipped ON locally (`target_uses_tir_drop_insertion` → `true`, the ONLY change activation needs — the restack `f2b2d1b32` already did the pipeline append + pinned-name entries + `stats.len()==30`). Then, on a clean drops-wired build:
> * **Finding #4(C)'s headline cases ALL PASS:** `import typing`/`re`/`collections`/`warnings` → byte-identical CPython, exit 0 (the `'object' object is not subscriptable` garbage-`type_id` error is GONE — it was the stale `.o`).
> * **Drops provably FIRE and CLOSE THE LEAK** (proof they are not silently skipped): `bigint_accumulator` n=1000 → `dealloc_bigint=3001` (every per-iter BigInt freed), `peak_rss=8 MB` (was the 297 MB leak). **30M-iter** bigint → `alloc=60000636 dealloc=60000007 dealloc_bigint=60000001`, `peak_rss=8 MB`, 3.35 s, exit 0 under `--rss-mb 64` + `MOLT_ASSERT_NO_LEAK=1`. `string_concat` → `dealloc_string=10001`, RSS-bounded (the 30M form is O(n²)-SLOW but RSS stays 14 MB — bounded, NOT leaking; not a drop issue). Memory corpus **14/14** (incl. every adversarial over-release regression: `alias_reassign_bigint`/`slice`/`conditional_del`, `generator_consumer_*`, `rc_sites_*`) under `--rss-mb 64`+`MOLT_ASSERT_NO_LEAK=1`.
> * **Compliance `pytest -n 4`: 46/46** byte-identical (curated CPython-semantic parity).
> * **Peel 9/9 native + 9/9 LLVM** (raw lane carries zero RC ops). **nested_try_handler_reraise + inline_llvm_module_phase_activation byte-identical on LLVM.** LLVM leak repros (bigint/string) leak-closed too. **Backend lib 1109 + runtime lib 628, 0 fail, 0 warnings.**
> * **ZERO drop-induced regressions** across the basic corpus files tested (≈180 unique, multiple partial sweeps). EVERY failure observed was verified PRE-EXISTING by building+running the SAME file on the DORMANT binary: the `async_*` cluster (~44 files — the drop pass correctly BAILS on `has_state_machine`/`has_exception_handlers`, so async coroutine/event-loop leaks+errors are identical wired vs dormant; e.g. `async_comprehensions` OOMs 137 BOTH ways), `attr_security` (a pre-existing `refcount underflow before dec_ref type_id=0` in the descriptor/`__getattr__` exception path — IDENTICAL wired+dormant, NOT drop-induced), `builtins_symbol_*` (pre-existing `_hashlib` chunk link failure + marked `# MOLT_META: expect_fail=molt` → xfail), `dict_subclass_slots_weakref_ref` (pre-existing dict-subclass-`__slots__` frontend layout-size assert), exec/eval (xfail by design).
>
> **(C) WHAT IS NOT DONE — the ONE remaining activation gate: a single CLEAN full ~796-file `tests/differential/basic` sweep (wired no-worse vs dormant) + its formal dormant diff. Blocked by TWO environment/tooling confounds (NOT drop-pass issues), both of which the next session MUST defeat before trusting a corpus result:**
>
> **Confound 1 — SIGURG process-kill.** The molt daemon's tokio runtime delivers `SIGURG` that hard-kills any long-running multi-build harness: `molt_diff.py` dies (~file 25-200), standalone Python scripts die even with `signal(SIGURG, SIG_IGN)` + `nohup`, bash task-wrappers die even with `trap '' URG`, and even a `pytest -n 6` process that SURVIVES SIGURG for ~590/796 files eventually dies before writing its `--junitxml`. Only (i) SHORT bash loops (<~3 min — the 14-file memory corpus + 9-file peel completed), (ii) SHORT `pytest -n` runs (compliance's 46 tests / 114 s completed), and (iii) single `molt build` commands reliably survive.
>
> **Confound 2 (NEWLY FOUND, the more dangerous one) — per-file BUILD-CONTEXT DRIFT poisons mass comparison.** Binaries built across a long mass run do NOT have a stable feature surface: e.g. `bytes_codec` (imports `molt_msgpack`/`molt_cbor`) built by the pytest mass-run raised `NotImplementedError: msgpack requires stdlib_serialization feature` and was flagged a wired FAIL, while a FRESH `molt build` of the same file (identical flags) matches CPython (exit 1) on BOTH wired AND dormant — the mass-built binary was byte-DIFFERENT from the fresh one (`stdlib_serialization` feature state differed). So a "wired-fail / dormant-pass" delta computed from mass-built binaries is a FALSE REGRESSION. **`bytes_codec` is NOT a drop regression** — it was the only "regression" the mass triage surfaced, and it evaporated under fresh identical builds. LESSON: the corpus gate MUST build each file FRESH with identical flags on both wired and dormant in the SAME run, and compare per-file; never compare mass-built binaries from different runs/contexts.
>
> **What WAS reliably established (fresh, identical-flag, single-file builds — the trustworthy method):** the 2 corpus UAFs are BOTH pre-existing — `attr_security` (`refcount underflow before dec_ref type_id=0`, descriptor/`__getattr__` exception path) and `memoryview_format_codes` (`invalid object header before dec_ref type_id=1599684946`, memoryview lifetime) fail IDENTICALLY wired+dormant on fresh builds. Plus the ~30-40 fresh single-file triages in (B) (async cluster, dict_subclass, builtin_numeric_ops, descriptor_delete, container_mutation, …) were all pre-existing, and `bytes_codec` is a non-regression. ZERO drop-induced regressions found by any TRUSTWORTHY (fresh-build) comparison.
>
> **Current activation status (2026-06-20): NativeCranelift now participates in
> `target_uses_tir_drop_insertion` with LLVM/WASM/Luau.** The old false-to-true
> activation flip is closed; the remaining gate is broader native legacy-RC
> deletion and full ownership-surface proof. Any deletion of native automatic
> temp-RC/value-tracking lanes still needs a fresh corpus harness that builds
> each file with identical flags, applies the xfail overlay, survives SIGURG via
> short batches or forked tests, and emits zero wired-fail/dormant-pass deltas
> plus zero new `invalid object header`/`refcount underflow` UAFs under
> `MOLT_ASSERT_NO_LEAK=1`.

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

- `runtime/molt-tir/src/tir/analysis/mod.rs` — add `AnalysisId::Liveness` to the enum and `ALL` array. Implement `Analysis for TirLiveness`.
- `runtime/molt-tir/src/tir/passes/liveness.rs` (new file) — implement `TirLiveness`:
  - Struct fields: `pub live_in: HashMap<BlockId, HashSet<ValueId>>`, `pub live_out: HashMap<BlockId, HashSet<ValueId>>`.
  - Compute using backward dataflow: iterate until fixpoint; seed with `LiveOut[exits] = {}`.
  - Exclude `Repr::RawI64Safe`/`Bool`/`FloatUnboxed` values from the live sets.
  - `CFG_SENSITIVE`: `true`.
  - Public query: `fn is_live_in(&self, block: BlockId, val: ValueId) -> bool`.
  - Public query: `fn last_use_in_block(&self, block: &TirBlock, val: ValueId) -> Option<usize>` — returns the index of the last op that uses `val` in this block (or `None` if not used in this block at all).
- `runtime/molt-tir/src/tir/passes/mod.rs` — add `pub mod liveness;`.

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

- `runtime/molt-tir/src/tir/passes/drop_insertion.rs` (new file):
  - `pub fn run(func: &mut TirFunction, am: &mut AnalysisManager, repr_map: Option<&HashMap<ValueId, Repr>>) -> PassStats`
  - Consumes: `TirLiveness`, `ImmediateDoms`, `PredMap`, `LoopForest`, `AliasAnalysis` (from am), repr_map parameter.
  - Straight-line placement: for each block, walk ops, identify last-use positions, insert `DecRef` after last use.
  - Successor-edge placement: for each block-exit edge where a value V is live-in to the predecessor but not live-in to the target successor AND V is not passed as a branch argument to that successor — insert `DecRef(V)` at the end of the current block before the terminator. When the CondBranch has two successors with different dead-value sets, use the "before-the-terminator" insertion for values that die on ALL successors (common-prefix), and for values that die only on one successor, insert after the terminator switch by placing them at the start of the successor block (this keeps the pass OpsOnly — no edge-splitting).
  - Loop-exit placement: detect loop exit edges using `LoopForest`. For phi values that are the back-edge carrier (last live use at the back-edge branch), insert `DecRef` before the loop-exit branch.
  - Suspension handling: for each `StateYield`/`ChanSendYield`/`ChanRecvYield`/`Yield`/`YieldFrom` op, for each value that is live-across-this-yield (in `LiveIn` of the resume continuation block), insert `IncRef(V)` immediately before the yield op.
  - Stack filter: values produced by `StackAlloc` or `ObjectNewBoundStack` — never insert DecRef.
  - Borrow inference: if V's only remaining use after the drop candidate is as an operand to a `Call`/`CallMethod`/`CallBuiltin` where V is dead after the call, and no IncRef is needed (no heap-exposing barrier between definition and call), skip the IncRef+DecRef pair entirely.
  - Set `func.attrs.insert("drop_inserted", AttrValue::Bool(true))` for every non-bailed full-function analysis, even when no physical `DecRef`/`IncRef` is inserted; report this as `PassStats.attrs_changed` so pass-manager snapshot restore preserves metadata-only RC authority changes.
  - Mutation class: `Cfg`, because mixed-ownership phi retains may split critical edges.
- `runtime/molt-tir/src/tir/passes/mod.rs` — add `pub mod drop_insertion;`.
- `runtime/molt-tir/src/tir/pass_manager.rs` — add two new pass slots at the end of `build_default_pipeline`:
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

- `runtime/molt-tir/src/tir/lower_to_wasm.rs` — add handling for `OpCode::DecRef`: emit a call to `molt_dec_ref_obj` (the WASM import already exists for it). Add `OpCode::IncRef`: emit `molt_inc_ref_obj`. The LIR path already carries TIR ops in `LirOp::tir_op`; the WASM lowering function must pattern-match them.
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
| `runtime/molt-tir/src/tir/passes/liveness.rs` | Create | `TirLiveness` analysis — backward dataflow liveness |
| `runtime/molt-tir/src/tir/analysis/mod.rs` | Modify | Add `AnalysisId::Liveness`, register `TirLiveness` |
| `runtime/molt-tir/src/tir/passes/drop_insertion.rs` | Create | `DropInsertion` pass — core of this substrate |
| `runtime/molt-tir/src/tir/passes/mod.rs` | Modify | Add `pub mod liveness; pub mod drop_insertion;` |
| `runtime/molt-tir/src/tir/pass_manager.rs` | Modify | Append `drop_insertion` + `refcount_elim_post` to `build_default_pipeline` |
| `runtime/molt-tir/src/tir/lower_to_wasm.rs` | Modify | Wire `OpCode::DecRef`/`IncRef` |
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
- IncRef/DecRef opcodes: `runtime/molt-tir/src/tir/ops.rs:123-125`
- refcount_elim pass (existing, to run post-insertion): `runtime/molt-tir/src/tir/passes/refcount_elim.rs`
- effects.rs side-effect oracle (DecRef already listed): `runtime/molt-tir/src/tir/passes/effects.rs:171-172`
- alias_analysis is_rc_barrier: `runtime/molt-tir/src/tir/passes/alias_analysis.rs`
- TIR AnalysisManager (add Liveness): `runtime/molt-tir/src/tir/analysis/mod.rs:66-100`
- Pass pipeline build (add drop_insertion + refcount_elim_post): `runtime/molt-tir/src/tir/pass_manager.rs:282`
- LLVM DecRef lowering (already wired): `runtime/molt-backend/src/llvm_backend/lowering.rs:1275-1287`
- SimpleIR DecRef round-trip (already wired): `runtime/molt-tir/src/tir/lower_to_simple.rs:1903`
- loop_reassign_old_val guard (Phase 3 modification): `runtime/molt-backend/src/native_backend/function_compiler.rs:3577-3628`
- emit_dec_ref_obj (Cranelift inline tag-check, already correct): `runtime/molt-backend/src/native_backend/simple_backend.rs:1076-1103`
- Repr lattice (filter raw scalars): `runtime/molt-backend/src/representation_plan.rs:78-133`
- reuse_analysis (Perceus, follow-up optimization): `runtime/molt-tir/src/tir/passes/reuse_analysis.rs`
