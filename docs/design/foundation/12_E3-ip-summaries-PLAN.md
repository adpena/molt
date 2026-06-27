<!-- Wave-3 recon implementation plan (wf_00af7480-2ba, 2026-06-04), live-code-verified. -->

# E3: Interprocedural Escape + Purity Summaries — Complete Implementation Plan

## 1. Current State (verified against live code)

### What exists today

**`ip_summary.rs` — `FunctionSummary` (lines 22-36)**
```
pub struct FunctionSummary {
    pub is_leaf: bool,
    pub op_count: usize,
    pub return_type: TirType,
}
```
Three fields. The module docs at lines 13-14 explicitly name the missing slots: `does_not_capture_param[i]` and `is_pure`.

**`escape_analysis.rs` — `analyze()` (line 220)**
The per-function escape analysis is fully functional. `OpCode::Call` arm (line 367-369) unconditionally forces `GlobalEscape` regardless of whether the callee is known-pure. This is the precise legacy line E3 replaces.

**`alias_analysis.rs` — `AliasAnalysisResult::compute()` (line 424)**
Anchors escape analysis as the S5 points-to phase. `AliasAnalysisResult.escape` is computed by `escape_analysis::analyze(func)` (line 437). The alias oracle's `is_barrier_for`, `may_observe_slot`, and `is_stack_object` all flow from this escape map. Improving the escape map improves all three queries transitively.

**`module_phase.rs` — `run_module_pipeline()` (line 115)**
Pipeline order today (verified lines 116-178):
1. `CallGraph::build(module)` — line 116
2. `ModuleSummaries::compute(module, &call_graph)` — line 117
3. `run_inliner(module, &call_graph, &summaries, tti)` — line 120
4. `run_module_slot_promotion(module)` — line 139
5. Re-optimize changed functions via `run_pipeline` — lines 153-158
6. Rebuild `CallGraph` + `ModuleSummaries` over post-inline module — lines 172-173
7. Return `ModuleAnalysis { call_graph, summaries, changed_functions }` — line 174

`ModuleAnalysis.summaries` (`ModuleSummaries`) is returned but currently only consumed by the inliner (for `op_count` / `is_leaf`). It is NOT threaded into the per-function pipeline; there is no `Option<&ModuleSummaries>` anywhere in `alias_analysis.rs`, `escape_analysis.rs`, or `licm.rs`.

**`effects.rs` — `builtin_effects()` / `method_effects()` (lines 448, 488)**
`builtin_effects` covers `len`, `sorted`, `abs`, `min`, `max`, `sum`, `bool`, `int`, `float`, `str`, `repr`, `hash`, `chr`, `ord`, `hex`, `oct`, `bin`, `range`, `enumerate`, `zip`, `map`, `filter`, `tuple`, `frozenset`, `math.*`. `method_effects` covers immutable types: `str`, `tuple`, `frozenset`, `int`, `float`. Neither covers user-defined callees — that is the E3 gap.

**`call_graph.rs` — `CallGraph::bottom_up_order()` (line 386)**
Returns `Vec<Vec<String>>` (SCC condensation, callees before callers). Used by `ModuleSummaries::compute` today (line 65 of `ip_summary.rs`).

---

## 2. End-State Design

### 2a. New `FunctionSummary` fields

```rust
pub struct FunctionSummary {
    // existing (unchanged)
    pub is_leaf: bool,
    pub op_count: usize,
    pub return_type: TirType,

    // E3 additions
    /// For each parameter index i, true iff the function provably does NOT
    /// store or return param[i] into any heap location (does not capture it).
    /// Populated bottom-up in SCC order. Conservative default: false (unknown).
    pub does_not_capture_param: Vec<bool>,
    /// True iff the function is side-effect-free AND returns a deterministic
    /// result solely from its parameters (no observable I/O, no global reads
    /// that could change, no stores to heap). Conservative default: false.
    pub is_pure: bool,
}
```

### 2b. Summary computation: bottom-up SCC walk

`ModuleSummaries::compute` already walks `call_graph.bottom_up_order()` (ip_summary.rs:65). The E3 extension adds a new intra-function analysis for each function in the walk:

**`does_not_capture_param[i]`**: For each param `p_i` (a `ValueId` = `ValueId(i)` in the entry block), ask the existing `escape_analysis::analyze(func)` what `escape.get(&p_i)` says. If the result is `NoEscape` or `ArgEscape`, the param does not escape this function — `does_not_capture_param[i] = true`. If it is `GlobalEscape` (or absent, meaning it is not an allocation root), we must additionally check whether a callee consumes it: use `CallGraph::callees` to find every callee that takes this param as an argument and check those callee summaries (already computed, because we are bottom-up) for their `does_not_capture_param`.

Concretely for param `p_i` used at a `Call` site: if callee summary says `does_not_capture_param[j] = true` for the argument position, the param does not escape through that call even though the local escape state says `GlobalEscape`. The intra + callee-summary combination is the IP extension.

**`is_pure`**: A function is `is_pure` iff:
1. No op in the body is side-effecting (`effects::opcode_is_side_effecting`) EXCEPT `CheckException` (observation only) and `IncRef`/`DecRef` (implementation detail, not user-visible).
2. Every `OpCode::Call` in the body targets a callee whose `FunctionSummary.is_pure = true` (or the callee is an inlined-away function — its ops are directly observable).
3. No `OpCode::CallMethod` or opaque call (conservatively impure).
4. No `OpCode::Import`, `ModuleCacheGet`, `ModuleGetAttr`, etc. (module-dict reads could change — impure).
5. No `OpCode::Yield`/`YieldFrom`/generator ops.

Recursive SCCs get `is_pure = false` and `does_not_capture_param = vec![false; arity]` (bottom-lattice, conservative). The bottom-up walk propagates purity up to callers.

### 2c. Threading `Option<&ModuleSummaries>` into escape analysis

`escape_analysis::analyze` signature changes:

```rust
pub fn analyze(
    func: &TirFunction,
    summaries: Option<&ModuleSummaries>,
) -> HashMap<ValueId, EscapeState>
```

In the `OpCode::Call` arm (currently line 367-369 in escape_analysis.rs):

**Before (line 367-369):**
```rust
OpCode::Call => {
    escapes.insert(val, EscapeState::GlobalEscape);
}
```

**After:**
```rust
OpCode::Call => {
    // Can we prove the callee does not capture this argument?
    let callee_name = attr_str(&use_info.attrs, "s_value");
    let does_not_capture = callee_name
        .and_then(|n| summaries?.get(n))
        .and_then(|s| s.does_not_capture_param.get(use_info.operand_index - 1))
        // operand_index 0 is the callee itself (if bound), 1.. are args;
        // the exact offset depends on the calling convention encoding —
        // verify against `collect_call_sites` in inliner.rs
        .copied()
        .unwrap_or(false);
    if does_not_capture {
        escalate(&mut escapes, val, EscapeState::ArgEscape);
    } else {
        escapes.insert(val, EscapeState::GlobalEscape);
    }
}
```

The `does_not_capture` upgrade means: the value crossed a call boundary (ArgEscape) but the callee provably does not store it. The existing `apply()` (line 688) already treats `ArgEscape` as promotable — no change needed there.

**Similarly for `is_pure` in LICM / DCE:**

`is_hoistable` in `licm.rs` (line 61-63) currently ignores `OpCode::Call`. With summaries threaded in, an `OpCode::Call` to a `is_pure=true` callee is hoistable:

```rust
fn is_hoistable(op: &TirOp, summaries: Option<&ModuleSummaries>) -> bool {
    if super::effects::opcode_is_pure_movable(op.opcode) || op.is_plain_value_copy() {
        return true;
    }
    if op.opcode == OpCode::Call {
        let name = attr_str(&op.attrs, "s_value");
        return name
            .and_then(|n| summaries?.get(n))
            .map(|s| s.is_pure)
            .unwrap_or(false);
    }
    false
}
```

DCE: a `Call` to `is_pure=true` callee whose result is dead can be dropped (DCE already removes side-effect-free ops whose results are unused; it just needs to consult summaries for `Call`).

### 2d. Architecture of the threading

The key architectural question is: where does `Option<&ModuleSummaries>` enter the per-function pipeline?

The per-function pipeline runs via `run_pipeline(func, tti)` → `pass_manager::build_default_pipeline(tti).run(func)` (passes/mod.rs:81). The `PassManager::run` currently threads `&AnalysisManager` and `&TargetInfo`. `ModuleSummaries` is a module-level artifact, not per-function.

**Design choice**: add `Option<Arc<ModuleSummaries>>` as a field of `TargetInfo` (S2, tir/target_info.rs). The cost model and the IPO summary are both "module-phase outputs the per-function pipeline reads" — this is the correct architectural home. `TargetInfo` is already threaded to every pass via `TirPass::run(func, am, tti)`.

Alternatively, add it as a new field in `PassManager` (built once per module, shared across functions). Either works; the `TargetInfo` path is simpler (no `PassManager` API change).

---

## 3. Legacy Deleted

The unconditional `OpCode::Call → GlobalEscape` arm in `escape_analysis::analyze` (line 367-369). This is the `Call`-always-escapes barrier. After E3, this arm becomes the precise does-not-capture conditional. The name of the deleted behavior: "unconditional-Call-GlobalEscape arm".

---

## 4. Phased Landing

### Phase E3a: Extend `FunctionSummary` + bottom-up computation (pure data; no consumers yet)

Files changed:
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/ip_summary.rs` — add `does_not_capture_param: Vec<bool>` and `is_pure: bool` to `FunctionSummary`; populate in `ModuleSummaries::compute` bottom-up using `escape_analysis::analyze` on each function (summaries for callees are already computed when we reach a caller, because the walk is bottom-up over the SCC condensation)
- All existing `FunctionSummary { is_leaf, op_count, return_type }` struct literals in tests need the two new fields (or derive `Default` and use `..Default::default()`)

Unit tests:
- `leaf_pure_fn_is_marked_pure`: a function with only arithmetic ops and a `ConstNone` return is `is_pure=true`
- `call_to_pure_fn_is_pure`: `a` calls `b` where `b` is pure → `a` is pure iff `a` has no other impure ops
- `call_to_impure_fn_is_not_pure`: `a` calls `b` where `b` has a `StoreAttr` → `a.is_pure=false`
- `does_not_capture_local_alloc`: a function that takes a param and only passes it to `len()` (borrowing) → `does_not_capture_param[0]=true`
- `does_not_capture_through_pure_callee`: param passed to `is_pure` callee that doesn't capture → still `does_not_capture`
- `recursive_scc_gets_bottom`: both functions in a mutual-recursion SCC get `is_pure=false, does_not_capture=vec![false;arity]`

This phase is **additive** — zero behavior change, all new fields are conservatively false at first until the computation is correct.

### Phase E3b: Thread summaries into escape analysis; delete unconditional-Call-GlobalEscape arm

Files changed:
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/escape_analysis.rs` — change `analyze(func)` to `analyze(func, summaries: Option<&ModuleSummaries>)`; replace `OpCode::Call → GlobalEscape` (line 367-369) with the precise `does_not_capture_param` conditional
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/alias_analysis.rs` — update `AliasAnalysisResult::compute(func)` (line 424) to pass `summaries` through; signature becomes `compute(func, summaries: Option<&ModuleSummaries>)`; `AliasAnalysis::compute` in the `Analysis` impl (line 724) needs the summaries source — this is where the `TargetInfo` field enters
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/target_info.rs` — add `pub ip_summaries: Option<Arc<ModuleSummaries>>` field; populate in `module_phase::run_module_pipeline` before the per-function pipeline runs (the rebuild at line 172-173 produces the final summaries; set them on `tti` before `compile_module_parallel` is called)
- All call sites of `escape_analysis::analyze` (there are 3: in `alias_analysis.rs`, in `escape_analysis::run`, and in any test that calls `analyze` directly) need the new argument

**Soundness argument**: The escape analysis is monotone — it only ever moves values UP the lattice (NoEscape → ArgEscape → GlobalEscape). Adding `does_not_capture_param` summaries can only prevent a value from escalating from ArgEscape to GlobalEscape at a Call site. If the summary is wrong (falsely claims does-not-capture), a value that should be GlobalEscape is left ArgEscape. ArgEscape values are stack-promoted and RC-stripped. A false-positive `does_not_capture` therefore causes a use-after-free. The computation is conservative-correct because:
1. Recursive SCCs get `does_not_capture=false` (conservative bottom)
2. Unknown/opaque callees (no summary) fall through to `false` → GlobalEscape preserved
3. The `does_not_capture` proof requires `EscapeState::NoEscape || EscapeState::ArgEscape` at the local escape analysis of the callee's param, which means the callee's own escape analysis also proved non-capture

### Phase E3c: Thread summaries into LICM + DCE for pure user calls

Files changed:
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/licm.rs` — extend `is_hoistable` to consult `tti.ip_summaries` for `OpCode::Call`; LICM pass receives `tti` already via `TirPass::run`
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/dce.rs` — extend dead-call elimination to drop `OpCode::Call` to `is_pure=true` callee when result is unused

Unit tests for LICM:
- `pure_user_call_in_loop_is_hoisted`: a loop containing `result = pure_fn(arg)` where `arg` is loop-invariant and `pure_fn` has `is_pure=true` in summaries → `result` hoisted to preheader
- `impure_user_call_not_hoisted`: same structure but `pure_fn` has a `StoreAttr` → not hoisted

---

## 5. Observability Instrument

`MOLT_EA_STATS=1` (mirrors `MOLT_INLINE_STATS`): emit to stderr at the end of `run_module_pipeline`, after the final summaries rebuild:

```
[E3] module '{name}': {N} call sites downgraded from GlobalEscape to ArgEscape
     ({M} alloc roots newly promotable), {K} pure user calls eligible for LICM/DCE
     ({L} functions marked is_pure, {P} params marked does_not_capture across {Q} fns)
```

This is the "firing instrument" — an unexpectedly zero count is immediately visible instead of silently non-firing (the lesson from `L4`/`needs_inlining`/`promotion`).

---

## 6. Differential Python Test Shapes

All shapes must be byte-identical vs CPython 3.12 / 3.13 / 3.14 on native + WASM:

```python
# Shape 1: basic does-not-capture → stack promotion across a user call
def identity(x): return x  # does_not_capture_param[0]=true, is_pure=true

def f():
    class Box: pass
    b = Box()
    identity(b)  # b should not escape through identity; stack-promotable
    return 42

print(f())  # 42
```

```python
# Shape 2: pure user call hoisted by LICM
def square(x): return x * x

def bench(n):
    result = 0
    k = 3
    for i in range(n):
        result += square(k)  # invariant: k doesn't change; hoist square(k)
    return result

print(bench(1000))  # 9000
```

```python
# Shape 3: adversarial — capture through a nested store
def capturer(lst, x):
    lst.append(x)  # x IS captured — list.append stores it

def f():
    class Point: pass
    p = Point()
    out = []
    capturer(out, p)  # p escapes through capturer's append
    return out

r = f()
print(type(r[0]).__name__)  # 'Point'
```

```python
# Shape 4: recursive SCC gets conservative bottom (no promotion)
def even(n): return True if n == 0 else odd(n - 1)
def odd(n): return False if n == 0 else even(n - 1)

class Box: pass
def g():
    b = Box()
    even(b)  # b passed into recursive cycle — must stay GlobalEscape
    return 0

print(g())  # 0 (no crash from UAF)
```

```python
# Shape 5: BigInt correctness through a pure call
def double(x): return x * 2
print(double(1 << 60))  # 2305843009213693952 — must not truncate
```

```python
# Shape 6: exception propagation through a pure call
def safe_div(a, b): return a // b

try:
    safe_div(1, 0)
except ZeroDivisionError:
    print("caught")  # byte-identical vs CPython
```

---

## 7. Perf Gate

`bench_sum` is already 3.8× faster than CPython (module_slot_promotion). E3 targets a measurable stack-promotion improvement on class-heavy code:

- Create a benchmark `bench_points.py`: construct N `Point(x, y)` objects locally, call `distance(p)` (a pure function), accumulate. Before E3: `p` escapes through `distance` call → heap-alloc. After: `does_not_capture_param[0]=true` → stack-alloc, RC-free.
- Perf gate: `bench_points` must be ≥ 1.1× faster than CPython on native release-fast. If it does not fire (stack-promotion not happening), `MOLT_EA_STATS=1` exposes the zero-downgrade count.

---

## 8. Essential Files (absolute paths)

- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/ip_summary.rs` — `FunctionSummary`/`ModuleSummaries`; add `does_not_capture_param` + `is_pure`; E3a target
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/escape_analysis.rs` — `analyze()` line 220; `OpCode::Call` arm lines 367-369 (the deleted legacy); E3b target
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/alias_analysis.rs` — `AliasAnalysisResult::compute` line 424; threads summaries to escape; E3b target
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/module_phase.rs` — `run_module_pipeline` line 115; pipeline integration + `tti.ip_summaries` population; E3a/b target
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/licm.rs` — `is_hoistable` line 61; E3c target
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/dce.rs` — dead `Call` elimination; E3c target
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/effects.rs` — `builtin_effects`/`method_effects`; the existing oracle E3 extends to user-defined callees via summaries
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/call_graph.rs` — `CallGraph::bottom_up_order` line 386; the traversal order E3a requires
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/mod.rs` — module declaration; E3b pass registration if needed
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/analysis/mod.rs` — `AnalysisId` enum lines 66-101; E3b may add `IpSummaries` slot if summaries are cached as an analysis rather than threaded via `TargetInfo`
