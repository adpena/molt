<!-- Foundation blueprint (architect swarm wf_18b24759-006, 2026-06-04). Arc: E1 activation: route codegen through the inlined TirModule + retire the SimpleIR inliner -->

# TIR Inliner Activation (E1 Phase e): Retiring SimpleIR Dual-Path

## 1. Precise Problem Statement

The TIR inliner (`tir/passes/inliner.rs`) is complete and sound through phase c (phases a/b `f14b196ce`, hardening `951938075`, phase-c `6d9962a98`). It is called by `run_module_pipeline` (`tir/module_phase.rs:110-126`). But that function is called in exactly two places: its own unit tests, and `compute_leaf_functions_via_call_graph` (`native_backend/simple_backend.rs:349-360`) — which builds the call graph for the leaf set and **drops the inlined `TirModule`** without using the inlined bodies.

All three backends compile un-inlined code today:

- **Native (Cranelift)**: SimpleIR → TIR (per function, `lower_to_tir`) → TIR pipeline → `lower_to_simple_ir` → SimpleIR → `inline_functions` (`passes.rs:290`) → Cranelift.
- **WASM**: identical per-function TIR roundtrip at lines `wasm.rs:2117-2143`, then `crate::inline_functions` at `wasm.rs:2159-2162`.
- **LLVM**: per-function TIR lift + pipeline at `simple_backend.rs:2739-2752`, no inlining call at all (LLVM relies on its own `-O2` IPO; no molt-level inlining is ever done on the LLVM path today).

The **dual-path legacy** is `passes::inline_functions` and `passes::is_inlineable_with_limit`, which do string-rename inlining on SimpleIR after the TIR roundtrip. They have no SSA representation, no exception-label safety, no call graph (flat pass over a vec, not bottom-up), no cost-model-gated budgets on a per-callee basis, and no type-refinement re-run on merged callers. The TIR inliner is strictly superior in every dimension.

**Why this is load-bearing for the 5-year goals:** Inlining is the prerequisite for every downstream IPO pass — IPSCCP (E4), monomorphization (E5), IP-escape (E3), and SROA. Without it, every function boundary is opaque to constant propagation and type specialization. The `call_internal` count in representative programs (see Section 7) shows 30-60% of all calls are statically devirtualizable leaves. On the performance side, call overhead (10 cycles modeled in `tti.call_overhead`) plus the dynamic-dispatch cost at runtime boundaries means even a medium-density program leaves 20-40% raw performance on the table from uneliminated calls alone.

---

## 2. End-State Architecture (no workarounds)

One inliner. One decision point. One consumption path per backend.

```
[Frontend: FunctionIR vec]
          |
          | lower_to_tir (per function, parallel-safe)
          v
[TirModule — pre-inline]
          |
          | run_module_pipeline (single thread, module-scope)
          |   1. CallGraph::build
          |   2. ModuleSummaries::compute
          |   3. run_inliner (bottom-up, cost-model via tti)
          |   4. Rebuild CallGraph + Summaries (post-inline leaf set)
          v
[TirModule — post-inline, every body fully type-refined]
          |
    ------+--------+----------+
    |              |          |
    | native       | WASM     | LLVM
    v              v          v
lower_to_simple  lower_to_   try_lower_tir_to_llvm
(TIR→SimpleIR)   wasm        (TIR→LLVM IR directly,
    |              |          no SimpleIR roundtrip needed)
    v              v          v
Cranelift       WASM        LLVM opt+emit
    
[leaf set from ModuleAnalysis.leaf_functions()
 describes THE EMITTED program — post-inline]
```

`passes::inline_functions` and `passes::is_inlineable_with_limit` are deleted. `compute_leaf_functions_via_call_graph` is deleted (leaf set comes from the `ModuleAnalysis` returned by `run_module_pipeline`).

---

## 3. Data Structures and IR Constructs

### 3.1 What exists and is complete

- `TirModule { name: String, functions: Vec<TirFunction> }` — `tir/function.rs`
- `run_module_pipeline(module: &mut TirModule, tti: &TargetInfo) -> ModuleAnalysis` — `tir/module_phase.rs:110`
- `ModuleAnalysis { call_graph: CallGraph, summaries: ModuleSummaries }` with `.leaf_functions() -> BTreeSet<String>` — `tir/module_phase.rs:72-88`
- `lower_to_simple_ir(func: &TirFunction) -> Vec<OpIR>` — `tir/lower_to_simple.rs:159` (the TIR→SimpleIR back-conversion)
- `repr_by_value_for(tir_func, Some(&vr))` — `representation_plan.rs` (value-keyed Repr, pure TIR `ValueId` carrier authority; no `FunctionIR` or `SimpleValueNames` bridge participates in the proof)
- `LlvmReprFacts::build(tir_func)` — `representation_plan.rs` (pure TIR value-keyed LLVM Repr facts; container dispatch no longer routes through the ValueId→SimpleIR-name bridge)
- The held LLVM driver-wiring patch at `/Users/adpena/.claude/projects/-Users-adpena-Projects-molt/memory/phase_e_e1_llvm_driver_wiring.patch` — complete, already handles the `function_repr_facts` name-keyed rebuild and the externs-out/module-run/reassemble pattern.

### 3.2 What must be built

**`lower_to_tir_module`** — a new function in `tir/lower_from_simple.rs` (or the relevant backend module) that takes `&[FunctionIR]` and returns a `TirModule` with extern slots preserved. This consolidates the repeated lift-loop that exists in three backends:

```rust
pub fn lower_function_vec_to_tir_module(
    functions: &[FunctionIR],
    module_name: &str,
) -> (TirModule, Vec<bool>) // (module, is_extern per function, aligned)
```

The `Vec<bool>` carries the `is_extern` flag in alignment with the returned module's `functions` vec so the caller can reconstruct the extern/non-extern partition after the module pipeline runs.

**`ReprFactsForInlinedTir`** — no wrapper is needed. `LlvmReprFacts::build(tir_func)` now consumes the post-inline `TirFunction` directly, so fresh `ValueId`s introduced by inlining are classified from that merged body’s own value-range. Container dispatch no longer participates in `LlvmReprFacts`; LLVM len specialization resolves from refined TIR type instead of a `ValueId -> SimpleIR name -> container kind` bridge.

---

## 4. Exact Files to Create/Modify

### 4.1 `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/lower_from_simple.rs`

**Add** `pub fn lower_functions_to_tir_module(functions: &[FunctionIR], module_name: &str) -> (TirModule, Vec<bool>)` — iterates the slice, calls the existing `lower_to_tir(f)` for non-extern functions (building the TirFunction), builds a `TirModule`, and returns the aligned `is_extern` vec. Extern `FunctionIR` entries produce a TirFunction that is NOT added to the module (externs are not inlineable and their bodies are empty); the `is_extern` bool at the corresponding position is `true`.

This function is the consolidation point for the three backends' identical lift patterns.

### 4.2 `/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/simple_backend.rs`

**The native Cranelift path** — three changes:

**Change 1** (lines ~2619-2630): Replace the pre-split `inline_functions` call with a no-op comment block. The TIR module phase (below) is the new inliner.

**Change 2** (lines ~2483-2598): The main per-function TIR parallel loop (the rayon workers that do `lower_to_tir` → `run_pipeline` → `lower_to_simple_ir`) currently produces updated `func_ir.ops` (SimpleIR). After this arc, it produces an updated `TirModule`. The structural change is:

1. Lift all non-extern `FunctionIR` to TIR in parallel (using rayon, same as today — `refine_types` + `run_pipeline` + `refine_types`). Store the resulting `Vec<TirFunction>` in a `Vec<(usize, TirFunction)>` (original index preserved).
2. Assemble into a `TirModule` (sequential, single-threaded — the module phase must run single-threaded because `run_inliner` takes `&mut TirModule`).
3. Call `run_module_pipeline(&mut tir_module, &native_tti)` — returns `ModuleAnalysis` with the post-inline leaf set.
4. Back-convert each function: for each `TirFunction` in the post-inline module, call `lower_to_simple_ir(&tir_func)` and write the result into `ir.functions[original_idx].ops`. This replaces the current `lower_to_simple_ir` call inside the rayon workers.
5. Replace the `compute_leaf_functions_via_call_graph` call and its `BTreeSet` with `module_analysis.leaf_functions()`.

**Change 3** (lines ~349-360): Delete `compute_leaf_functions_via_call_graph`. The leaf set is now the `ModuleAnalysis.leaf_functions()` returned by step 3 above. Remove the function entirely.

**Change 4**: Delete the `inline_functions` call at line ~2625. The module phase inlines.

### 4.3 `/Users/adpena/Projects/molt/runtime/molt-backend/src/wasm.rs`

**The WASM path** — mirror the native structural change at `wasm.rs:2087-2162`:

1. The existing per-function TIR loop at lines 2092-2154 (with cache hit/miss) continues to run per-function refine+pipeline. After the loop, instead of writing back `func_ir.ops` immediately, collect the post-pipeline `TirFunction`s.
2. Assemble into a `TirModule`.
3. Call `run_module_pipeline(&mut tir_module, &TargetInfo::wasm_release_fast())`.
4. Back-convert each post-inline `TirFunction` via `lower_to_simple_ir` → update `func_ir.ops`.
5. Delete the `crate::inline_functions(...)` call at line 2159-2162.

**LIR fast path**: The `prepare_lir_wasm_fast_output(&tir_func)` call that
produces `lir_fast_outputs` currently runs inside the per-function loop using the
pre-inline `tir_func`. After the module phase, the `tir_func` has been
potentially modified (inlined callers have new ops). The LIR fast path must be
called on the **post-inline** `TirFunction`. Move the call to after the module
phase back-conversion, using the post-inline `TirFunction`; no SimpleIR companion
participates in value-carrier proof.

### 4.4 `/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/simple_backend.rs` (LLVM branch, lines ~2700-2840)

Apply the held patch at `/Users/adpena/.claude/projects/-Users-adpena-Projects-molt/memory/phase_e_e1_llvm_driver_wiring.patch` with one addition: remove the diagnostic block (lines 59-102 of the patch diff, the `TEMP DIAGNOSTIC` block that writes `phase_e_diag/` artifacts). The diagnostic was correct for debugging but must not ship to production.

The patch correctly handles:
- Separating externs into their own vec before the module phase.
- Calling `run_module_pipeline`.
- Reassembling `tir_funcs` from externs + post-inline non-externs.
- Building `function_repr_facts` by name-keyed lookup (not positional zip) so inlined callers whose `ValueId` space grew still get a correctly-seeded `LlvmReprFacts`.

**Pre-existing LLVM e2e link failure** (`molt_app_resolve_intrinsic` undefined symbol): this is a pre-existing gap in the LLVM lane — the LLVM path never emits the per-app intrinsic resolver that `ddc4ff73b` added for the native path. This arc does NOT fix that gap (it is a separate, pre-existing issue). The LLVM inliner activation lands correctly despite it; the link failure only affects e2e binary linking, not correctness of the inlining itself.

### 4.5 `/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs`

**Delete** `pub fn inline_functions(ir: &mut SimpleIR, tti: &TargetInfo)` (lines 290-~480) and its helper `fn is_inlineable_with_limit(func: &FunctionIR, defined_functions: &BTreeSet<&str>, limit: usize) -> bool` (lines ~255-285). No callers will remain after Changes 4.2 and 4.3 above.

---

## 5. Soundness Argument

### 5.1 The TIR inliner is already sound

The three structural invariants hold by construction:

1. **SSA**: Every splice is followed by `verify_function` in tests; the merged body is fully type-refined before it leaves `run_inliner` (`inliner.rs:1169-1172`). A splice that produced invalid SSA panics — no silent corruption.

2. **REFCOUNT**: The `+0 borrowed` calling convention is preserved verbatim; the `call_site_has_arg_incref` guard (lines 337-348) refuses any site where the caller does `IncRef(arg)` in the ≤2 ops before the `Call`, conservatively correct.

3. **LOOP METADATA**: All five loop maps (`label_id_map`, `loop_roles`, `loop_pairs`, `loop_break_kinds`, `loop_cond_blocks`) are transferred with remapped keys (`inliner.rs:542`).

### 5.2 The leaf-set soundness argument

The native backend skips the recursion guard at call sites targeting `leaf_functions()` members. After this arc, the leaf set comes from `ModuleAnalysis.leaf_functions()` which is built by `CallGraph::build` over the **post-inline TirModule** (the second `CallGraph::build` at `module_phase.rs:120`). A function that had a `Call` op inlined away is now a leaf in the post-inline call graph — correctly, because the emitted code has no call from it. A function that still retains calls (any callee over budget, or a recursive or handler-bearing callee) still has call edges and is not a leaf. The leaf set describes exactly the emitted program. This is structurally identical to the sound contract the prior `compute_leaf_functions_via_call_graph` upheld — but that function's comment (lines 328-342) explicitly documented the unsoundness of using it with the inlined module ("the leaf set MUST describe the program codegen actually produces"), which this arc resolves.

### 5.3 Repr/trusted-unbox safety

`repr_by_value_for` is derived from the post-inline `TirFunction`'s own SSA value-range analysis (`representation_plan.rs:321-322`, `value_range_for(tir_func)` on the post-inline body). Fresh `ValueId`s introduced by the splice are classified through `Repr::default_for(TirType)` — which floors `int` to `MaybeBigInt` (boxed, BigInt-correct). They are only promoted to `RawI64Safe` if `ValueRangeResult::fits_inline_int47` proves them. No `RawI64Safe` promotion can be introduced by inlining alone; the only proof is a range bound from the value-range analysis, which runs fresh on the merged body. The `apply(f, 1<<60, 7)` bigint test cannot regress.

### 5.4 First cut is conservative

The phase-c inliner conservatively refuses:
- Any callee with `has_exception_handlers()` (true `try/except` or generator state regions)
- Any recursive SCC member (self-calls or cycles)
- Any callee over `tti.inline_budget(&callee.name)` ops
- Any callee with a generator/async opcode
- Any callee whose entry block has predecessors

Every refusal leaves the `Call` intact. This is never a miscompile; at worst it forgoes an optimization. Phase d (cost / fixed-point / multi-site) relaxes some budgets; phase e (handler-bearing callees) removes the handler restriction. Both are separate later arcs.

---

## 6. Legacy This Arc Deletes

| What | File | Lines (current) |
|---|---|---|
| `inline_functions` (SimpleIR inliner) | `passes.rs` | 290-~480 |
| `is_inlineable_with_limit` | `passes.rs` | ~255-285 |
| `compute_leaf_functions_via_call_graph` | `native_backend/simple_backend.rs` | 349-360 |
| The comment block explaining why `run_module_pipeline` is NOT used for leaf detection | `native_backend/simple_backend.rs` | 328-342 |
| The `inline_functions(...)` call on the native path | `native_backend/simple_backend.rs` | 2624-2631 |
| The `crate::inline_functions(...)` call on the WASM path | `wasm.rs` | 2159-2162 |

After deletion, the only inlining logic in the compiler is `tir/passes/inliner.rs` and `tir/module_phase.rs`. No dual source of truth.

---

## 7. Test Plan

### 7.1 Rust unit tests (new, in `tir/passes/inliner.rs`)

All existing tests pass. Add:

**`test_wasm_path_calls_module_pipeline`** — build a two-function `TirModule` (a leaf and its caller), run `run_module_pipeline`, verify the caller's body has zero `Call` ops and the returned `ModuleAnalysis.leaf_functions()` contains the caller.

**`test_native_leaf_set_post_inline`** — after inlining the leaf into the caller, `ModuleAnalysis.leaf_functions()` must contain the caller. Without inlining the caller retained a `Call` and was not a leaf.

**`test_repr_fresh_values_are_maybebigint`** — after inlining a simple `fn add(a, b): return a+b` callee into a caller, call `repr_by_value_for(tir_func, Some(&vr))` with `vr` from `value_range_for`. The fresh cloned ValueIds for `a+b` result must floor to `MaybeBigInt` unless the value-range proves them in range.

**`test_observation_only_callee_inline_refcount`** — inline an observation-only callee (has `CheckException`, no handlers), verify no unbalanced `IncRef`/`DecRef` vs the un-inlined baseline using `verify_function`.

### 7.2 Differential test shapes (Python snippets, `tests/molt_diff.py`)

These cover the primary correctness axes. All must produce CPython 3.12/3.13/3.14 byte-identical output on native, WASM, LLVM, Luau.

**Basic inlining:**
```python
def add(a, b): return a + b
print(add(3, 4))           # 7
print(add(10, 20))         # 30
```

**BigInt boundary (the critical non-regression):**
```python
def apply(f, x, y): return f(x, y)
def mul(a, b): return a * b
print(apply(mul, 1 << 60, 7))   # 8317084599235823616 -- bigint path
print(apply(mul, 3, 4))         # 12 -- inline path
```

**Exception observation-only callee (phase c path):**
```python
def inner(x):
    return x + 1
def outer(lst):
    return [inner(v) for v in lst]
print(outer([1, 2, 3]))         # [2, 3, 4]
```

**Exception propagation through inlined body:**
```python
def div(a, b): return a // b
def safe_div(a, b):
    try:
        return div(a, b)
    except ZeroDivisionError:
        return -1
print(safe_div(10, 2))   # 5
print(safe_div(10, 0))   # -1
```
Note: `div` is a handler-free callee (observation-only); `safe_div` has a handler and is NOT inlined into (it is the caller with a handler, not a callee with one). `div` is inlined into `safe_div`. The exception edge in the inlined body routes to `safe_div`'s own handler via the post-call `CheckException` / continuation block. This is the primary phase-c correctness shape.

**Recursive callee (must NOT be inlined):**
```python
def fib(n):
    if n <= 1: return n
    return fib(n-1) + fib(n-2)
print(fib(10))   # 55
```
Verify `fib` is in the recursive set and the call is retained.

**Over-budget callee (must NOT be inlined):**
Write a function with more than `tti.inline_op_limit` (30) ops and verify the call survives.

**Cross-function type specialization (the key win):**
```python
def inc(x): return x + 1
def main():
    total = 0
    for i in range(100):
        total = inc(total)
    print(total)
main()   # 100
```
After inlining `inc` into `main`, the SCCP pass should fold the loop-accumulated value through the inlined `+1` body. Verify the output is correct on all backends.

**Adversarial: name collision in label ids (regression for 951938075 hardening):**
```python
def f(x):
    try:
        return x + 1
    except:
        return 0
def g(y):
    try:
        return f(y) * 2
    except:
        return -1
print(g(5))    # 12
print(g("s"))  # -1 -- but f has no handler so f is inlined into g
```
`f` is observation-only (`has_exception_handling=True`, `has_exception_handlers()=False`). It is inlined into `g` which has a real handler. Label ids from `f`'s clone must not collide with `g`'s labels. The `build_label_remap` function (`inliner.rs:731-745`) handles this; this test is the regression.

**Leaf set soundness:**
```python
def leaf(x): return x * 2      # leaf — no calls
def caller(x): return leaf(x)  # after inlining: also a leaf
print(caller(7))   # 14
```
The recursion guard in the Cranelift-emitted `caller` should be absent for `leaf` (it was already absent before), and after inlining, `caller` itself joins the leaf set, so callers of `caller` also skip the recursion guard for it.

### 7.3 Backend-specific differential tests

For WASM: run via `python3 -m molt build --target wasm --output /tmp/test_wasm test_file.py --rebuild` + `node /tmp/test_wasm`.

For LLVM: use `molt build --backend llvm`. The pre-existing link failure for `molt_app_resolve_intrinsic` is not caused by this arc; confirm LLVM inlining fires by checking the `phase_e_diag/pre_inline.txt` and `phase_e_diag/post_inline.txt` artifacts (from the patch's diagnostic block, which is removed before shipping but useful during development).

---

## 8. Perf-Gate Plan

### 8.1 Expected delta

The primary win is in programs with many small helper functions. The `call_internal` to `call_func` ratio in representative programs:

Measurement instruction: `grep -c '"call_internal"' tmp/dumped_ir.json` vs total function-call ops in the SimpleIR dump (enable via `MOLT_DUMP_IR=json`). For a typical numeric benchmark with 5-10 helpers, expect 40-70% of calls to be `call_internal` (statically devirtualizable), of which the subset in budget will be inlined.

Expected runtime improvement per benchmark category:
- **Small helper functions** (e.g. `bench_sum`, list comprehensions, numeric loops with helper): 10-25% improvement from eliminating call overhead + enabling downstream constant folding.
- **Recursive programs** (fib, ackermann): 0% change (recursive callees are excluded by `is_inlineable`).
- **Large programs with handler-bearing callees**: small improvement from observation-only inlining (phase c), larger deferred to phase e/handler-aware.

### 8.2 Perf gate methodology

For each benchmark, on each target (native, WASM, LLVM where e2e works), on each profile (release-fast primary, dev-fast secondary):

```bash
python3 tools/bench_runner.py --bench bench_sum --target native --rss-mb 2048
python3 tools/bench_runner.py --bench bench_list --target native --rss-mb 2048
```

Gate: molt must be ≥ CPython 3.12 on every benchmark × target × profile after this arc lands. A regression below CPython baseline on any benchmark is a blocker for landing. The pre-arc baseline is already measured (molt 2-5× CPython on numeric benchmarks, 1.4× baseline on the "1.4× faster" figure from the MEMORY.md — this figure was measured with the inliner DORMANT so the baseline is correct).

### 8.3 Compile-time gate

The module phase is O(n × budget × call-graph-depth) where n = number of functions and the inliner is a single bottom-up pass. For programs with 100-500 functions and a budget of 30 ops, the module phase adds at most a few percent compile time over the existing TIR parallel pipeline. Measure: `MOLT_BACKEND_TIMING=1` output; the module phase should stay under 5% of total compilation time.

---

## 9. Risk, Rollback, and Dependencies

### 9.1 Dependencies (must land before this arc)

- Phase c (`6d9962a98`) — LANDED. Observation-only callees inline correctly.
- S2 `TargetInfo` (`9ff5d2e00`) — LANDED. The `tti.inline_budget()` call is the cost-model gate.
- S4 `ModuleAnalysis` / `run_module_pipeline` (`7915b29a0`) — LANDED. The module phase is the shell this arc activates.
- S5 alias analysis (`fb574b289`) — LANDED. Not a hard dependency for inliner correctness, but the inliner's re-optimization of merged callers benefits from it.
- Repr promotion / `fits_inline_int47` (`64c2c53b8`) — LANDED. `repr_by_value_for` uses `ValueRangeResult::fits_inline_int47`.

### 9.2 What this arc unblocks

- E1 phase d (fixed-point inlining / multi-level call chains) — can use the same `run_module_pipeline` loop, extended with a fixed-point condition.
- E3 IP-escape analysis — reads the post-inline call graph.
- E4 IPSCCP — operates on inlined bodies.
- E5 monomorphization — requires inlining to specialize generic callees.
- S5 phases 2-5 (MemSSA / MemGVN / cross-block DSE / SROA) — these benefit from inlining because the IPO optimizer can see through the merged body.

### 9.3 Risk: TIR→SimpleIR back-conversion for inlined bodies

The Cranelift and WASM backends use `lower_to_simple_ir` as a final step to produce the `OpIR` vec that `SimpleBackend::compile_function` / the WASM lowering consumes. The back-conversion has been in production for the per-function path since the TIR roundtrip was introduced; this arc extends it to cover bodies with inlined content. The inlined body is fully type-refined before back-conversion (the `run_inliner` re-runs `refine_types` + `run_pipeline` + `refine_types` on every changed caller, `inliner.rs:1169-1172`), so the `SimpleValueNames` derivation and the structured-loop detection in `lower_to_simple` operate on a valid, fully-refined TIR body. The key regression risk is structured-loop detection: the back-conversion reads `loop_roles` / `loop_pairs` / `loop_break_kinds` / `loop_cond_blocks`, all of which the inliner transfers with remapped block ids. The differential tests above cover this path.

### 9.4 Risk: WASM LIR fast path

The `prepare_lir_wasm_fast_output(&tir_func)` path must be called with the
post-inline `TirFunction`, not the pre-inline one. It no longer accepts a
SimpleIR companion for representation proof: WASM computes `value_range_for` and
`repr_by_value_for` directly from that post-pipeline TIR body, matching
`LlvmReprFacts::build(tir_func)`. Container specialization is outside the
value-carrier proof and must be resolved from refined TIR facts or the surviving
name-keyed scalar plan at the consumer that still owns that namespace.

### 9.5 Risk: Megafunction splitting interaction

The native path runs `split_megafunctions` after inlining (`simple_backend.rs:2642`). After this arc, inlining can grow caller bodies. A caller that was under the megafunction threshold (4000 ops) before inlining could exceed it afterward. This is handled correctly: `split_megafunctions` runs after the module phase (and after `lower_to_simple_ir` produces the final `OpIR` vecs), so it sees the fully-merged sizes. No ordering change needed.

### 9.6 Refusal And Regression Policy

`run_inliner` has no process-global rollback lane. Inlining is controlled by
the pass legality/profitability predicates and by `non_inlinable` external
linkage facts from the module pipeline. If a regression appears, fix the
predicate, representation fact, or backend consumer that made the inline
unsound, or revert the structural change; do not reintroduce an env-controlled
no-op path.

---

## 10. Phased Landing Sequence

Each phase is a complete structural piece that can be independently committed and verified.

### Phase e-1: Native back-conversion restructure

**Files**: `native_backend/simple_backend.rs`, `tir/lower_from_simple.rs`

1. Add `lower_functions_to_tir_module` to `tir/lower_from_simple.rs`.
2. Restructure the native per-function TIR parallel loop to produce a `Vec<(usize, TirFunction)>` instead of writing `func_ir.ops` inline. After the parallel section, assemble the `TirModule` sequentially.
3. Call `run_module_pipeline(&mut tir_module, &native_tti)` — this is the activation. Store the returned `ModuleAnalysis`.
4. Back-convert each post-inline `TirFunction` via `lower_to_simple_ir` and write back to `ir.functions[original_idx].ops`.
5. Replace the `compute_leaf_functions_via_call_graph` call with `module_analysis.leaf_functions()`.
6. Delete the `inline_functions(...)` call (line ~2625).
7. **Delete** `compute_leaf_functions_via_call_graph` from the file.

Acceptance: `cargo test -p molt-backend` passes (861 tests). Differential: all basic/stdlib shapes byte-identical to CPython. Perf: ≥ pre-arc baseline on bench_sum.

### Phase e-2: WASM back-conversion restructure

**Files**: `wasm.rs`

1. Mirror the native structural change: per-function TIR loop produces `Vec<TirFunction>`, module assembled, `run_module_pipeline` called, back-conversion produces updated `func_ir.ops`.
2. Move `prepare_lir_wasm_fast_output` call to after the module phase.
3. Delete the `crate::inline_functions(...)` call at line 2159-2162.

Acceptance: `cargo test -p molt-backend` passes. WASM differential byte-identical. Perf: ≥ pre-arc baseline on WASM target.

### Phase e-3: LLVM activation (apply held patch + remove diagnostics)

**Files**: `native_backend/simple_backend.rs` (LLVM branch)

1. Apply the held patch at `/Users/adpena/.claude/projects/-Users-adpena-Projects-molt/memory/phase_e_e1_llvm_driver_wiring.patch`.
2. Remove the diagnostic block (the `TEMP DIAGNOSTIC` section that writes to `phase_e_diag/`).
3. Verify the `simple_by_name` name-keyed `function_repr_facts` rebuild is correct.

Acceptance: LLVM compilation completes without panic. The pre-existing `molt_app_resolve_intrinsic` link failure is unchanged (not caused by this arc). Differential at the LLVM IR level (not e2e binary, due to the pre-existing link gap): per-function LLVM IR contains inlined bodies for eligible callees.

### Phase e-4: passes.rs legacy deletion

**Files**: `passes.rs`

1. Delete `pub fn inline_functions`.
2. Delete `fn is_inlineable_with_limit`.
3. Update all references (there are none left after phases e-1, e-2, e-3).
4. Verify `cargo test -p molt-backend` with zero new warnings. The `#[cfg_attr(not(any(...)), allow(dead_code))]` attrs above the deleted functions are deleted too.

Acceptance: Zero compilation warnings. All tests green. `grep -r 'inline_functions\|is_inlineable_with_limit' runtime/molt-backend/src/passes.rs` returns nothing.

Phase e-4 is the arc's completion gate: the dual path is gone. Phases e-1 through e-4 together constitute the complete structural arc and must all land before any is committed as "done." Per CLAUDE.md, intermediate commits that leave `inline_functions` still present alongside the activated TIR inliner are exactly the "two parallel sources of truth" pattern that this policy prohibits. Commit phases e-1 through e-4 as one atomic arc, or commit each with an explicit baton note that the dual path is intentionally still present until e-4 lands.
