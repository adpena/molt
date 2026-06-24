<!-- Foundation blueprint (architect swarm wf_18b24759-006, 2026-06-04). Arc: E3 interprocedural escape+purity summaries + E5 function monomorphization (the Julia engine) -->

# Interprocedural Specialization Engine — Complete Implementation Blueprint

## 1. Precise Problem Statement

### Why This Is Load-Bearing

molt has **one body per function**. Every parameter enters as `DynBox` unless annotated. This forces three cascading inefficiencies:

1. **escape_analysis.rs:367** — `OpCode::Call` unconditionally marks every argument `GlobalEscape` (line 367). User functions are "opaque-impure" to the escape analysis because there are no callee summaries saying "I don't capture param[0]". Stack-allocation therefore stops dead at every user-function call boundary.

2. **sccp.rs, licm.rs, gvn.rs** — all three check `is_pure()` or `effect_free` via the effects table (effects.rs:427, 448–478), but that table only covers builtins. User-defined functions are always impure → LICM cannot hoist `y = f(x)` out of a loop even when `f` observably has no side effects, SCCP cannot propagate the return value, and CSE cannot deduplicate repeated calls.

3. **Representation specialization (the Julia axis)** — `add(a, b)` where `a` and `b` are proven `RawI64Safe` at every call site compiles the generic body that tests for BigInt and dispatches through boxed helpers. Julia solves this by cloning + compiling per-`(Repr, Repr)` tuple; this is **the** performance multiplier for numeric code.

These three gaps compose: without summaries, escape cannot cross calls (prevents stack alloc); without purity summaries, LICM/CSE cannot cross calls (prevents loop optimization); without representation specialization, every hot numeric path pays the BigInt dispatch tax.

The 5-year goal (Mojo/Julia speed but Python semantics) is flatly impossible without all three. This is the single highest-leverage arc after E1 activation.

---

## 2. The Structurally Correct Design

### 2.1 Architecture Overview

Four tightly coupled pieces, all landing in `run_module_pipeline` (module_phase.rs):

```
run_module_pipeline(module, tti)
  1. CallGraph::build(module)
  2. ModuleSummaries::compute(module, cg)      ← E3: now computes does_not_capture[i] + is_pure
  3. run_inliner(module, cg, summaries, tti)   ← E1 (existing)
  4. run_specializer(module, cg, summaries, tti)  ← E5: clone per Repr-tuple
  5. Rebuild cg + summaries over post-transform module
  6. Return ModuleAnalysis
```

Steps 2 and 4 are the two new arcs. They are additive over the existing skeleton — no structural rewrites to the surrounding machinery.

### 2.2 E3 — Bottom-up Interprocedural Summaries

**Data structure** — extend `FunctionSummary` (ip_summary.rs:24):

```rust
pub struct FunctionSummary {
    pub is_leaf: bool,
    pub op_count: usize,
    pub return_type: TirType,
    // --- E3 additions ---
    /// does_not_capture_param[i] = true iff parameter i never escapes the
    /// function (escape state is NoEscape or ArgEscape in the function's own
    /// escape map). Enables IP-escape: a caller's allocation passed as arg[i]
    /// to this function can remain stack-promotable.
    pub does_not_capture_param: Vec<bool>,
    /// True iff the function is observationally pure: no store to any heap
    /// location reachable outside the function, no I/O, no raise of an
    /// exception that didn't come from an argument. Enables CSE/LICM/DCE of
    /// call results at callers.
    pub is_pure: bool,
    /// The proven Repr of the return value, if the function has a single
    /// uniform representation at all return sites. None = unknown / DynBox.
    /// Feeds return-type backpropagation (E4) and the call-site
    /// representation upgrader in the specializer.
    pub return_repr: Option<Repr>,
    // compute_return_alias_summaries (S4 deferred): see §2.2.3
    pub return_alias: ReturnAliasSummary,
}
```

**Computation** (bottom-up over SCC condensation, exactly as today):

For each function in bottom-up order:
1. Run `escape_analysis::analyze(func)` → `HashMap<ValueId, EscapeState>`. The function's entry-block `ValueId`s are the parameter values (they are the `TirValue::id` fields of `func.blocks[func.entry_block].args`). Map each param index `i` to `args[i].id`; `does_not_capture_param[i] = escapes[param_id] != GlobalEscape`.
2. Compute `is_pure`: scan all blocks for any op that is not in `effects::opcode_is_pure_movable`, and for any `CallBuiltin` whose `builtin_effects` returns `None` or a non-pure entry. A function with any `Call` to another user function is pure iff **that callee's summary** says `is_pure=true` (bottom-up order guarantees it's already computed). A function with any `CallMethod` or opaque call is `is_pure=false` (conservative). Exception: a function with only `CheckException` (no handler) and whose body is otherwise pure is still pure — `CheckException` is a read of a flag, not a write.
3. Compute `return_repr`: collect all `Return` terminators' returned value `ValueId`s; for each, look up the function's own `repr_by_value` (if available via `representation_plan.rs`). If all return sites agree on the same `Repr`, record it; otherwise `None`.
4. Compute `return_alias` — migrate the existing `passes::compute_return_alias_summaries` (passes.rs:156) from the legacy SimpleIR layer to TIR, operating on the same bottom-up order (this is the "deferred" slot S4 reserved at 7915b29a0).

**Integration point**: escape_analysis.rs:367 — the `OpCode::Call` arm currently sets `GlobalEscape` unconditionally. After E3 lands, this arm becomes:

```rust
OpCode::Call => {
    if let Some(callee_name) = s_value_of_call_op(use_info) {
        if let Some(summary) = module_summaries.get(callee_name) {
            // use_info.operand_index gives us param index i
            let i = use_info.operand_index;  // 0-based
            if i < summary.does_not_capture_param.len()
                && summary.does_not_capture_param[i]
            {
                escalate(&mut escapes, val, EscapeState::ArgEscape);
                continue;
            }
        }
    }
    escapes.insert(val, EscapeState::GlobalEscape);
}
```

This requires threading `ModuleSummaries` into `escape_analysis::analyze`. The function signature becomes:

```rust
pub fn analyze(
    func: &TirFunction,
    summaries: Option<&ModuleSummaries>,
) -> HashMap<ValueId, EscapeState>
```

Existing callers pass `None` (backward-compatible, conservative-correct).

### 2.3 E5 — Representation Specialization

**Design principle** (Julia dispatch, not template explosion):

A *specialization* of function `f` for argument `Repr`-tuple `(r0, r1, …, rN)` is a clone of `f`'s body where:
- Entry-block arg `i` has `Repr = r[i]` injected into the clone's `repr_by_value`.
- The per-function pipeline (`run_pipeline`) runs on the clone.
- Because the pipeline now has proven `Repr` facts for the parameters, the unboxing pass, LICM, BCE, SCCP, and the RawI64Safe arithmetic paths all fire on what was previously an opaque DynBox argument.
- The clone is named `{f}__spec__{r0}_{r1}_…_{rN}` to avoid collision.
- Every call site in the module that can statically prove its argument `Repr`s match the specialization's key is redirected to the specialized clone.
- The **generic** (unspecialized) original is retained as a fallback for call sites whose argument `Repr`s are unknown or don't match any specialization.

**Cost gate (explosion prevention)**: The specializer never creates a specialization when:
- `tti.specialization_budget(func)` (a new `TargetInfo` field, see §3) is 0 (size-optimized targets like WASM set this to 0).
- The number of existing specializations of `f` already equals the budget.
- The function's op count times the number of specializations would exceed `tti.specialization_code_growth_limit` (a new absolute cap).
- The function is recursive (in `call_graph.recursive_set()`).
- The function has no `DynBox` parameter that would concretely benefit (a parameter typed `TirType::I64` whose call-site `Repr` is still `MaybeBigInt` is a viable candidate; a parameter typed `TirType::Str` is not).

**Repr-tuple key computation at a call site**:

At a `Call` op in a caller, the call site carries arguments as operand values. For each argument value `v`, look up `caller_repr_plan.repr_by_value.get(v)` (or `Repr::default_for(caller.value_types[v])` if not in the plan). The key is the `Vec<Repr>` of all parameter arguments. A key containing any `Repr::DynBox` position that the callee's generic handles fine is filtered to "only specialize on positions where the caller can prove something better than the callee's generic".

**Clone mechanics** (reuse the inliner's `clone_function_body_with_fresh_ids` primitives):

```rust
fn specialize_function(
    callee: &TirFunction,
    repr_key: &[Repr],
    caller_repr_plan: &RepresentationPlan,
    tti: &TargetInfo,
) -> TirFunction {
    let mut clone = callee.clone();
    clone.name = format!("{}__{}", callee.name, repr_key_to_suffix(repr_key));
    // Inject the proven Repr for each parameter into the clone's value_types
    // and a seed repr_by_value so run_pipeline starts with the caller's proof.
    // The pipeline's unboxing pass, SCCP, BCE then propagate from this seed.
    for (i, &repr) in repr_key.iter().enumerate() {
        if let Some(param_val) = clone.blocks[&clone.entry_block].args.get(i) {
            inject_repr_seed(&mut clone, param_val.id, repr);
        }
    }
    run_pipeline(&mut clone, tti);
    clone
}
```

`inject_repr_seed` inserts a phantom `repr_by_value` entry. The unboxing pass reads this map; the representation_plan recompute inside `run_pipeline` will see the seed and not override it with the default floor.

**Call-site rewriting**:

After building all specializations, a second pass over the module rewrites `Call` ops. For each `Call` with `s_value = f`:
1. Compute the argument `Repr` key.
2. If a specialization named `f__spec__{key}` exists in the module, replace `s_value` with that name.
3. Update the call graph edge accordingly.

**Return-type backpropagation (E4 tie-in)**:

After specializing a callee, its `return_repr` is known from the clone's final return-site `repr_by_value`. The ModuleSummaries updater (step 5 of `run_module_pipeline`) records this. The caller's pipeline re-run picks it up via the summary and may further promote the call-result value's `Repr`.

**Union splitting**:

When a parameter's `TirType` is `TirType::Union(variants)`, the specializer additionally generates union-split versions: one specialization per variant where the entry block receives a `TypeGuard` op immediately, allowing block_versioning and SCCP to fold the type test. This is a controlled form of type-directed cloning — guarded by `tti.union_split_budget`.

---

## 3. Exact Files to Create / Modify

### CREATE: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/specializer.rs`

```
Purpose: E5 — representation specialization engine.
Public API:
  pub struct SpecializerStats { pub functions_specialized: usize, pub call_sites_redirected: usize, pub clones_created: usize }
  pub struct SpecializerKey(pub Vec<Repr>);   // the Repr-tuple identifier
  pub fn run_specializer(module: &mut TirModule, call_graph: &CallGraph, summaries: &ModuleSummaries, tti: &TargetInfo) -> SpecializerStats
  fn specialize_function(callee: &TirFunction, key: &SpecializerKey, tti: &TargetInfo) -> TirFunction
  fn inject_repr_seed(func: &mut TirFunction, param_id: ValueId, repr: Repr)
  fn repr_key_to_suffix(key: &SpecializerKey) -> String
  fn is_specializable(callee: &TirFunction, key: &SpecializerKey, call_graph: &CallGraph, tti: &TargetInfo) -> bool
  fn compute_call_site_repr_key(call_op: &TirOp, caller: &TirFunction, repr_plan: &RepresentationPlan) -> Option<SpecializerKey>
  fn rewrite_call_sites(module: &mut TirModule, specialization_map: &BTreeMap<String, Vec<(SpecializerKey, String)>>)
Internal:
  Bottom-up traversal via call_graph.bottom_up_order()
  Per-callee explosion guard via tti.specialization_budget (new field) + tti.specialization_code_growth_limit
  Union-splitting via TirType::Union detection + synthetic TypeGuard injection
  run_pipeline() on each clone after Repr seeding
```

### MODIFY: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/ip_summary.rs`

Lines 22–36: extend `FunctionSummary`:
- Add `pub does_not_capture_param: Vec<bool>` (empty = unknown / not yet computed)
- Add `pub is_pure: bool`
- Add `pub return_repr: Option<Repr>` (import `crate::representation_plan::Repr`)
- Add `pub return_alias: ReturnAliasSummary` (import from passes.rs, see below)

Lines 55–82: extend `ModuleSummaries::compute` to populate the new fields. The function gains a signature parameter:
```rust
pub fn compute(
    module: &TirModule,
    call_graph: &CallGraph,
    repr_plans: Option<&HashMap<String, RepresentationPlan>>,
) -> ModuleSummaries
```
Existing callers pass `None`; the module_phase rebuilds with `Some(per_function_repr_plans)` in step 5.

### MODIFY: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/escape_analysis.rs`

Line 220 — `analyze` signature: add `summaries: Option<&ModuleSummaries>`.

Line 367 — `OpCode::Call` arm: use `does_not_capture_param` from summary when available. See §2.2 above for the exact guard.

Line 771 — `run` convenience function: pass `None` as summaries (backward-compatible).

### MODIFY: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/pass_manager.rs`

The per-function pipeline's `escape_analysis` pass currently calls `escape_analysis::run(func)`. It needs to optionally receive an `Arc<ModuleSummaries>` to pass through. Two options:

**Chosen approach**: Add an optional `module_summaries: Option<Arc<ModuleSummaries>>` field to `PassManager` (not `TargetInfo` — summaries are module-scope, not target-scope). The escape_analysis pass adapter in `build_default_pipeline` captures it:

```rust
// In pass_manager.rs, PassManager struct:
pub module_summaries: Option<Arc<ModuleSummaries>>,
```

The escape analysis `TirPass::run` impl reads `self.module_summaries.as_deref()`.

### MODIFY: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/target_info.rs`

Add to `TargetInfo`:
```rust
/// Max number of specializations (Repr-tuple clones) per function.
/// 0 = specialization disabled (size-optimized builds). The behavioral
/// baseline is 0 (no specialization today → no behavior change on landing).
pub specialization_budget: usize,

/// Max ratio of total module op count increase from specialization.
/// Caps total code growth. E.g. 2.0 means at most 2× the original op count.
pub specialization_code_growth_limit: f32,

/// Whether to create union-split specializations.
pub union_split_enabled: bool,
```

Both constructors (`native_release_fast`, `wasm_release_fast`, etc.) set `specialization_budget = 0` and `union_split_enabled = false` as the **behavioral baseline** — Phase 1 of E5 lands without changing any decision. A subsequent phase sets the budget to a nonzero value after perf-gating the first benchmark.

Add query method:
```rust
pub fn is_specialization_enabled(&self) -> bool {
    self.specialization_budget > 0
}
```

### MODIFY: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/module_phase.rs`

`run_module_pipeline` (line 110): add E5 call between E1 and the rebuild:

```rust
pub fn run_module_pipeline(module: &mut TirModule, tti: &TargetInfo) -> ModuleAnalysis {
    let call_graph = CallGraph::build(module);
    let summaries = ModuleSummaries::compute(module, &call_graph, None);

    // E1: inline (existing)
    let _inline_stats = super::passes::inliner::run_inliner(module, &call_graph, &summaries, tti);

    // E5: specialization (new — currently a no-op when tti.specialization_budget == 0)
    if tti.is_specialization_enabled() {
        let _spec_stats = super::passes::specializer::run_specializer(
            module, &call_graph, &summaries, tti,
        );
    }

    // Rebuild post-transform
    let call_graph = CallGraph::build(module);
    let summaries = ModuleSummaries::compute(module, &call_graph, None);
    ModuleAnalysis { call_graph, summaries }
}
```

### MODIFY: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/mod.rs`

Add `pub mod specializer;` (line ~34, after `pub mod sccp;`).

### MODIFY: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/parallel.rs`

The per-function `compile_module_parallel` currently creates a `PassManager` with no module summaries. Thread `Arc<ModuleSummaries>` from the `ModuleAnalysis` returned by `run_module_pipeline` into each per-function `PassManager`:

```rust
// After run_module_pipeline returns analysis:
let summaries = Arc::new(analysis.summaries.clone());
// In the rayon closure per function:
let mut pm = build_default_pipeline(tti.clone())
    .with_module_summaries(Arc::clone(&summaries));
pm.run(func)
```

### MIGRATE: `passes.rs` `compute_return_alias_summaries` → `ip_summary.rs`

The existing `pub fn compute_return_alias_summaries` at passes.rs:156 operates on `&[FunctionIR]` (the legacy SimpleIR layer). The new `return_alias` field in `FunctionSummary` requires a TIR-native equivalent.

**Chosen approach**: keep the legacy SimpleIR version in passes.rs intact for now (it feeds the native backend directly). Add a new `compute_tir_return_alias_summary(func: &TirFunction, summaries: &ModuleSummaries)` in ip_summary.rs that produces the same `ReturnAliasSummary` enum from TIR ops. The `return_alias` field in `FunctionSummary` is populated by this new function. The legacy version remains and is not deleted yet — deletion happens when the TIR-native version is verified end-to-end on the native backend (a separate phase).

---

## 4. Soundness Argument

**E3 — does_not_capture_param[i]**:
- The computation is the existing `escape_analysis::analyze` applied intraprocedurally, bottom-up. The analysis is monotone (NoEscape ≤ ArgEscape ≤ GlobalEscape, only ever escalates). The bottom-up property ensures a callee's summary is computed before any caller reads it.
- The caller-side escape analysis upgrade (line 367): it transitions from GlobalEscape to ArgEscape — the lattice moves DOWN (less escaping), not UP. This is only sound if the callee truly does not capture the argument. The callee's summary says `does_not_capture_param[i] = true` only when the intraprocedural escape analysis of the callee returns NoEscape or ArgEscape for that parameter. That is the same analysis the intra-function path already trusts for builtins (the ArgEscape path at line 390).
- Recursive functions: in a mutual-recursion SCC, the bottom-up pass processes the SCC as a unit. Conservative treatment: for an SCC, `does_not_capture_param[i]` is only set to true when ALL members of the SCC agree. A self-recursive function's parameter always sees a `Call` to itself → the intraprocedural escape analysis marks the parameter at that `Call` as GlobalEscape (no summary available for self, conservative) → `does_not_capture_param[i] = false`. Fail-closed.
- SCC with multiple members: each member's escape analysis sees the other member's Call as opaque (no summary yet in the bottom-up walk). Conservative → GlobalEscape → `does_not_capture_param[i] = false` for all recursive cycle members. Correct.

**E3 — is_pure**:
- A function is classified `is_pure = true` only when:
  1. Every opcode in every block is in `effects::opcode_is_pure_movable` OR is a `CallBuiltin` with `builtin_effects.is_pure() = true` OR is a `Call` to a function whose summary says `is_pure = true`.
  2. No `CallMethod` (always opaque-impure), no opaque calls.
  3. No `TryStart/TryEnd/StateBlock*` (exception handlers have observable side effects).
  4. No `CheckException` that is the *only* impure thing: `CheckException` is a read of a flag that the callee's own behavior sets — it is a function of what the callee did, not an independent side effect. A function with only `CheckException` propagation and otherwise-pure ops is classified pure.
- A wrong `is_pure = true` would allow LICM to hoist a call that has side effects into a position that runs it unconditionally. The conservative default (any opcode not in the pure list → impure) prevents this. **The baseline (ip_summary.rs today) sets `is_pure = false` for everything** until the computation is added, so no behavior change from the new field on landing.

**E5 — Representation Specialization**:
- A clone is only created when the call site can statically prove all argument `Repr`s. Proof lives in `repr_by_value` of the caller's `RepresentationPlan`, which is constructed conservatively (default_for(type) = MaybeBigInt for int, never assumes RawI64Safe without proof).
- A call site that **cannot** prove the Repr key dispatches to the generic fallback. The generic is always retained.
- A specialization's parameter seed (`inject_repr_seed`) injects the caller's proven `Repr` into the clone's value-type map. If the proof was correct at the call site, the clone's pipeline will see a valid non-DynBox parameter. If the proof was wrong (a compiler bug), the worst case is a miscompile — but the proof is exactly the same one already trusted for the caller's own arithmetic lowering (which also calls `repr_by_value`). A miscompile from a wrong Repr seed is exactly as dangerous as the existing wrong-Repr miscompile the BigInt-correctness invariant already guards against (e.g. `apply(f, 1<<60, 7)`). The same differential tests that validate the int carrier (test_repr_bigint_roundtrip.py) will catch a bad specialization.
- **The generic fallback invariant**: the specializer MUST NOT delete the generic function body from the module. Any call site not statically resolved to a specialization continues to call the generic. This is enforced by the fact that `run_specializer` only *adds* new functions (clones) and *rewrites* resolved call sites — it never removes the original.
- **Recursive functions are never specialized**: call_graph.recursive_set() check in `is_specializable` (§3). A recursive function called with `RawI64Safe` arguments would require a specialization that itself calls the specialization recursively, creating an unbounded clone chain. Conservative exclusion.

---

## 5. Legacy This Arc Deletes

**E3 phase (when complete and end-to-end verified)**:
- The `OpCode::Call → GlobalEscape` unconditional arm in escape_analysis.rs:367 becomes the summary-gated version. The old unconditional path is removed.
- The `call_graph.rs` comment "Any function with an opaque call … conservatively recursion-capable" is correct and stays; only the escape analysis conservatism changes.

**E5 phase**:
- `tti.specialization_budget = 0` on all constructors until the perf gate passes. The specializer is a no-op gated behind `is_specialization_enabled()`. No dual path until the gate opens — no legacy code to delete.
- When the perf gate passes on native/release-fast and the specializer is activated: the representation_plan's `repr_by_name` map (the string-keyed legacy carrier of Repr facts) can be retired as the sole source of truth. The `ValueId`-keyed `repr_by_value` already is the single source of truth for intra-function decisions; specialization makes it the inter-function truth as well.

**Deferred S4 `compute_return_alias_summaries` migration**:
- The legacy `passes::compute_return_alias_summaries` (passes.rs:156) operating on `&[FunctionIR]` is replaced by the new TIR-native version in ip_summary.rs. The legacy version is deleted when the TIR version produces byte-identical codegen on all three backends (native, WASM, LLVM).

---

## 6. Test Plan

### 6.1 Rust Unit Tests (new file + additions)

**`specializer.rs` unit tests** (inside `#[cfg(test)] mod tests`):

```
test_specializer_leaf_int_function_creates_clone:
  Build a function f(x: I64) -> I64 { x + 1 }.
  Caller has x proven RawI64Safe.
  After run_specializer: module contains f__spec__RawI64Safe.
  f still present (generic fallback).
  Call site in caller rewired to f__spec__RawI64Safe.

test_specializer_does_not_specialize_when_budget_zero:
  tti.specialization_budget = 0.
  run_specializer produces SpecializerStats { clones_created: 0 }.

test_specializer_does_not_specialize_recursive_function:
  f calls itself. call_graph.recursive_set() contains f.
  run_specializer: no clones for f.

test_specializer_fallback_retained:
  After specialization, generic f is still in module.functions.

test_specializer_bigint_correct_at_boundary:
  f(x: I64) where x is MaybeBigInt at call site → no specialization,
  still dispatches to generic (bigint-correct path).

test_specializer_union_split:
  tti.union_split_enabled = true.
  f(x: Union([I64, Str])) → two specializations for the int and str variants.
```

**ip_summary.rs additions**:

```
test_does_not_capture_param_for_pure_function:
  f(x) { return len(x) } → does_not_capture_param = [true] (len is ArgEscape only).

test_is_pure_function:
  f(x) { return x * 2 } → is_pure = true.

test_is_impure_function_with_print:
  f(x) { print(x); return x } → is_pure = false.

test_return_repr_rawI64_known:
  f(x: RawI64Safe) { return x + 1 } → return_repr = Some(RawI64Safe).

test_return_repr_unknown_with_bigint_path:
  f(x: MaybeBigInt) { return x + 1 } → return_repr = None.

test_does_not_capture_recursive_function_is_conservative:
  f(x) { f(x) } → does_not_capture_param = [false].
```

**escape_analysis.rs additions**:

```
test_user_call_does_not_escape_when_summary_says_no_capture:
  Build TirFunction f with a Call op to "g".
  ModuleSummaries has g.does_not_capture_param = [true].
  analyze(f, Some(&summaries)) → call arg is ArgEscape, not GlobalEscape.

test_user_call_still_escapes_without_summary:
  analyze(f, None) → Call arg is GlobalEscape (conservative).
```

### 6.2 Differential Tests (Python snippets)

All tests run against CPython 3.12, 3.13, 3.14 using `tests/molt_diff.py` (basic lane).

**Stack allocation across call boundaries**:
```python
# tests/differential/basic/stack_alloc_across_call.py
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

def get_x(p):      # p does NOT escape get_x
    return p.x

def caller():
    p = Point(1, 2)  # should stack-allocate: get_x doesn't capture
    return get_x(p)

assert caller() == 1
```

**CSE/LICM across pure user calls**:
```python
# tests/differential/basic/licm_pure_user_call.py
def double(x):
    return x * 2

def f(n):
    result = 0
    for i in range(n):
        result += double(3)   # double(3) is loop-invariant; should hoist
    return result

assert f(100) == 600
```

**Representation specialization numeric correctness**:
```python
# tests/differential/basic/specialize_int_add.py
def add(a, b):
    return a + b

# Call site with proven small ints → specialization should produce RawI64Safe path
assert add(1, 2) == 3
assert add(1 << 60, 1) == 1152921504606846977  # bigint fallback must be correct
```

**BigInt boundary: specialization must NOT miscompile large ints**:
```python
# tests/differential/basic/specialize_bigint_boundary.py
def square(n):
    return n * n

assert square(3) == 9
assert square(1 << 47) == (1 << 47) ** 2      # crosses inline int boundary
assert square(1 << 60) == (1 << 60) ** 2      # bigint
```

**Exception propagation through specialized callee**:
```python
# tests/differential/basic/specialize_exception_propagation.py
def maybe_raise(x):
    if x < 0:
        raise ValueError(x)
    return x

try:
    maybe_raise(-1)
    assert False
except ValueError:
    pass
assert maybe_raise(5) == 5
```

**Union splitting correctness**:
```python
# tests/differential/basic/union_split.py
def show(x):
    if isinstance(x, int):
        return x * 2
    return str(x)

assert show(5) == 10
assert show("hi") == "hi"
```

**does_not_capture enables stack alloc (regression)**:
```python
# tests/differential/basic/ip_escape_no_capture.py
class Box:
    def __init__(self, v): self.v = v

def read_only(b):
    return b.v   # b does not escape

def caller():
    b = Box(42)   # must remain stack-allocated (read_only doesn't capture)
    return read_only(b)

assert caller() == 42
```

**is_pure enables LICM (regression)**:
```python
# tests/differential/basic/ip_pure_licm.py
def compute(x):
    return x * x - x + 1

total = 0
for _ in range(10):
    total += compute(3)   # loop-invariant, pure; LICM should hoist

assert total == 70
```

**Cross-backend parity**: run all snippets above through `molt build --target native`, `--target wasm`, `--target llvm` (where supported). Output must be byte-identical to CPython on every shape.

---

## 7. Perf Gate Plan

### 7.1 Benchmarks

The existing benchmark suite (referenced in MEMORY.md under `bench_sum`) plus:

| Benchmark | What it measures | Expected delta (E3 alone) | Expected delta (E5 enabled) |
|-----------|-----------------|--------------------------|----------------------------|
| `bench_sum` (loop `total += i`) | Loop-carried int accumulator | 0% (bug #15 still open) | TBD post-dual-loop-peel |
| `bench_struct` (struct field access via calls) | Stack alloc across calls | +10–20% (fewer heap allocs) | — |
| `bench_pure_math` (math-heavy loop with user fn) | LICM of pure user call | +15–30% (hoisting pure fn) | — |
| `bench_numeric_specialize` (new; int arithmetic through user fn) | Repr specialization | — | +2–5× on the RawI64Safe path |
| `bench_bigint_boundary` (stresses the 2^47 boundary) | BigInt correctness regression gate | must stay identical | must stay identical |

### 7.2 Measurement Protocol

1. Measure on native/release-fast first (baseline); must be ≥ CPython on every benchmark.
2. Measure on WASM (node 22, `--target wasm`) — E5 specialization_budget=0 on WASM so no code growth risk.
3. Measure on LLVM (`molt build --backend llvm`) — after the LLVM e2e link gap is fixed.
4. Measure on dev-fast and debug-with-asserts profiles — must not regress.
5. E5 activation: set `specialization_budget = 4` on native/release-fast only. Re-run all benchmarks. Accept the activation only if all benchmarks are ≥ pre-E5 numbers AND the bigint differential tests are green.

### 7.3 Code Size Gate

The existing `tools/verify_native_binary_valid.sh` runs after every build. E5 specialization adds functions to the module; the binary size will grow proportional to the number of specializations. Add a gate: `specialization_code_growth_limit = 1.5` (50% module op-count growth cap) on native/release-fast. Measure binary size delta before vs after E5 activation; if it exceeds the baseline by more than 200KB, reduce `specialization_budget`.

---

## 8. Risk, Rollback, Dependencies

### Dependencies (blocking)

- **E3 requires S4 call_graph + run_module_pipeline** — already landed (`7915b29a0`). Unblocked.
- **E3's does_not_capture requires escape_analysis::analyze** — already exists and is correct. Unblocked.
- **E5 requires repr_by_value in RepresentationPlan** — already exists (`cd66f365e` S6 + `64c2c53b8` repr-promotion). Unblocked.
- **E5 requires run_pipeline operating on cloned TirFunctions** — already exists. Unblocked.
- **E5 requires inliner clone primitives** — `clone_function_body_with_fresh_ids` at inliner.rs. Reuse directly.
- **E3's CheckException interaction with has_exception_handling** — the MEMORY.md keystone finding: `CheckException` sets `has_exception_handling = true`, making `is_inlineable` refuse real functions. The `is_pure` computation in E3 MUST use `has_exception_handlers()` (the narrower predicate from function.rs:153, which only fires on TryStart/TryEnd/StateBlock), NOT `has_exception_handling`. A function with only CheckException observation is still pure. This is the exact same distinction the inliner already draws.

### What E3 + E5 Unblock

- **E1 phase-c (exception-observation inlining)** — the MEMORY.md DORMANT finding. E3 does not unblock this directly, but the summary infrastructure it builds (does_not_capture, is_pure, return_repr) is prerequisite input for the activation's ROI calculation.
- **E4 IPSCCP** — return_repr backpropagation from E3 feeds the IPSCCP seed.
- **S5 MemorySSA + DSE** — IP-escape summaries (does_not_capture) are the callee-side input S5 needs to classify loads as non-aliasing across call boundaries.
- **os.walk OOM (Tier-3 D1)** — generator fusion and os.walk depend on E1 activation, which depends on E3 + the exception-observation inlining arc. Not unblocked yet.

### Risks

1. **SCC purity fixpoint complexity**: A mutual-recursion SCC where all members are individually "almost pure" — the conservative treatment (all recursive → is_pure=false) loses precision. This is sound and acceptable; recovering purity through SCC fixpoint is a follow-on arc.

2. **Specialization explosion on polymorphic call sites**: `f` called from 10 different callers each with a different Repr-key could produce 10 clones. The `specialization_budget` cap (§3, §7.2) prevents runaway growth. The initial budget of 4 per function is very conservative; it should be tuned post-measurement.

3. **Union-split correctness**: The TypeGuard injection into the clone entry block must use the exact same guard semantics as block_versioning.rs:502–565 (the existing SBBV path). Reuse the same guard-emission code path rather than reimplementing.

4. **repr_by_value availability in module phase**: `run_module_pipeline` runs *before* the per-function `compile_module_parallel`. The `repr_by_value` map for a caller is computed inside the per-function pipeline (representation_plan.rs). The specializer needs the caller's `repr_by_value` to compute the Repr key at each call site. Resolution: the specializer runs as a separate module-phase pass **after** the per-function pipeline has already run on all callers. This requires `run_module_pipeline` to be callable in two phases, or the specializer to run as a second module-phase pass invoked from `compile_module_parallel` after the per-function pipelines complete. The chosen sequence (§2.1) places E5 inside `run_module_pipeline` which runs BEFORE per-function pipelines — this means the specializer operates on the *unoptimized* Repr maps. Mitigation: run the per-function pipeline (just the representation plan computation, not the full 24-pass pipeline) on each function before the specializer consults its Repr map. This is a one-pass lightweight representation scan, not the full pipeline.

   **Concrete resolution**: Phase 1 of E5 (this arc) uses only `Repr::default_for(param_type)` as the specialization key — not the fully optimized `repr_by_value`. This is sound: if the param type is `TirType::I64`, the default floor is `MaybeBigInt`, so no specialization is created on Phase 1's key. The real Repr-key specialization (Phase 2) runs as a second module-phase pass after per-function pipelines, using the populated `repr_by_value`. Phase 1 (this arc) only implements the infrastructure: the SpecializerKey type, the clone+rename machinery, the call-site rewriter, and the `specialization_budget = 0` gate. All tests pass and the gate is 0 → no behavior change. Phase 2 activates by setting `specialization_budget > 0` after a per-function pre-pass populates repr maps.

### Rollback

All new code is additive (new files + new fields with zero-initialized defaults). Rolling back requires:
- Remove `pub mod specializer;` from passes/mod.rs
- Remove the `run_specializer` call from module_phase.rs (gated behind `is_specialization_enabled()`, so the gate can stay closed)
- Revert the ip_summary.rs field additions (trivial, the fields default to empty/false/None)
- The escape_analysis change is the riskiest: the `Option<&ModuleSummaries>` signature change is source-compatible and behavioral-neutral when `None` is passed

---

## 9. Phased Landing Sequence

Each phase is a complete structural piece. A phase is not done until its unit tests + the differential test matrix (all 3 CPython versions × all supported backends × all profiles) are green.

### Phase E3-A: Summary Fields + Intraprocedural Escape Computation
**Files**: ip_summary.rs, representation_plan.rs (import for Repr)
**Work**: Add `does_not_capture_param: Vec<bool>`, `is_pure: bool`, `return_repr: Option<Repr>` to `FunctionSummary`. Populate them in `ModuleSummaries::compute` using only intraprocedural escape_analysis + effects checks (no callee summary lookup yet). All recursive/SCC functions get conservative defaults.
**Delete**: Nothing.
**Gate**: `test_does_not_capture_param_for_pure_function`, `test_is_pure_function`, all existing ip_summary tests still green.

### Phase E3-B: Bottom-up Callee-Summary Propagation
**Files**: ip_summary.rs
**Work**: In the bottom-up loop, read already-computed callee summaries to propagate purity (a Call to a pure callee is pure). Extend `does_not_capture` across callee boundaries for pure callee calls.
**Delete**: Nothing.
**Gate**: `test_is_pure_function` with a caller+callee chain, existing tests.

### Phase E3-C: IP-Escape Integration into escape_analysis.rs
**Files**: escape_analysis.rs, pass_manager.rs, parallel.rs
**Work**: Thread `Option<Arc<ModuleSummaries>>` through the pipeline. Change `OpCode::Call` arm in escape_analysis::analyze per §2.2. Update all callers to pass `None` (backward-compatible). Add `test_user_call_does_not_escape_when_summary_says_no_capture` unit test.
**Delete**: The unconditional GlobalEscape arm for OpCode::Call.
**Gate**: `test_user_call_does_not_escape_when_summary_says_no_capture`, `stack_alloc_across_call.py` differential, all escape_analysis tests.

### Phase E3-D: Return-Alias Summary Migration (S4 deferred)
**Files**: ip_summary.rs, passes.rs (add TIR-native version)
**Work**: Add `compute_tir_return_alias_summary` in ip_summary.rs. Populate `return_alias` in FunctionSummary. Wire to the native backend's existing consumer sites in simple_backend.rs (currently calling `passes::compute_return_alias_summaries`). Verify byte-identical codegen.
**Delete**: Legacy `passes::compute_return_alias_summaries` call sites that now have a TIR-native equivalent (keep the function itself until verified on all three backends).
**Gate**: All existing return-alias-dependent tests + differential parity on all backends.

### Phase E5-A: Specializer Infrastructure (no-op phase)
**Files**: passes/specializer.rs (new), passes/mod.rs, target_info.rs, module_phase.rs
**Work**: Full specializer implementation with `specialization_budget = 0` gate. The specializer runs but produces zero clones. SpecializerKey, clone+rename, call-site rewriter, union-split stubs — all implemented, none activated.
**Delete**: Nothing.
**Gate**: `test_specializer_does_not_specialize_when_budget_zero`, `test_specializer_fallback_retained`, all existing tests still green.

### Phase E5-B: Repr-Key Computation + First Activation (native/release-fast only)
**Files**: specializer.rs, target_info.rs (set `specialization_budget = 4` in `native_release_fast`)
**Work**: Implement `compute_call_site_repr_key` using the post-per-function-pipeline repr_by_value (requires the second-module-phase pattern described in §8 Risk 4). Activate with `specialization_budget = 4` on native/release-fast. Run perf gate.
**Delete**: Nothing yet.
**Gate**: All differential tests including `specialize_bigint_boundary.py`, `specialize_int_add.py`, perf gate shows ≥ baseline on all benchmarks.

### Phase E5-C: WASM + LLVM Activation
**Files**: target_info.rs (wasm_release_fast, llvm_release_fast)
**Work**: Enable specialization on WASM and LLVM backends with their own budget values (likely lower due to binary-size concerns on WASM). End-to-end test on node 22 and LLVM.
**Delete**: Nothing.
**Gate**: Cross-backend differential parity, binary-size gate for WASM.

### Phase E5-D: Union Splitting
**Files**: specializer.rs
**Work**: Implement `union_split_enabled` path: for `TirType::Union` parameters, generate per-variant clones with TypeGuard injection. Gate behind `tti.union_split_enabled = true` (initially false on all constructors).
**Delete**: Nothing.
**Gate**: `union_split.py` differential test, verify TypeGuard injection uses exact same emission as block_versioning.rs:502–565.

---

## File:Line Reference Summary (verified against live code)

| Location | Current Role | This Arc's Change |
|---|---|---|
| `tir/passes/ip_summary.rs:24` | `FunctionSummary` struct | Add `does_not_capture_param`, `is_pure`, `return_repr`, `return_alias` fields |
| `tir/passes/ip_summary.rs:55` | `ModuleSummaries::compute` | Extend to populate new fields bottom-up |
| `tir/passes/escape_analysis.rs:367` | `OpCode::Call → GlobalEscape` (unconditional) | Summary-gated ArgEscape when `does_not_capture_param[i]` is true |
| `tir/passes/escape_analysis.rs:220` | `pub fn analyze(func: &TirFunction)` | Add `summaries: Option<&ModuleSummaries>` param |
| `tir/passes/escape_analysis.rs:771` | `pub fn run(func)` | Pass `None` summaries (backward-compat) |
| `tir/pass_manager.rs` | `PassManager` struct | Add `module_summaries: Option<Arc<ModuleSummaries>>` field |
| `tir/module_phase.rs:110` | `run_module_pipeline` | Add E5 `run_specializer` call between E1 and rebuild |
| `tir/passes/mod.rs:~34` | module declarations | Add `pub mod specializer;` |
| `tir/target_info.rs:163` | `TargetInfo` struct | Add `specialization_budget`, `specialization_code_growth_limit`, `union_split_enabled` |
| `tir/target_info.rs:246` | `native_release_fast()` | Set `specialization_budget = 0` (behavioral baseline, no change) |
| `tir/parallel.rs` | `compile_module_parallel` | Thread `Arc<ModuleSummaries>` into each function's `PassManager` |
| `passes.rs:156` | `compute_return_alias_summaries` | Migrate TIR-native equivalent to ip_summary.rs; retire after verification |
| **NEW** `tir/passes/specializer.rs` | — | E5 specializer engine (full implementation) |
