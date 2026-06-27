<!-- Foundation blueprint (architect swarm wf_18b24759-006, 2026-06-04). Arc: L4 loop-transform family: re-enable the 3 exception-gated passes + fix the range_devirt shape gap + L1 IV strength reduction -->

# Loop Optimization Re-Enablement and IV Strength Reduction — Architecture Blueprint

## 1. Problem Statement and Load-Bearing Nature

### The Production Regression

Commit `430e09793` (C2) made `CheckException` universal: every real function now carries at least one `CheckException` op. The `has_exception_handling` flag is set by `lower_from_simple.rs:319-330` on any function containing `TryStart | TryEnd | StateBlockStart | StateBlockEnd | CheckException`. This means `has_exception_handling == true` on virtually every non-trivial function.

Three loop optimization passes bail at the top on `has_exception_handling`:
- `loop_unroll.rs:250` — `find_unroll_candidates` returns empty
- `block_versioning.rs:378` — `run` returns early
- `type_guard_hoist.rs:90` — `run` returns early

Additionally `licm.rs:101-122` already solved this problem correctly: it does NOT bail wholesale on `has_exception_handling` and instead delegates the safety question to the per-op `is_hoistable` predicate (which gates on `opcode_is_pure_movable`, excluding `CheckException` and all exception-adjacent ops). The same design pattern is the correct fix for the other three passes.

The consequence: on every function in the standard compilation path, the three passes produce zero work. LICM already runs correctly. The perf gap is:
- No loop unrolling → tight counted loops (`for i in range(8):`) generate full loop overhead instead of straight-line code
- No block versioning → TypeGuard chains in loop successors are not specialized; unnecessary runtime type checks survive
- No type guard hoisting → loop-invariant `TypeGuard(%x, INT)` rechecked every iteration

### The range_devirt Non-Fire Issue

The blueprint's claim that `range_devirt=(0,0,0)` on real code indicates a separate problem: `range_devirt.rs` matches `IterNextUnboxed` in loop headers (`loop_roles`), but the frontend `for i in range(n):` may lower through a different shape depending on whether it arrives at TIR with a recognized `CallBuiltin("range")` + `GetIter` pair, or already as a direct `LoopHeader(ind_var)` 2-arg form from a different frontend path. The mismatch needs diagnosis before `loop_unroll` can fire even after the gate fix.

Specifically: `range_devirt.rs:152-158` only matches `IterNextUnboxed` (requiring `op.results.len() == 2`). It skips `ForIter`. The `_uses_unboxed` field at line 67 is dead — `ForIter` is in the enum comment but the match arm (line 157) is a bare `continue` for non-`IterNextUnboxed`. If the frontend emits `ForIter` instead of `IterNextUnboxed`, range_devirt silently skips it.

### The `block_versioning` and `type_guard_hoist` PredMap Hazard

Both passes use `am.get::<PredMap>(func)` which calls `dominators::build_pred_map` (full CFG including exception edges). With `CheckException` ops, this creates extra edges from every block containing a `CheckException` to its handler target. `block_versioning::find_loop_headers` uses `p.0 >= bid.0` back-edge detection on this augmented pred map — exception edges from high-numbered blocks to low-numbered handler blocks can create phantom "back-edges" that misidentify non-header blocks as loop headers, causing them to be skipped for versioning. Similarly `type_guard_hoist.rs:114` uses `p.0 >= bid.0` on the same full PredMap.

The fix: both passes must switch to a terminator-only predecessor map for back-edge detection. `dominators::build_pred_map_with(func, CfgEdgePolicy::TerminatorOnly)` already exists. A new `TerminatorOnlyPredMap` analysis needs to be registered in the `AnalysisManager`.

### The IV Strength Reduction Gap (L1)

`strength_reduction.rs:101-113` defers `FloorDiv` and `Mod` reductions because they require inserting a new `ConstInt` op — a block mutation. The comment says "deferred to Phase 3" (lines 104-110). The SCEV analysis (`scev.rs`) is already built and registered as an `AnalysisManager` analysis (S6). `AddRec` recurrences are already recognized for unit-step `for i in range(n):` loops. IV strength reduction (`i * stride` → `acc += stride` across back-edge) is entirely absent; `strength_reduction.rs` only performs scalar op simplification, not induction variable transformation.

This is load-bearing for the 5-year perf goals: every `for` loop with a body computing `i * stride` (array indexing, matrix strides, convolution offsets) re-multiplies each iteration instead of accumulating. It is also load-bearing for SIMD vectorization: a vector lane with a stride-multiply induction variable is a precondition for auto-vectorization recognizers.

---

## 2. Structurally Correct Design

### Design Principle

Follow the `licm.rs` model exactly: do NOT bail on `has_exception_handling`. Instead gate on `has_exception_handlers()` (the new predicate that excludes `CheckException`-only functions) for operations that are actually unsafe across handler boundaries. For the cases where the only hazard is the augmented-CFG pred map creating phantom back-edges, switch to the terminator-only pred map.

Every change is a narrowing of an over-conservative gate — it is conservative-correct by construction (the existing code is already correct in the regime where the gate fires; we only expand the regime where the pass runs).

### Design for Each Pass

**loop_unroll:** Gate `find_unroll_candidates` on `func.has_exception_handlers()` instead of `func.has_exception_handling`. A CheckException op in a loop body is NOT a hazard for unrolling: the unrolled clone of the `CheckException` op retains the same handler label (pointing at the fn-exit handler block), which is OUTSIDE the loop — correct. The only genuine hazard is a `TryStart`/`TryEnd` within the loop body (a `try` block inside the loop), which could mean a `CheckException` handler label inside a per-iteration scope. `has_exception_handlers()` catches exactly this case.

Additionally: `find_unroll_candidates` requires the loop to already be in the canonical `range_devirt` shape (header with 1 arg, comparison in header, Add in body). This requires `range_devirt` to have fired. See Phase 1 fix.

**block_versioning:** Two changes:
1. Gate on `func.has_exception_handlers()` instead of `func.has_exception_handling`
2. Replace `am.get::<PredMap>(func)` (line 382) with `am.get::<TerminatorOnlyPredMap>(func)` for the `find_loop_headers` call. The phase-2 predecessor rewiring (line 562) also reads `pred_map.get(&orig_bid)` — this must also use the terminator-only map so exception-edge predecessors are not incorrectly redirected to the specialized block.

The comment at lines 373-377 (the justification for why full PredMap == terminator-only PredMap when no exception handling) is no longer true. Delete the comment, replace the analysis used.

**type_guard_hoist:** Two changes:
1. Gate on `func.has_exception_handlers()` instead of `func.has_exception_handling`
2. Replace `am.get::<PredMap>(func)` (line 100) with `am.get::<TerminatorOnlyPredMap>(func)`. The back-edge detection at line 114 (`p.0 >= bid.0`) on the full pred map is the exact source of phantom back-edges from `CheckException → handler` arcs. With the terminator-only map, this detection is sound.

The comment at lines 94-98 (same justification, now false) must be deleted.

**New TerminatorOnlyPredMap analysis:** Register a new `AnalysisId::TerminatorOnlyPredMap` in `analysis/mod.rs`. Its `compute` calls `dominators::build_pred_map_with(func, CfgEdgePolicy::TerminatorOnly)`. It is CFG-sensitive, not ops-sensitive — same invalidation class as `PredMap`.

**range_devirt ForIter gap:** Add a match arm for `OpCode::ForIter` in `find_candidates` (line 153-158) that produces the `(elem_val, done_val)` pair. `ForIter` has a single result for the element value; the done flag is implicit in the CondBranch. Alternatively: confirm from the frontend whether real `for i in range(n):` ever produces `ForIter` vs `IterNextUnboxed`, and ensure the correct form is matched.

**IV Strength Reduction (L1):** Design a new TIR pass `passes/iv_strength_reduction.rs`. It runs AFTER `loop_unroll` and range_devirt in the pipeline, consuming `AnalysisManager::get::<ScalarEvolution>()`. For each loop, it identifies uses of the pattern `mul_result = Mul(iv, stride)` where `iv` is an `AddRec` over the loop and `stride` is loop-invariant. It transforms this by:
1. Computing the initial value `start * stride` and inserting it in the preheader as a new induction variable `acc0`
2. Adding `acc0` as a new block argument on the loop header
3. Replacing the `Mul(iv, stride)` use with `acc` (the current-iteration accumulator block arg)
4. Appending `acc_next = Add(acc, stride)` to the back-edge block (with `no_signed_wrap` if stride and IV are both proven bounded)
5. Threading `acc_next` as the block argument update on the back-edge terminator
6. Removing the original `Mul(iv, stride)` op if its result has no other uses

This requires block-mutation API support: inserting an op at the end of a block (before the terminator) and appending block arguments to a header and its predecessors. This mutation is already done by `range_devirt.rs` in `apply_transform` — the pattern is established.

---

## 3. Component Design

### 3.1 New Analysis: `TerminatorOnlyPredMap`

**File:** `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/analysis/mod.rs`

Add to `AnalysisId` enum:
```rust
/// Predecessor map over terminator edges ONLY (no implicit exception edges).
/// Used by back-edge detection in block_versioning and type_guard_hoist to
/// avoid phantom back-edges from CheckException -> handler arcs.
TerminatorOnlyPredMap,
```

Add to `AnalysisId::ALL` array (currently 10 entries; add as entry 11).

Add marker type and `impl Analysis`:
```rust
pub struct TerminatorOnlyPredMap;
impl Analysis for TerminatorOnlyPredMap {
    type Result = HashMap<BlockId, Vec<BlockId>>;
    const ID: AnalysisId = AnalysisId::TerminatorOnlyPredMap;
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = false;
    fn compute(func: &TirFunction) -> Self::Result {
        dominators::build_pred_map_with(func, CfgEdgePolicy::TerminatorOnly)
    }
}
```

Update `cfg_sensitive` and `ops_sensitive` match functions at the bottom of `mod.rs` to include the new variant.

Update `assert_analyses_fresh` in `pass_manager.rs` macro invocation to include `TerminatorOnlyPredMap`.

### 3.2 Gate Fix: `loop_unroll.rs`

**File:** `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/loop_unroll.rs`

Line 250: Replace `if func.has_exception_handling {` with `if func.has_exception_handlers() {`

Update the doc comment at line 14: change "No exception handling in the function." to "No real exception handler regions (TryStart/TryEnd/StateBlock) in the function. Functions that only contain CheckException observation ops — which merely forward a pending exception to the function-exit path — are eligible for unrolling because CheckException is a transparent pass-through, not a handler."

Add `CheckException` safety note to the soundness documentation at lines 1-52, explaining why duplicated `CheckException` ops in unrolled bodies are safe: they all point at the same fn-exit handler block, which is outside the loop and correctly models the "exception propagates immediately to caller" semantics.

### 3.3 Gate + PredMap Fix: `block_versioning.rs`

**File:** `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/block_versioning.rs`

Add import: `use crate::tir::analysis::TerminatorOnlyPredMap;`

Line 378: Replace `if func.has_exception_handling {` with `if func.has_exception_handlers() {`

Lines 373-377: Delete the stale comment about full-CFG == terminator-only when no exception handling (now false).

Line 382: Replace `let pred_map = am.get::<PredMap>(func).clone();` with `let pred_map = am.get::<TerminatorOnlyPredMap>(func).clone();`

This one substitution fixes both uses of `pred_map` in the pass (`find_loop_headers` call at line 383 and the predecessor rewiring at line 562).

### 3.4 Gate + PredMap Fix: `type_guard_hoist.rs`

**File:** `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/type_guard_hoist.rs`

Add import: `use crate::tir::analysis::TerminatorOnlyPredMap;`

Line 90: Replace `if func.has_exception_handling {` with `if func.has_exception_handlers() {`

Lines 94-98: Delete the stale comment.

Line 100: Replace `let pred_map = am.get::<PredMap>(func).clone();` with `let pred_map = am.get::<TerminatorOnlyPredMap>(func).clone();`

### 3.5 range_devirt ForIter Gap

**File:** `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/range_devirt.rs`

In `find_candidates`, the match at lines 153-158:

```rust
let (uses_unboxed, elem_val, done_val) = match op.opcode {
    OpCode::IterNextUnboxed if op.results.len() == 2 && !op.operands.is_empty() => {
        (true, op.results[0], op.results[1])
    }
    _ => continue,
};
```

The `ForIter` op emits a single result for the element (the done flag is implicit in the CondBranch structure — when `ForIter` is in a header and the header's CondBranch is on the implicit termination). Diagnosis: grep for how `ForIter` is emitted in the frontend vs `IterNextUnboxed`. If `ForIter` has 1 result and relies on a separate done-flag value from the CondBranch condition, the devirt transformation is different enough that it should be treated as a separate `for_iter_devirt` pattern (already handled by `iter_devirt.rs`). The priority fix is to ensure that after `iter_devirt` runs, the result is in the `IterNextUnboxed` shape that `range_devirt` recognizes — or alternatively, that `range_devirt` runs after `iter_devirt` in the pipeline and the `ForIter` → `IterNextUnboxed` normalization happens first.

Current pipeline order (`pass_manager.rs:290-293`): `range_devirt` runs before `iter_devirt`. This is the ordering bug: `iter_devirt` may produce the canonical `IterNextUnboxed` shape that `range_devirt` needs, but it runs second. Swap the order to: `iter_devirt` first, then `range_devirt`. This is a pipeline reordering change in `build_default_pipeline`.

### 3.6 IV Strength Reduction Pass

**File:** `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/iv_strength_reduction.rs` (new)

```rust
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats
```

Algorithm:
1. `if func.loop_roles.is_empty() { return stats; }` — fast path
2. Consume `am.get::<ScalarEvolution>(func)` and `am.get::<LoopForest>(func)`
3. For each loop header (from LoopForest, innermost first):
   a. For each `Mul(iv_candidate, stride_candidate)` op in the loop body:
      - Look up `iv_candidate` in the SCEV result: must be `AddRec { start, step, loop_header }` for THIS loop's header
      - `stride_candidate` must be `Invariant` (loop-invariant, confirmed by DefMap: defined outside the loop)
      - stride must be known (either `Constant` or `Invariant` SCEV form)
   b. If candidate found:
      - Resolve `start_val` and `stride_val` as SSA `ValueId`s (they may be ConstInt or loop-invariant values)
      - Compute `acc0 = start_val * stride_val` (either fold constant or emit a `Mul` in preheader)
      - Add `acc` as a new block argument to the loop header
      - Thread `acc0` from the preheader edge
      - In the loop body: find the back-edge increment block, append `acc_next = Add(acc, stride_val)` before the branch, thread `acc_next` on the back-edge
      - Replace all uses of the `Mul` result with `acc` (the current header block arg)
      - DCE the `Mul` op if result is now dead

**Block-mutation contract:** This pass creates new block args on the header and modifies predecessor terminators — it is `Mutates::Cfg` and must be registered as such in `build_default_pipeline`.

**Pipeline placement:** After `loop_unroll` (unrolling may eliminate loops entirely; run SR only on surviving loops), before `canonicalize` (so SCCP can fold the now-constant `acc` values in unrolled bodies). Insert between `loop_unroll` and `canonicalize` in `build_default_pipeline`.

**Registration in `mod.rs`:** Add `pub mod iv_strength_reduction;`

**Registration in `pass_manager.rs`:**
```rust
pass("iv_strength_reduction", Cfg, |f, am, _tti| {
    passes::iv_strength_reduction::run(f, am)
}),
```

---

## 4. Soundness Argument

### loop_unroll with CheckException

A `CheckException` op with attribute `value: <label>` carries an implicit CFG edge to the handler block. When the loop body is unrolled (cloned N times into a landing block), each clone of a `CheckException` in the body retains the SAME label value. The handler block is by construction outside the loop (it is the fn-exit handler block at the function level). Therefore:

- No iteration of the unrolled body can route to a mid-loop handler — there is none
- Each `CheckException` clone correctly propagates to the function-exit handler if an exception is raised
- The unrolled code has the same observable exception semantics as the original loop

This is provably sound: `has_exception_handlers()` is true only if there is a `TryStart`/`TryEnd` (a `try:` block) or `StateBlockStart`/`StateBlockEnd` (a generator state region) in the function. A function with only `CheckException` ops (no real handler) is safe to unroll because the exception-exit edge always exits the function entirely — it never re-enters the loop.

Adversarial case that MUST remain blocked: a `try:` block INSIDE a loop iteration — e.g., `for i in range(4): try: risky(i) except: handle(i)`. Here `TryStart`/`TryEnd` appear inside the loop body, `has_exception_handlers()` returns `true`, and unrolling is correctly refused.

### block_versioning and type_guard_hoist with TerminatorOnlyPredMap

Back-edge detection by `p.0 >= bid.0` is sound ONLY on the terminator-only CFG. A `CheckException` op creates an exception edge from its block to the handler block. Handler blocks typically have LOW `BlockId` values (they are allocated early during lowering). A block with a high `BlockId` containing `CheckException` creates an apparent "back-edge" `p.0 >= bid.0` to the handler — but this is NOT a loop back-edge, it is an exception arc.

Using `TerminatorOnlyPredMap` eliminates these phantom arcs. The back-edge detection then sees only genuine loop back-edges (block B with `Branch` or `CondBranch` targeting a lower-numbered header H), which is the correct structural invariant.

The `has_exception_handlers()` gate remains necessary: a TypeGuard inside a genuine `try` block has different semantics when hoisted out of the loop (if the exception handler RESETS a type assumption, hoisting the guard changes when the type check runs). The terminator-only pred map alone is not sufficient — you also need to not be inside a handler region.

**LICM already solved this:** `licm.rs:95-122` explains the design. It does not bail on `has_exception_handling` and instead trusts `is_hoistable` (which excludes all exception-adjacent ops). This arc applies the same insight to block_versioning and type_guard_hoist.

### IV Strength Reduction

The SCEV analysis already enforces the critical soundness rules (`scev.rs:39-56`):
- `AddRec` only created when `no_signed_wrap` attr is present (set by `range_devirt` for unit-step increments, `range_devirt.rs:475`)
- Degree-2 recurrences → `Unknown` (prevents the loop-IV accumulator OOM hazard, bug #15)

The IV-SR pass only transforms when SCEV returns a sound `AddRec`. The new accumulator induction variable `acc += stride` is a new `AddRec` over the loop. It may wrap if `stride` is large and the trip count is large — `no_signed_wrap` should be set on the new `Add(acc, stride)` only if `stride` is a small bounded constant (same logic `range_devirt` applies: safe for unit stride, not for unbounded stride). For non-constant or large stride, emit without `no_signed_wrap` and let SCEV classify the new IV as `Unknown` conservatively.

---

## 5. Legacy This Arc Deletes

### Stale Comments

- `block_versioning.rs:373-377`: the comment "Because the pass only proceeds when `has_exception_handling == false`, the cached full-CFG `PredMap` has no exception edges and coincides with the terminator-only predecessor relation" — this was the justification for the original bail-out. With the gate changed and the pred map switched, the comment is not only stale but actively misleading. DELETE.

- `type_guard_hoist.rs:94-98`: identical justification comment. DELETE.

### No Dual Paths

There is no legacy code path to delete for the gate changes — the passes simply had dead code branches (`if func.has_exception_handling { return; }`). The "legacy" being replaced is the incorrect over-conservative gate, replaced by the structurally correct narrower gate.

For IV-SR: `strength_reduction.rs:101-113` contains the deferred `FloorDiv`/`Mod` comments ("deferred to Phase 3"). After IV-SR lands and the block-mutation API is proven in production, `Phase 3` of `strength_reduction.rs` (the `//` and `%` power-of-two cases) should also be completed — those two `continue` cases become functional rewrites. This is a follow-on cleanup, not part of this arc.

---

## 6. Test Plan

### Rust Unit Tests

**loop_unroll.rs** — add to the existing `#[cfg(test)]` module:

Test name: `test_unroll_survives_check_exception_body`
Setup: Use `build_test_loop(0, 4, 1, 1)` (from the existing helper at line 1215), then add a `CheckException` op to the body block with `value: AttrValue::Int(99)` and a matching entry in `func.label_id_map`. Set `func.has_exception_handling = true` (which will be true in practice). Assert `stats.ops_added > 0` — the loop IS unrolled despite `has_exception_handling == true`, because `has_exception_handlers()` returns false.

Test name: `test_unroll_bails_try_in_body`
Setup: Build same loop, add a `TryStart` op to the body block. Assert `stats.ops_added == 0` — correctly refused.

**block_versioning.rs** — add:

Test name: `test_block_versioning_fires_with_check_exception`
Build a function with a TypeGuard on an int-producing value, no TryStart/TryEnd, but with `has_exception_handling = true` (set by adding a `CheckException` op). Assert `stats.values_changed > 0`.

Test name: `test_block_versioning_false_backedge_from_check_exception`
Build a function where `CheckException` creates an exception edge from a high-ID block to a low-ID block that would formerly be misidentified as a loop header. Assert that the non-loop block containing a TypeGuard IS versioned (would have been skipped due to phantom loop-header misidentification with the old full PredMap).

**type_guard_hoist.rs** — add:

Test name: `test_type_guard_hoist_fires_with_check_exception`
Build a loop function with `has_exception_handling = true` (no TryStart), loop-invariant TypeGuard in body. Assert `stats.ops_added > 0`.

**analysis/mod.rs** — add:

Test name: `test_terminator_only_pred_map_excludes_exception_edges`
Build a function with a `CheckException` op with `value: 5` and an entry in `label_id_map` mapping to block B5. Assert `TerminatorOnlyPredMap` result does NOT contain B5 as a predecessor of the handler block, while `PredMap` does.

### Differential Test Shapes (Python snippets)

All tests must be verified against CPython 3.12, 3.13, 3.14 on all backends (native, WASM, LLVM).

**Shape 1 — Counted loop with CheckException, no try block:**
```python
def sum_range(n):
    total = 0
    for i in range(n):
        total += i
    return total
assert sum_range(4) == 6
assert sum_range(0) == 0
assert sum_range(1) == 0
```
Expected: loop_unroll fires (trip count 4 ≤ cap), SCCP folds to constant. Post-unroll TIR should have no loop blocks.

**Shape 2 — Loop with try block inside (unroll must NOT fire):**
```python
def safe_sum(items):
    total = 0
    for i in range(4):
        try:
            total += items[i]
        except IndexError:
            pass
    return total
assert safe_sum([1, 2, 3, 4]) == 10
assert safe_sum([1]) == 1
```
Expected: loop_unroll does NOT fire. Function runs correctly.

**Shape 3 — TypeGuard hoisting across CheckException:**
```python
def typed_loop(x, n):
    result = 0
    for i in range(n):
        if isinstance(x, int):
            result += x + i
    return result
assert typed_loop(5, 4) == 26
assert typed_loop("s", 4) == 0
```
Expected: `isinstance` → TypeGuard, hoisted to preheader. `stats.ops_added > 0` in type_guard_hoist.

**Shape 4 — Adversarial: exception mid-unroll (CheckException in unrolled body must propagate correctly):**
```python
def raising_range(n, bad):
    total = 0
    for i in range(n):
        if i == bad:
            raise ValueError(f"bad {i}")
        total += i
    return total

try:
    raising_range(4, 2)
    assert False
except ValueError as e:
    assert "bad 2" in str(e)
assert raising_range(4, 99) == 6
```
Expected: if unrolled, the unrolled copy for iteration i=2 raises, propagates to function exit via CheckException, CPython-identical behavior.

**Shape 5 — BigInt correctness across loop (must not regress):**
```python
def sum_large(n):
    total = 0
    for i in range(n):
        total += i
    return total
assert sum_large(4) == 6
# 1<<60 should stay bigint-correct even if IV strength reduction fires
x = 1 << 60
result = sum_large(2)  # small trip, exercises unroll path
assert result == 1
```

**Shape 6 — IV strength reduction: stride multiply:**
```python
def stride_sum(n, stride):
    total = 0
    for i in range(n):
        total += i * stride
    return total
assert stride_sum(5, 3) == 30   # 0+3+6+9+12
assert stride_sum(4, 0) == 0
assert stride_sum(1, 7) == 0
```
Expected: `i * stride` converted to accumulator increment pattern. Perf gate: stride_sum on large n must be measurably faster than a naive multiply-each-iteration loop.

**Shape 7 — Block versioning with CheckException:**
```python
def versioned_add(x, y):
    if isinstance(x, int) and isinstance(y, int):
        return x + y
    return 0
assert versioned_add(3, 4) == 7
assert versioned_add(3, "s") == 0
assert versioned_add(1 << 60, 1) == (1 << 60) + 1  # BigInt path
```

**Shape 8 — range_devirt fires correctly (regression guard):**
```python
def counted_loop(n):
    s = 0
    for i in range(n):
        s += i
    return s
assert counted_loop(100) == 4950
assert counted_loop(0) == 0
assert counted_loop(1) == 0
```
Verify with `TIR_OPT_STATS=1` that `range_devirt: 1 values_changed` is in the output.

---

## 7. Perf-Gate Plan

For each shape, measure on each `(target, profile)` pair: `(native, release-fast)`, `(native, dev-fast)`, `(wasm, release-fast)`, `(llvm, release-fast)`.

**Benchmark: sum_range / sum N=8 iterations**
- Baseline (pre-fix): full loop overhead, 8 iterations, ~N × loop overhead
- Post-fix: straight-line unrolled code, SCCP folds to single ConstInt
- Expected: ≥2× speedup on native release-fast for trip_count ≤ 8
- Perf gate: result must be FASTER than CPython 3.12 on same workload

**Benchmark: stride_sum / N=10_000_000 iterations with constant stride**
- Baseline: Mul each iteration
- Post-fix: Add each iteration (1 ALU op vs 2)
- Expected: ≥10% faster for large N
- Gate: must maintain ≥1× CPython baseline

**Benchmark: typed_loop / TypeGuard in loop N=1_000_000**
- Baseline: TypeGuard executed N times
- Post-fix: TypeGuard executed 1 time (hoisted to preheader)
- Expected: 5-10% faster on pure compute-heavy loops
- Gate: ≥CPython

**Measurement tooling:** `tools/safe_run.py --rss-mb 2048 --timeout 30 -- python3 -m molt bench <file>`. Use `MOLT_SESSION_ID=bench-loop` to isolate. Use `TIR_OPT_STATS=1` to verify pass fire counts. Run 3 times, report median.

---

## 8. Risk, Rollback, and Dependency Notes

### Blocked By

None for Phase 1 (gate fixes + TerminatorOnlyPredMap). These are conservative narrowing changes on existing passes.

IV strength reduction (Phase 3) depends on:
- SCEV being registered and producing correct `AddRec` for the target loop shape (LANDED, S6)
- LoopForest analysis producing correct natural-loop bodies (LANDED, S1)
- Phase 1 (gate fixes, so the downstream loops are actually being processed)

### Unblocks

Phase 1 unblocks:
- Branchless count optimization firing on more functions (it already bails independently on `has_exception_handling` — this should be audited separately)
- Effective loop unrolling → SCCP folding → smaller code for counted loops
- Block versioning → eliminates runtime TypeGuard overhead in hot paths

Phase 3 (IV-SR) unblocks:
- Auto-vectorization recognizers (vectorize.rs) which need stride-uniform loop IVs
- Loop tiling / polyhedral analysis (polyhedral.rs) which reasons about array access patterns

### Miscompile Risks

**Risk 1 (loop_unroll):** A loop body that calls a function which may raise, and the caller function has no `TryStart` but the callee does. `has_exception_handlers()` checks the CALLER function only. This is correct: the callee's exceptions propagate via the return path, not via a handler block in the caller. The `CheckException` in the caller's unrolled body correctly propagates. Risk is LOW, already handled by the existing model.

**Risk 2 (block_versioning TerminatorOnlyPredMap):** If a handler block (reachable only via exception edge) contains a TypeGuard that should NOT be versioned, the terminator-only pred map causes it to be misidentified as having no predecessors (it has no terminator predecessors). The versioning candidate search would find it has no "any_proves" predecessors and skip it anyway. Risk is LOW — exception-only-reachable blocks would simply not get versioned.

**Risk 3 (IV-SR + BigInt):** If the IV is `MaybeBigInt` (not `RawI64Safe`), SCEV returns `Unknown` (cannot form `AddRec` without `no_signed_wrap`). IV-SR correctly skips it. The `i * stride` stays as-is. No miscompile. Explicitly verify with the `apply(f, 1<<60, 7)` shape from `representation_plan.rs`.

### Rollback

Each phase is an independent change. Rollback by reverting the specific pass file + analysis/mod.rs for TerminatorOnlyPredMap. No cross-pass state sharing means one broken pass doesn't corrupt another.

---

## 9. Phased Landing Sequence

### Phase 1: TerminatorOnlyPredMap + Gate Fixes (one atomic commit)

**Complete structural piece. Delivers: all three passes fire on CheckException-bearing functions.**

Tasks (in order, all in one commit):

- [ ] `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/analysis/mod.rs`: Add `TerminatorOnlyPredMap` variant to `AnalysisId`, add to `AnalysisId::ALL`, implement `Analysis` for `TerminatorOnlyPredMap`, update `cfg_sensitive`/`ops_sensitive` match arms
- [ ] `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/pass_manager.rs`: Add `TerminatorOnlyPredMap` to the `check!` macro in `assert_analyses_fresh` import list + macro body
- [ ] `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/loop_unroll.rs` line 250: `has_exception_handling` → `has_exception_handlers()`; update doc comment at line 14
- [ ] `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/block_versioning.rs` line 378: gate change; line 382: switch to `TerminatorOnlyPredMap`; delete stale comment lines 373-377; add import
- [ ] `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/type_guard_hoist.rs` line 90: gate change; line 100: switch to `TerminatorOnlyPredMap`; delete stale comment lines 94-98; add import
- [ ] Add unit tests for all three passes as described in Section 6
- [ ] `cargo test -p molt-backend` — all tests green, zero new warnings
- [ ] Run differential shapes 1, 2, 3, 4, 7 on native backend
- [ ] Verify `TIR_OPT_STATS=1` shows loop_unroll/block_versioning/type_guard_hoist non-zero on shapes 1/3/7

### Phase 2: range_devirt Ordering Fix (one atomic commit)

**Complete structural piece. Delivers: range_devirt fires on real for-loops; counted loop unrolling becomes effective.**

Tasks:

- [ ] Investigate via `TIR_OPT_STATS=1` + `MOLT_DUMP_IR=1` on `for i in range(4): pass` whether `iter_devirt` fires first and produces `IterNextUnboxed`, or whether the frontend already emits `IterNextUnboxed` directly
- [ ] If iter_devirt produces the shape needed by range_devirt: swap pipeline order in `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/pass_manager.rs:290-293` to `iter_devirt` before `range_devirt`
- [ ] If frontend already emits `IterNextUnboxed`: diagnose why `range_devirt` does not find the pattern; the issue may be that `GetIter` is not emitted (frontend uses a different shape) — add the missing match arm or alternative detection path
- [ ] Run differential shape 8 to confirm `range_devirt: 1 values_changed`
- [ ] Run shape 1 to confirm loop_unroll fires end-to-end
- [ ] All tests green

### Phase 3: IV Strength Reduction Pass (one atomic commit)

**Complete structural piece. Delivers: `i * stride` → accumulator IV for all loops with constant-step SCEV AddRec IVs.**

Tasks:

- [ ] Create `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/iv_strength_reduction.rs` with `pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats`
- [ ] Add `pub mod iv_strength_reduction;` to `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/mod.rs`
- [ ] Register in `build_default_pipeline` after `loop_unroll`, before `canonicalize`, as `Mutates::Cfg`
- [ ] Implement the transform as described in Section 3.6
- [ ] Unit tests: `test_iv_sr_constant_stride`, `test_iv_sr_loop_invariant_stride`, `test_iv_sr_bigint_skip` (MaybeBigInt IV → no transform)
- [ ] Differential shapes 5 and 6
- [ ] Perf gate: stride_sum(10_000_000, 3) must show improvement in `TIR_OPT_STATS` (iv_strength_reduction: N values_changed) and wall-clock measurement ≥ CPython
- [ ] `cargo test -p molt-backend` — all green, zero new warnings

### Phase 4: strength_reduction.rs Phase 3 Completion (one atomic commit)

**Complete structural piece. Delivers: `x // 2^k` → `x >> k` and `x % 2^k` → `x & (2^k-1)`.**

Tasks:

- [ ] Add block-level op-insertion helper (or reuse the pattern from `range_devirt::apply_transform` which inserts a `ConstInt` op inline) to `strength_reduction.rs`
- [ ] Implement `FloorDiv` → `Shr` and `Mod` → `BitAnd` reductions in `strength_reduction::run` (lines 101-110)
- [ ] Remove the "deferred to Phase 3" comments
- [ ] Tests: `test_floordiv_power_of_two`, `test_mod_power_of_two`, `test_floordiv_non_power_of_two_unchanged`
- [ ] Differential: `assert 100 // 4 == 25`, `assert 100 % 8 == 4`, `assert 7 // 3 == 2` (non-power, unchanged)
- [ ] All tests green

---

## Critical Implementation Notes

**On the `AnalysisId::ALL` array:** The `assert_analyses_fresh` function in `pass_manager.rs` iterates `AnalysisId::ALL` and calls a `check!` macro for each. When adding `TerminatorOnlyPredMap`, the `check!` macro body (around `pass_manager.rs:376`) also needs a new arm for `AnalysisId::TerminatorOnlyPredMap => check!(TerminatorOnlyPredMap)`. Without this, the debug self-check silently skips the new analysis.

**On `loop_unroll` and the header-1-arg requirement:** After Phase 2 fixes range_devirt firing, the loop shape will have exactly 1 block arg (the induction variable). The unroller's requirement at `loop_unroll.rs:271` (`header_block.args.len() != 1`) remains correct and unchanged.

**On the unrolled body's `CheckException` ops:** The current `unroll_candidate` function at `loop_unroll.rs:517` clones body ops verbatim (lines 596-614) via `op.attrs.clone()`. The `value: AttrValue::Int(label)` attribute on `CheckException` ops is preserved in the clone. This is correct: all clones point at the same handler label, which resolves to the same handler block outside the loop. No remapping needed.

**On Phase 3 IV-SR and Mutates::Cfg:** Adding block arguments to the loop header changes the block structure (block args are part of the block definition), so the pass is `Mutates::Cfg`. The `AnalysisManager::invalidate_cfg` will be called after it runs, clearing LoopForest, PredMap, ImmediateDoms, etc. This is correct — the loop structure is preserved but the AnalysisManager treats the function as potentially restructured.

---

## CORRECTION — empirically verified 2026-06-04 (native codegen, `MOLT_DUMP_IR`)

**The L4 gate fix is correct and sound but COMPLETELY INERT on real code. The
three passes fire 0 times BEFORE and AFTER `has_exception_handling →
has_exception_handlers()`. The premise (here and in the baton) that "the
exception gate is THE blocker disabling these passes on real code" is
INCOMPLETE — the passes were already inert for reasons INDEPENDENT of, and
predating, the exception gate.** This was found by implementing the full gate fix
(+ `TerminatorOnlyPredMap`), confirming 889 lib tests + 0 warnings, then dumping
real probes and measuring per-pass stats — all `(0,0,0)`. The implementation was
reverted (not landed) because shipping an inert prod-codegen change that implies
"loop opts re-enabled" would be false, and the block_versioning/type_guard_hoist
observation path is untested (nothing makes them fire).

Root causes, independent of the gate (these are the REAL L4 prerequisites):

1. **block_versioning + type_guard_hoist: NO `TypeGuard` ops are generated.** A
   polymorphic loop (`for x in items: total += x.value`) produces **zero**
   `TypeGuard` ops in the TIR (`grep TypeGuard` = 0, pre and post pipeline).
   Both passes exist solely to specialize/hoist `TypeGuard` ops; with none
   emitted by `type_refine` for real polymorphic-dispatch patterns, they have no
   candidates regardless of the gate. **Real prerequisite: emit `TypeGuard` ops
   for polymorphic attribute/method/operator dispatch** (in `type_refine` or the
   frontend lowering), so SBBV/guard-hoist have something to specialize.

2. **loop_unroll: TOTAL shape mismatch.** The detector requires a *canonical*
   loop — a **1-arg header** (the IV only) whose **terminator is the `CondBranch`
   with the comparison in the header**. Real `for i in range(N)` lowers to the
   OPPOSITE: a **multi-arg header** (IV + every loop-carried value, e.g. an
   accumulator `acc`) ending in a plain `Branch` to a **separate cond block** that
   holds the `Lt`. `range_devirt` does NOT canonicalize this — it only converts
   *iterator* loops (`IterNextUnboxed`), and the frontend already emits a
   **counted** loop (no `IterNextUnboxed`/`ForIter`/`GetIter` to match), so
   `range_devirt = (0,0,0)`. loop_unroll therefore fires on ~no real counted loop.
   (Both the integrated-program "swap iter_devirt before range_devirt" and this
   blueprint's "match `ForIter`" hypotheses are wrong: there is no iterator op in
   the loop at all.) **Real prerequisite: loop-shape canonicalization** (rewrite
   the frontend's `multi-arg-header → Branch → separate-cond-block` counted loop
   into the `1-arg-header + comparison-in-header` form) **AND generalize the
   detector to multi-arg headers** (thread loop-carried values through each
   unrolled iteration). This is a real, miscompile-sensitive structural arc, not
   a gate flip.

**Corrected L4 dependency order:** (a) `TypeGuard`-op generation for polymorphic
dispatch → unblocks block_versioning + type_guard_hoist; (b) loop-shape
canonicalization + multi-arg-header unroll generalization → unblocks loop_unroll;
(c) THEN the `has_exception_handlers()` gate fix + `TerminatorOnlyPredMap`, landed
WITH (a)/(b) so the passes are non-inert and the observation path is exercised by
real differential tests (the exception-mid-loop soundness case). The gate fix is a
~30-line change trivially re-applied as the last step of (c).
