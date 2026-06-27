<!-- Foundation plan 04b. Architect: read-only agent, 2026-06-06, verified against the
current tree (post fae639e94 counted-loop contract, post S6 SCEV, post E1 activation,
post overflow_peel). Supersedes the stale Arc-1 gate-flip framing of 04_L4-loops.md.
Saved verbatim from the architect's final report per the full-text-artifact policy. -->

# molt L4 Loop-Optimization Arc — Implementation-Ready Plan

## Section 1: Current-State Inventory

### 1.1 Complete Loop-Pass Inventory

The pipeline runs 30 passes in this order (verified against `runtime/molt-passes/src/tir/passes/mod.rs`:133-170 and `pass_manager.rs`:316-475):

| Position | Pass | File | Loop relevance | Fires today? |
|----------|------|------|----------------|--------------|
| 1 | `range_devirt` | `passes/range_devirt.rs` | Devirtualizes `range()` iterator loops. Matches `IterNextUnboxed` + `GetIter` + `CallBuiltin("range")` chain | NEVER on real code (see §1.2) |
| 2 | `iter_devirt` | `passes/iter_devirt.rs` | Devirtualizes list/iterator loops to index-based loops | Fires on list-iteration shapes |
| 3 | `tuple_scalarize` | `passes/deforestation.rs` | No loop role | — |
| 4 | `loop_unroll` | `passes/loop_unroll.rs` | Fully unrolls constant-trip counted loops. Uses `counted_loop::recognize_counted_loop` | FIRES on small constant-trip loops (verified `fae639e94`) |
| 5-8 | `canonicalize` (×2), `unboxing` | — | No loop role | — |
| 9 | `block_versioning` | `passes/block_versioning.rs` | Specializes blocks with `TypeGuard` ops | NEVER — no `TypeGuard` producer exists (see §1.3) |
| 10 | `gvn` | `passes/gvn.rs` | Cross-block CSE | Fires on invariant loop expressions when dominator structure present |
| 11 | `licm` | `passes/licm.rs` | Loop-invariant code motion | FIRES — does NOT bail on `has_exception_handling`, per-op `is_hoistable` predicate (see §1.4) |
| 18 | `type_guard_hoist` | `passes/type_guard_hoist.rs` | Hoists `TypeGuard` ops to preheaders | NEVER — no `TypeGuard` producer (see §1.3). ALSO bails at line 90 on `has_exception_handling` |
| 25 | `check_exception_elim` | `passes/check_exception_elim.rs` | Eliminates redundant `CheckException` ops | Fires on observation-only functions |
| 26 | `overflow_peel` | `passes/overflow_peel.rs` | Dual-loop peel for unbounded int accumulators. Gates on `has_exception_handlers()` | FIRES — landed `e267a4f5a`, 14× faster than CPython on `bench_sum` |
| 19 | `strength_reduction` | `passes/strength_reduction.rs` | `x*2^k → x<<k`, `x**2 → x*x`. FloorDiv/Mod deferred | Fires on power-of-two mul/pow patterns |
| 28 | `dce` | `passes/dce.rs` | Dead code elimination, block removal | Fires after unroll removes loop region |
| — | `counted_loop` | `passes/counted_loop.rs` | Support module: loop recognition contract | Not a pipeline pass; called by `loop_unroll` |

### 1.2 loop_unroll: Current Gate, Shape Recognition, Fire Status

Gate at `runtime/molt-passes/src/tir/passes/loop_unroll.rs`:170:
```rust
if func.has_exception_handlers() {
    return Vec::new();
}
```

This uses `has_exception_handlers()` (the narrow predicate from `function.rs`:153-166), NOT `has_exception_handling`. The gate fix from the original design doc was ALREADY APPLIED to the current tree (confirmed reading line 170 — it says `has_exception_handlers()`, not `has_exception_handling`). This means loop_unroll's gate was fixed as part of `fae639e94` (the canonical counted-loop contract commit).

Shape recognition: `loop_unroll.rs`:8-65 documents the recognized shape — the multi-arg header + separate cond block (`counted_loop.rs` Route B). This is NOT the old 1-arg header shape. The recognizer (`counted_loop::recognize_counted_loop`) handles the real frontend shape.

Fire conditions (`loop_unroll.rs`:187-217):
1. `recognize_counted_loop` succeeds (line 183)
2. `trip_count <= tti.unroll_max_trip()` — default 8 (`target_info.rs`:198)
3. `region_ops <= tti.unroll_max_body()` — default 20 ops
4. No region value escapes other than modelled threading

`bench_sum` loops over 10M iterations — trip_count >> 8, so `loop_unroll` correctly refuses. For the benchmarks with small constant trip counts (e.g., inner loops in `bench_matrix_math.py` with `range(2)`, trip=2), `loop_unroll` fires today.

### 1.3 block_versioning and type_guard_hoist: TypeGuard Producer Gap

`block_versioning.rs`:378:
```rust
if func.has_exception_handling {
    return stats;
}
```

This still uses the wide `has_exception_handling` flag (confirmed reading lines 373-380). This is NOT yet fixed.

`type_guard_hoist.rs`:90:
```rust
if func.has_exception_handling {
    return stats;
}
```

Same issue — still on the wide flag (confirmed reading lines 86-100). Also NOT yet fixed.

However, neither fix matters yet because of the deeper issue: there is no TypeGuard producer in the production pipeline. Searching all files that reference `OpCode::TypeGuard` confirms:
- The removed deoptimization skeleton used to read TypeGuard ops as bailout points, but it was never
  a TypeGuard generator.
- `type_refine.rs`: reads TypeGuard ops to propagate proven types (lines 44-49, 613-614) — not a generator
- `ssa.rs`:1860: deserialization string→OpCode table — not a generator
- `lower_to_simple.rs`:1660: lowers TypeGuard as a no-op copy-var — not a generator
- `effects.rs`:343: classifies TypeGuard as PURE — not a generator
- `vectorize.rs`, `alias_analysis.rs`, `escape_analysis.rs`: read but don't emit

The frontend does not emit `TypeGuard` ops for `isinstance` checks or polymorphic dispatch. There is no compiler pass that converts `isinstance(x, int)` or a polymorphic `CallMethod` into a `TypeGuard` op before `block_versioning` or `type_guard_hoist` run. Both passes are structural dead code in production.

### 1.4 LICM: Already Works Correctly

`licm.rs`:95-130 explicitly does NOT bail on `has_exception_handling`. It uses `effects::opcode_is_pure_movable` (line 62) which includes `TypeGuard` as pure (effects.rs:343). LICM fires on invariant arithmetic in loops today.

### 1.5 overflow_peel: The Fast Accumulator Path

`overflow_peel.rs`:145 gates on `func.has_exception_handlers()` (the narrow predicate — correct). Verified fire status: landed `e267a4f5a`, `bench_sum` went from 2.2× slower to 14× faster than CPython on native. The fast lane uses `CheckedAdd` with hardware overflow detection and a boxed slow-path continuation.

### 1.6 strength_reduction: FloorDiv/Mod Deferred

`strength_reduction.rs`:102-112 has explicit deferrals:
```rust
OpCode::FloorDiv => {
    // x // 2^k => x >> k — deferred to Phase 3 ...
}
OpCode::Mod => {
    // x % 2^k => x & (2^k - 1) — same complexity issue ...
}
```

The deferral comment at line 104 says "requires inserting a new ConstInt op ... not yet available." This is untrue now — `range_devirt::apply_transform` does exactly this mutation, and the block-mutation pattern is established. But this is a minor incremental fix, not a structural arc.

### 1.7 range_devirt: Fire Status

`range_devirt.rs`:152-158 matches only `OpCode::IterNextUnboxed`. The frontend emits `for i in range(n)` as a counted loop with no iterator protocol at all — no `CallBuiltin("range")`, no `GetIter`, no `IterNextUnboxed`. The `range_devirt` pass was designed to handle the iterator-protocol form of range loops, which the frontend no longer produces for simple `for i in range(...)` patterns. Thus `range_devirt` fires `(0,0,0)` on real benchmark code.

### 1.8 Pipeline Summary: What Is Actually Firing on Loop Code Today

For `bench_sum` (`for i in range(10M): total += i`):
- `range_devirt`: 0 (no iterator ops in the counted loop shape)
- `iter_devirt`: 0 (not a list iteration)
- `loop_unroll`: 0 (trip_count = 10M >> 8 cap)
- `block_versioning`: 0 (has_exception_handling = true AND no TypeGuard ops)
- `type_guard_hoist`: 0 (has_exception_handling = true AND no TypeGuard ops)
- `licm`: potentially fires on loop-invariant constants
- `overflow_peel`: FIRES — rewrites total += i into CheckedAdd dual-loop peel
- `check_exception_elim`: FIRES — eliminates per-iteration CheckException ops after peel

For `bench_matrix_math` (inner `for k in range(2)` loops):
- `loop_unroll`: FIRES (trip_count = 2 ≤ 8)
- SCCP then folds the unrolled constants

---

## Section 2: Value Analysis — What L4 Still Buys

### 2.1 Shape A: Polymorphic Method Dispatch in Loops (block_versioning/type_guard_hoist blocked by TypeGuard generation gap)

`bench_class_hierarchy.py`:

```python
def main() -> None:
    obj = Leaf()
    total = 0
    for i in range(5_000_000):
        total += obj.compute(i)  # polymorphic dispatch
```

Today: dispatch-IC (`798f9b136`) improved this to 0.32× CPython (still slower). Each iteration does a CHA/IC lookup for `obj.compute`. With TypeGuard generation, `isinstance`-based type tests AND CHA-proven method identity could emit a `TypeGuard(%obj, Leaf)` before the loop, which `type_guard_hoist` could hoist to the preheader, and `block_versioning` could use to emit a specialized block where the dispatch is devirtualized to a direct `Call`. Expected win: eliminate the IC lookup overhead, which is the dominant remaining cost. Order-of-magnitude estimate: 2-5× on top of the current 0.32×, potentially reaching 1×+ CPython. The dispatch-IC already landed a 7× improvement; TypeGuard hoisting gives the compiler-driven specialization where the IC provides runtime specialization.

### 2.2 Shape B: Type-Polymorphic Accumulation (block_versioning/type_guard_hoist)

```python
def sum_mixed(items):  # items: list of int | float
    total = 0
    for x in items:
        total += x  # DynBox dispatch every iteration
```

Today: `x` is `DynBox`, `total += x` is a polymorphic `InplaceAdd` with dynamic dispatch. With TypeGuard generation at the loop entry (a `TypeGuard(%x, int)` emitted by the devirtualized `iter_devirt` result), `block_versioning` would create an int-specialized block where `total += x` becomes a raw `InplaceAdd(I64, I64)` with overflow_peel. Type guard hoisting would move the guard to before the loop (if `x`'s type can be proven loop-invariant from the list's element type). Expected win: elimination of DynBox dispatch per iteration → 3-10× on typed list inputs.

### 2.3 Shape C: Small Fixed-Count Inner Loops with Stride Multiply (loop_unroll + IV strength reduction)

`bench_matrix_math.py`:

```python
for k in range(2):
    acc = acc + molt_buffer.get(a, row, k) * molt_buffer.get(b, k, col)
```

Today: `loop_unroll` fires (trip=2 ≤ 8), unrolling to 2 straight-line copies. BUT each copy still has `row * stride + k` style index computations (if the buffer access involves stride multiply). Without IV strength reduction, each unrolled copy re-multiplies. With IV-SR, the stride multiply `k * col_stride` becomes an accumulator `acc += col_stride`, eliminating the multiply from the unrolled copies. After SCCP folds the constants (k=0,1), the multiplies fold to constants anyway — so for fully unrolled loops the actual win of IV-SR is zero (SCCP handles it). The real win of IV-SR is for large-N loops that cannot be unrolled: `i * stride` in a 10M-iteration loop.

### 2.4 Shape D: Large Loops with Stride Multiply (IV strength reduction, not unroll)

```python
def stride_access(n, stride):
    total = 0
    for i in range(n):  # n = 10_000_000
        total += i * stride  # Mul per iteration
    return total
```

Today: `loop_unroll` refuses (n >> 8). Each of 10M iterations does a `Mul(i, stride)`. With IV-SR: `acc` accumulates `stride` each iteration, eliminating the Mul. For constant `stride`, LICM already hoists the stride value, but the per-iteration `i * stride` Mul is NOT loop-invariant (depends on `i`) so LICM doesn't eliminate it. IV-SR reduces it to an Add. Expected win: 1 Mul → 1 Add per iteration. On modern hardware, integer multiply costs 3-5 cycles versus 1 cycle for add. For 10M iterations: potentially 20-40M cycles saved. At ~3GHz that is ~7-13ms saved on a function that runs in maybe 30ms — roughly 1.3-1.4× speedup. Not a multiple-order-of-magnitude win, but this is the correct structural fix that also unblocks vectorization (stride-uniform IVs are the precondition for auto-vectorization recognizers in `vectorize.rs`).

### 2.5 Shape E: Loop-Invariant Type Guard Not Hoisted (type_guard_hoist gap)

For any function with a loop that contains an `isinstance` check on a loop-invariant value:

```python
def typed_sum(x, n):
    total = 0
    for i in range(n):
        if isinstance(x, int):  # type_guard lowered here
            total += x + i
    return total
```

Today: the `isinstance` check executes N times per loop invocation. Once TypeGuard generation exists, `type_guard_hoist` would hoist it to the preheader (x is loop-invariant, defined before the loop). Expected win: eliminate N-1 redundant type checks per call. For N=10M, this is ~10M branch predictions worth of overhead.

---

## Section 3: Phased Implementation Plan

### 3.1 Phase 1: Fix the Exception Gate on block_versioning and type_guard_hoist (preparatory, ~1 hour)

This is a 30-line change. It is ONLY worth landing because it is a prerequisite for Phase 2 testing (without it, even if TypeGuard ops exist post-Phase-2, the passes still bail). It is NOT a standalone perf win.

Files to change:
- `runtime/molt-passes/src/tir/passes/block_versioning.rs`:378 — change `func.has_exception_handling` to `func.has_exception_handlers()`
- `runtime/molt-passes/src/tir/passes/type_guard_hoist.rs`:90 — change `func.has_exception_handling` to `func.has_exception_handlers()`

The stale comments at `block_versioning.rs`:373-377 (the "full-CFG PredMap == terminator-only when no exception handling" justification) and `type_guard_hoist.rs`:94-98 must also be deleted or updated — they are now false.

The `TerminatorOnlyPredMap` analysis described in the original design doc is a structural improvement (prevents phantom back-edges from `CheckException → handler` arcs from misidentifying non-loop blocks as loop headers) and should accompany this fix. However, as the original design doc verified, WITHOUT TypeGuard ops these passes fire on zero candidates regardless, so the pred-map fix's miscompile risk only materializes with TypeGuard ops present. The correct sequencing:

- [ ] `runtime/molt-passes/src/tir/analysis/mod.rs`: Add `TerminatorOnlyPredMap` variant to `AnalysisId` enum after `Liveness` (line 87). Current `ALL` array is 12 entries — add as entry 13.
- [ ] Implement `Analysis` for `TerminatorOnlyPredMap` with `CFG_SENSITIVE: true`, `OPS_SENSITIVE: false`, calls `dominators::build_pred_map_with(func, CfgEdgePolicy::TerminatorOnly)` (the `TerminatorOnly` variant exists per `counted_loop.rs`:440 and `analysis/mod.rs`:214)
- [ ] Add `TerminatorOnlyPredMap` to the `cfg_sensitive`/`ops_sensitive` match arms in `analysis/mod.rs`:371-408
- [ ] Add `TerminatorOnlyPredMap` to the `check!` macro dispatch in `pass_manager.rs`:510-524
- [ ] `block_versioning.rs`:378 gate fix + line 382: switch from `am.get::<PredMap>(func)` to `am.get::<TerminatorOnlyPredMap>(func)` + delete stale comment lines 373-377 + add import
- [ ] `type_guard_hoist.rs`:90 gate fix + line 100: switch from `am.get::<PredMap>(func)` to `am.get::<TerminatorOnlyPredMap>(func)` + delete stale comment lines 94-98 + add import
- [ ] Unit tests: `test_terminator_only_pred_map_excludes_exception_edges` (build a function with a `CheckException` that would create a phantom back-edge; assert `TerminatorOnlyPredMap` excludes it while `PredMap` includes it)
- [ ] Unit test: `test_block_versioning_fires_with_check_exception` — build a function with `has_exception_handling = true` but no `TryStart`/`TryEnd`, add a `TypeGuard` op, assert `block_versioning` fires
- [ ] Unit test: `test_type_guard_hoist_fires_with_check_exception` — same for hoist pass
- [ ] `cargo test -p molt-backend` all green, zero new warnings
- [ ] Gate: `TIR_OPT_STATS=1` on a hand-constructed test function with a TypeGuard op shows `block_versioning: 1 values_changed`

Mutation class: `block_versioning` is `Cfg` (unchanged), `type_guard_hoist` is `Cfg` (unchanged). `TerminatorOnlyPredMap` is `CFG_SENSITIVE: true`, `OPS_SENSITIVE: false`.

This is the smallest complete structural piece: one atomic commit fixing the gate + predmap + registering the new analysis. It has zero production impact (neither pass fires without TypeGuard producers) but closes the structural debt.

### 3.2 Phase 2: TypeGuard Generation Pass (the true prerequisite for block_versioning/type_guard_hoist)

This is the real work. There is currently no pass that converts polymorphic dispatch patterns into `TypeGuard` ops. Two source patterns to handle:

**Pattern A: `isinstance(x, T)` lowering**

The frontend lowers `isinstance(x, int)` into a `CallBuiltin("isinstance", x, int_type)`. A TypeGuard generation pass would recognize this pattern and insert a `TypeGuard(x, "int")` op with the result being the bool outcome, in the block where the `isinstance` check occurs. The `CondBranch` on the result becomes the guard's success/failure edge.

This is NOT a trivial add — it requires:
1. Understanding how `isinstance` is currently lowered in the frontend (`ssa.rs` or the Python frontend lowering code)
2. Deciding whether to emit TypeGuard at the frontend (during SSA lowering) or as a mid-pipeline TIR pass
3. Ensuring the `TypeGuard` result round-trips correctly through `lower_to_simple.rs`:1660 (currently lowered as a `copy_var` no-op — this may need to route through the actual runtime isinstance check if not specialized)

**Pattern B: CHA-proven method dispatch**

When CHA (`dispatch_ic.rs`, commit `798f9b136`) proves that a method call `obj.compute(i)` always resolves to a specific method given `type(obj) == Leaf`, emit a `TypeGuard(%obj, Leaf)` before the loop. This guard is then hoistable (obj is loop-invariant), enabling the specialized block to contain a direct `Call(Leaf_compute, obj, i)` instead of a dynamic dispatch.

Architecture decision: TypeGuard generation belongs as a mid-pipeline TIR pass placed AFTER `type_refine` (which propagates type facts) but BEFORE `block_versioning` and `type_guard_hoist` (which consume them). Current pipeline order has `block_versioning` at position 9 and `type_guard_hoist` at position 18. A TypeGuard generation pass should run at position 8.5 — after `canonicalize_post` (position 8, so type-narrowed values are visible) and before `block_versioning` (position 9).

New file: `runtime/molt-passes/src/tir/passes/type_guard_gen.rs`

```
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats
```

Algorithm:
1. Walk all blocks in RPO (use `am.get::<StrictReachable>(func)` for the reachable set + `am.get::<ImmediateDoms>(func)` for the RPO order)
2. For each `CallBuiltin("isinstance", value, type_const)` → `bool_result`:
   a. Parse `type_const` to a `TirType`
   b. If parseable: rewrite the `CallBuiltin` to a `TypeGuard(value, ty)` op with `ty` attr = the type string
   c. The result `bool_result` now represents the TypeGuard's bool (true = guard succeeded)
3. The existing `CondBranch` on `bool_result` is unchanged — it now correctly represents the TypeGuard success/failure branch that `block_versioning` and `type_guard_hoist` both consume

Mutation class: `OpsOnly` (rewrites `CallBuiltin("isinstance")` to `TypeGuard` in-place — same operand/result arity, no CFG change). Important: `TypeGuard` is classified `PURE` in `effects.rs`:343, so it does not block LICM hoisting of other ops. However, per `effects.rs`:343, `TypeGuard` itself IS hoistable by LICM (`opcode_is_pure_movable` returns true for pure ops). This means LICM will already hoist loop-invariant TypeGuards WITHOUT `type_guard_hoist`. The `type_guard_hoist` pass runs AFTER `sccp` (position 18 vs LICM at position 11) — it catches cases LICM missed. Given this, the ordering matters: TypeGuard generation at position ~8.5 means LICM at position 11 will attempt to hoist loop-invariant guards automatically.

Test specification:
- Unit test: `test_isinstance_lowered_to_typeguard` — build a function with `CallBuiltin("isinstance", %x, "int")`, run `type_guard_gen`, assert result is `TypeGuard(%x, "int")`
- Unit test: `test_typeguard_gen_then_block_versioning` — end-to-end: isinstance → TypeGuard → block_versioning fires → `values_changed > 0`
- Differential test (Shape A): `isinstance(x, int)` in a loop, verify correct results on CPython and molt, verify `TIR_OPT_STATS=1` shows `type_guard_gen: N ops_added > 0`, `block_versioning: M values_changed > 0`

Add `pub mod type_guard_gen;` to `runtime/molt-passes/src/tir/passes/mod.rs`.

Register in `build_default_pipeline` (`pass_manager.rs`):
```rust
pass("type_guard_gen", OpsOnly, |f, am, _tti| {
    passes::type_guard_gen::run(f, am)
}),
```
Insert between `canonicalize_post` (position 8) and `block_versioning` (position 9) in `pass_manager.rs`:337-338.

The pipeline order table in `mod.rs`:139-170 MUST be updated to match; otherwise the `pipeline_records_every_pass_unconditionally` test at line 133 will fail (it asserts the exact ordered pass name list).

### 3.3 Phase 3: IV Strength Reduction (independent of TypeGuard arc)

New file: `runtime/molt-passes/src/tir/passes/iv_strength_reduction.rs`

```rust
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats
```

Algorithm:
1. Early exit: `if func.loop_roles.is_empty() { return stats; }`
2. Get `scev = am.get::<ScalarEvolution>(func)` and `forest = am.get::<LoopForest>(func)`
3. For each loop header `h` in `forest.headers` (innermost-first by descending `BlockId` order):
   - Get loop body blocks from `forest.bodies[h]`
   - For each op `Mul(iv_candidate, stride_candidate)` in any body block:
     - `scev.scev_of(iv_candidate)` must be `ScevExpr::AddRec { start, step, loop_header: h }`
     - `scev.scev_of(stride_candidate)` must be `ScevExpr::Invariant(_)` or `ScevExpr::Constant(_)`
     - The AddRec `start` and `step` must themselves be constants or invariants (resolvable to `ValueId`s)
   - If candidate found, plan the transform:
     - `acc_init` = the `start * stride` initial accumulator value (fold if both constant, else emit a `Mul` in the preheader via `find_preheader` from `licm.rs` pattern)
     - Add `acc` as a new block argument to header `h` (append to `header.args`)
     - Update the preheader's `Branch -> h(...)` to include `acc_init` as the new arg
     - Update the back-edge `Branch -> h(...)` to include `acc_next = Add(acc, stride_val)`
     - Insert `acc_next = Add(acc, stride_val)` before the back-edge terminator in the latch block
     - Replace the `Mul(iv, stride)` result with `acc` everywhere in the loop body
     - If the original `Mul` result is now dead, remove it

Soundness guards:
- Only fire when `scev.scev_of(iv)` is `AddRec` with `no_signed_wrap` confirmed (SCEV already enforces this — `scev.rs`:43, only creates `AddRec` from `no_signed_wrap` increments)
- If `stride` is `MaybeBigInt` (not `RawI64Safe`): SKIP. The new `Add` would inherit the same representation concern. The `Repr` of the accumulated result must match the `Repr` of the original `Mul` result — if the `Mul`'s result is `MaybeBigInt`, the accumulator must be too, which requires the slow-path allocation overhead. IV-SR only pays for `RawI64Safe` strides.
- The `acc_next = Add(acc, stride)` op should carry `no_signed_wrap` only when `stride` is proven bounded and `acc` accumulates within i64 range. Conservatively: do NOT add `no_signed_wrap` to the new Add unless SCEV can bound the accumulated range. Without it, SCEV classifies the accumulator as `Unknown` (degree-2 recurrence) — correct and safe.

Mutation class: `Cfg` (adds block arguments to headers, modifies predecessor terminators).

Pipeline placement: after `loop_unroll` (position 4), before `canonicalize` (position 5). This allows SCCP to fold the now-constant accumulator values in unrolled bodies.

Register:
```rust
pass("iv_strength_reduction", Cfg, |f, am, _tti| {
    passes::iv_strength_reduction::run(f, am)
}),
```
In `build_default_pipeline` between `loop_unroll` and `canonicalize`.

The `pipeline_records_every_pass_unconditionally` test in `mod.rs`:133 must be updated to include `"iv_strength_reduction"` in its expected list.

Test specification:
- Unit test `test_iv_sr_constant_stride` — build a loop with `Mul(iv, 3)` where `iv` is an `AddRec` with `no_signed_wrap`; run the pass; assert `ops_added > 0`, `ops_removed > 0`; verify semantics by constant-folding the unrolled output
- Unit test `test_iv_sr_loop_invariant_stride` — stride is a block argument (loop-invariant but not constant)
- Unit test `test_iv_sr_bigint_skip` — `iv` is `MaybeBigInt`-typed; assert pass fires 0 ops
- Unit test `test_iv_sr_degree_two_skip` — accumulator-style `i*i` (both operands are IVs); SCEV returns `Unknown` for one; assert pass skips

Differential test:
```python
def stride_sum(n, stride):
    total = 0
    for i in range(n):
        total += i * stride
    return total
assert stride_sum(5, 3) == 30
assert stride_sum(0, 7) == 0
```
Verify CPython-identical on all backends. Gate: `TIR_OPT_STATS=1` shows `iv_strength_reduction: 1 values_changed`.

### 3.4 Phase 4: strength_reduction.rs FloorDiv/Mod Power-of-Two Completion

This is a ~30-line change to complete the deferred "Phase 3" from `strength_reduction.rs`:102-112.

The deferral comment cites "requires inserting a new ConstInt op — a block mutation." This is a false constraint: `strength_reduction` is declared `OpsOnly` in `pass_manager.rs`:376, but adding a `ConstInt` op to a block is an op addition WITHIN a block — not a CFG mutation. `OpsOnly` explicitly permits "add/remove ops within a block" per `pass_manager.rs`:82-94.

The only pattern needed: find or create the ConstInt for the shift amount k, insert it BEFORE the FloorDiv op, replace FloorDiv with Shr(lhs, k). Two-pass approach: collect candidates (op index + shift amount), then apply insertions in reverse order to preserve indices.

Files to change:
- `runtime/molt-passes/src/tir/passes/strength_reduction.rs`:62-115

Differential tests:
```python
assert 100 // 4 == 25     # FloorDiv by power-of-2
assert 100 % 8 == 4       # Mod by power-of-2
assert 7 // 3 == 2        # NOT a power-of-2 — unchanged
assert (-8) // 4 == -2    # Negative operand (floor division)
assert (-7) // 4 == -2    # Python floor division rounds toward -inf
```

WARNING: Python `//` is floor division, NOT truncating division. `Shr` on a negative integer is arithmetic right shift (matches floor division for `x // 2^k` when `x < 0`). `x & (2^k - 1)` for `x % 2^k` returns the Python-correct non-negative result for negative `x` in two's complement. Verify both cases with differential tests before shipping.

### 3.5 Phase Ordering Summary (one complete structural piece per commit)

```
Phase 1 (preparatory, ~1h):
  TerminatorOnlyPredMap analysis + gate fixes for block_versioning + type_guard_hoist
  Files: analysis/mod.rs, pass_manager.rs, block_versioning.rs, type_guard_hoist.rs
  Gate: all 30 pipeline tests green, zero new warnings
  Perf gate: 0 production impact (by design — no TypeGuard producer)

Phase 2 (the unlock, ~1 day):
  TypeGuard generation pass from isinstance -> TypeGuard lowering
  Files: passes/type_guard_gen.rs (new), passes/mod.rs, pass_manager.rs
  Gates: isinstance differential tests on 3 CPython versions, TIR_OPT_STATS shows non-zero on polymorphic loop shapes

Phase 3 (IV-SR, ~1 day):
  IV Strength Reduction pass
  Files: passes/iv_strength_reduction.rs (new), passes/mod.rs, pass_manager.rs
  Gates: stride_sum differential test, SCEV soundness with MaybeBigInt and degree-2 shapes

Phase 4 (cleanup, ~30min):
  Complete FloorDiv/Mod power-of-2 reduction
  Files: passes/strength_reduction.rs
  Gates: floor division edge cases including negative numbers
```

---

## Section 4: Risk Register

### Risk 1: TypeGuard generation vs. isinstance semantics (CRITICAL, Phase 2)

`isinstance` in Python is not a simple type check — it traverses the MRO and handles registered ABCs, `__instancecheck__`, etc. If `type_guard_gen` replaces `CallBuiltin("isinstance")` with `TypeGuard`, it must NOT do so for cases where isinstance could return True for a subclass but the TypeGuard's downstream specialization assumes the exact class.

Mitigation: The TypeGuard rewrite must only fire when the type argument is a PRIMITIVE type (int, float, bool, str, bytes, NoneType) where isinstance has exact-type semantics AND where the backend's type lattice correctly represents the subtype relationship. For user-defined classes, defer until CHA is integrated.

The `lower_to_simple.rs`:1660 treatment of TypeGuard as `copy_var` means that if `block_versioning` does NOT fire (no proving predecessor), the TypeGuard falls through as a copy of the operand — which is semantically wrong (should be a bool). This requires a parallel fix: `lower_to_simple.rs` must lower `TypeGuard` as an actual isinstance call when it is NOT eliminated by `block_versioning`. The current `copy_var` treatment assumes `block_versioning` always eliminates or the TypeGuard is already dead — this is no longer safe once `type_guard_gen` starts emitting TypeGuards for `isinstance` calls that may not get versioned.

### Risk 2: PhantomBackEdge after TerminatorOnlyPredMap switch (LOW after Phase 1)

After Phase 1, `block_versioning` and `type_guard_hoist` use `TerminatorOnlyPredMap`. The back-edge detection `p.0 >= bid.0` on terminator-only preds is sound for the structured loop shapes the frontend emits. However, if a pass reorders BlockIds (e.g., `loop_unroll` inserts a fresh landing block with a new high BlockId), the `p.0 >= bid.0` heuristic could give a false result on the modified CFG.

The correct fix long-term is to use `LoopForest` for loop-header detection (not the BlockId ordering heuristic). Both `block_versioning.rs`:106-113 and `type_guard_hoist.rs`:111-126 should consult `am.get::<LoopForest>(func)` for the canonical loop-header set rather than re-deriving it from BlockId ordering. This is the deeper structural fix; the Phase 1 TerminatorOnlyPredMap fix is a necessary stepping stone but not the final form.

Filing this as a technical debt item: post-Phase 2 when the passes actually fire, add a migration from `p.0 >= bid.0` to `LoopForest`-based header detection in both passes.

### Risk 3: IV Strength Reduction + overflow_peel interaction (MEDIUM, Phase 3)

`overflow_peel` runs at position 26 (after `check_exception_elim` at 25). IV-SR runs at position 4.5 (after `loop_unroll`, before `canonicalize`). The interaction: IV-SR fires first and creates a simpler loop shape (acc += stride) that overflow_peel then handles correctly — overflow_peel already handles multi-accumulator loops (it peels ALL qualifying phi updates simultaneously). No conflict expected; verify with a combined-shape differential test.

### Risk 4: RC Drop Insertion interaction with IV-SR new block args (MEDIUM, Phase 3)

`drop_insertion` (dormant; activation pending design 20 finding #4/#5) inserts `DecRef` at each value's last use. IV-SR adds new block arguments to loop headers. The new header arg `acc` is a raw integer and should have `Repr::RawI64Safe` if stride is proven safe — the drop_insertion pass filters by repr and does not insert `DecRef` for raw scalars.

Verification gate: run IV-SR on a function, then confirm that `drop_insertion` produces zero new `DecRef` ops for the `acc` header arg. Use `TIR_OPT_STATS=1`. ORDERING CONSTRAINT: if RC activation lands first, re-run its memory corpus as part of Phase 3's gates.

### Risk 5: Pipeline order test breakage on new pass insertion (LOW)

`mod.rs`:133-172 asserts the exact ordered list of 30 pass names. Each phase that adds a pass changes this list. The test MUST be updated atomically with the pass registration.

### Risk 6: TypeGuard round-trip soundness through lower_to_simple (HIGH for Phase 2)

`lower_to_simple.rs`:1660 currently lowers `TypeGuard` as a `copy_var` — an unconditional no-op regardless of whether `block_versioning` fired. If `type_guard_gen` emits a TypeGuard for an isinstance call that was NOT eliminated by block_versioning, the copy_var lowering silently discards the isinstance check. Phase 2 must include a fix to `lower_to_simple.rs`:1660: an un-versioned TypeGuard surviving the pipeline must lower to an actual isinstance check (CallBuiltin path), not a copy.

### Risk 7: branchless_count has_exception_handling gate (minor, independent)

`branchless_count.rs` does not bail on `has_exception_handling` (verified — no such gate). It fires on any Bool-counted pattern regardless. Not on the L4 critical path.

---

## Section 5: Non-Goals and Dependency Edges

### Non-goals for this arc

- **CHA-based TypeGuard generation for method dispatch**: Phase 2 only handles `isinstance` (well-defined semantics for primitive types). CHA-generated TypeGuards require a separate `cha_devirt` pass consuming the S4 IP summaries.
- **Loop tiling / polyhedral optimization**: `polyhedral.rs` is a `ReadOnly` marking pass today. Actual tiling is not implemented and not in L4.
- **SIMD vectorization**: `vectorize.rs` is `ReadOnly` (marks candidates). L4 IV-SR is a precondition for vectorization but does not include vector codegen — that is L2.
- **Dynamic trip-count unrolling**: `loop_unroll` requires a static constant trip count. Partial/dynamic unrolling is not part of this arc.
- **TypeGuard for user-defined class hierarchies**: requires CHA + full MRO semantics. Out of scope for Phase 2.

### Dependency edges

```
Phase 1 (gate fix):
  Requires: nothing (conservative narrowing of existing passes)
  Unblocks: Phase 2 testing (passes will fire when TypeGuards exist)

Phase 2 (TypeGuard generation):
  Requires: Phase 1 (gate fix must be in place so block_versioning fires post-TypeGuard)
  Requires: lower_to_simple.rs TypeGuard lowering fix (correctness)
  Unblocks: real CHA devirt (TypeGuard is the hook); type_guard_hoist providing value

Phase 3 (IV-SR):
  Requires: S6 SCEV (LANDED, cd66f365e)
  Requires: S1 LoopForest + DefMap (LANDED, ef284d182)
  Requires: counted_loop contract (LANDED, fae639e94)
  Independent of: Phase 1, Phase 2 (different optimization axis)
  Unblocks: L2 vectorization (stride-uniform IV is the SIMD precondition)

Phase 4 (FloorDiv/Mod SR):
  Requires: nothing
  Independent of all other phases
  Note: verify Python floor division semantics on negative inputs
```

### Blocked arcs (what L4 does NOT unblock directly)

- **RC-1 DropInsertion** (design 20): independent, #1 correctness blocker. L4 phases must not introduce new lifetime hazards (see Risk 4; the Repr filter handles this correctly).
- **E1-e LLVM/Luau activation**: independent of loop passes (LLVM activation LANDED 0e55aff9a).
- **L2 real SIMD**: blocked on IV-SR landing (Phase 3) AND on vectorize.rs being upgraded from `ReadOnly` marking to actual emission. L4 Phase 3 is the prerequisite but not sufficient.
- **D1 generator fusion**: independent; requires E1 active (DONE) and SROA (DONE).

---

## Summary of Verified Facts vs. Stale Design Doc Claims

1. `loop_unroll` gate: already on `has_exception_handlers()` (loop_unroll.rs:170, part of `fae639e94`). DONE.
2. `block_versioning` (line 378) and `type_guard_hoist` (line 90) gates: still on the wide `has_exception_handling` flag. Phase 1 fixes them.
3. Loop shape: the real fix was `counted_loop.rs` (Route B recognizer) wired into `loop_unroll` — DONE in `fae639e94`. `loop_unroll` fires on real counted loops within the trip cap.
4. TypeGuard generation gap: confirmed — no producer exists anywhere. The actual prerequisite for block_versioning/type_guard_hoist.
5. `range_devirt` fires 0 on real `for i in range(n)` loops (frontend emits counted shape, no iterator protocol). Confirmed.
6. SCEV/ValueRange: LANDED (`cd66f365e` + precision `9e93503bb`). IV-SR can consume it.
7. overflow_peel: LANDED (`e267a4f5a`), bench_sum 14× faster. The simple `total += i` accumulator shape is DONE.
