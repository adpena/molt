<!-- Foundation blueprint (architect swarm wf_18b24759-006, 2026-06-04). Arc: W3 dead-field / per-attribute DCE -> the <2MB binary + <50ms startup lever -->

# Whole-Program Per-Attribute DCE: Complete Implementation Blueprint

## 1. Precise Problem Statement

### Why it is load-bearing

The binary size target is <2 MB (native) and cold-start target is <50 ms. The flagship intrinsic resolver landed (`ddc4ff73b`, 4.31 MB empty.py). The remaining attribution breakdown (from `project_binary_size_regrounding_20260602.md`):

- Rust core: 1.83 MB (fixed)
- `builtins.py` bodies: ~282 KB
- `sys.py` bodies: ~574–800 KB
- Other stdlib: >1 MB

The root cause: every `func_new` or `func_new_closure` op in a module-init function is an unconditional DCE root in `eliminate_dead_functions` (`/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs:2022`). Because module-init functions are BFS roots (`molt_init_builtins`, `molt_init_sys`, etc. are reachable through `molt_main` → `import_name` → `call`), every function they define via `func_new` is kept alive — regardless of whether user code ever reads that attribute from the module.

Concretely: `builtins` defines ~hundreds of functions via `func_new` ops in `molt_init_builtins`. A simple `print("hello")` program links all of `filter`, `sorted`, `zip`, `enumerate`, `vars`, `dir`, `format`, `hex`, `bin`, `oct`, etc. — because each one is `func_new`-ed into the builtins module on init.

This is the definition of `make_function` as an unconditional DCE root mentioned in `docs/design/compiler_foundation_gap_analysis.md:151`. The gap analysis notes it correctly (`cli.py:17043` refers to the `func_new` kind in `_DEAD_FUNCTION_ELIM_REFERENCE_KINDS`).

### Proof that refining this is the real lever

The existing DFE BFS at `passes.rs:2005` already runs. It correctly drops functions that nothing calls. What it cannot do is distinguish:
- a `func_new` op whose result is consumed at a read site `MODULE_GET_ATTR("sorted")` from a caller that is BFS-reachable, vs.
- a `func_new` op whose result is stored into the module dict and then never read by any reachable call chain.

The fix is: when a `func_new`/`func_new_closure` op's result flows into a `MODULE_SET_ATTR` (or `module_set_attr` in SimpleIR) in a module-init function, do not treat the `func_new` as an unconditional edge. Instead, make the edge conditional on whether any reachable code later reads that module attribute by name.

This converts `func_new` from a write-biased DCE root to a liveness-gated edge: a function body is live only when a `MODULE_GET_ATTR`/`MODULE_GET_GLOBAL`/`MODULE_GET_NAME`/`MODULE_IMPORT_FROM` on the same (module_name, attr_name) pair is reachable from the BFS roots.

**Expected reduction**: stripping builtins (~282 KB) + sys (~574 KB) + all unread stdlib functions (>1 MB additional). Conservative estimate: 1.5–2 MB reduction on empty.py, pushing toward the <2 MB target.

---

## 2. Structurally Correct Design: End-State

### The core abstraction: ModuleAttributeReadSet

The BFS must distinguish two classes of `func_new` in module-init code:

1. **Definition site**: `molt_init_builtins` does `func_new("sorted__builtins")` → `module_set_attr(module, "sorted", func_val)`. The function body `sorted__builtins` is defined here.

2. **Read site**: some other function does `module_get_attr(module, "sorted")` or `module_get_global(module, "sorted")`. This is the read that makes the attribute live.

The correct liveness rule:
> A function `F` referenced by a `func_new` op inside `molt_init_<M>` through attribute `A` is live if and only if: (a) `molt_init_<M>` is itself reachable, AND (b) some reachable function performs a read of attribute `A` from module `M` (statically or conservatively), OR (c) the module is imported with `import *` (wildcard fallback), OR (d) the attribute is accessed through a dynamic/opaque path (fail-closed).

### Data structures

**`ModuleAttrWriteMap`** (built during a single forward scan of the SimpleIR):

```
module_attr_writes: BTreeMap<
    (ModuleName, AttrName),      // e.g. ("builtins", "sorted")
    Vec<FunctionName>            // the function bodies the func_new s_value names
>
```

Built by scanning every `molt_init_<M>` function for the pattern:
```
func_new s_value="sorted__builtins", out="vN"
...
module_set_attr args=["module_obj", "attr_name_str", "vN"]
```

where `attr_name_str` was defined by a preceding `const_str s_value="sorted"`.

This requires light value-flow tracking within module-init functions (SSA is flat in SimpleIR; a single linear pass with a local const_str map suffices).

**`ModuleAttrReadSet`**: the set of `(ModuleName, AttrName)` pairs that are accessed by reachable functions. Built during the existing BFS, augmented to recognize:
- `module_get_attr`, `module_get_global`, `module_get_name`, `import_from` ops with a static `s_value` (the attribute name)
- `import_name` ops that pull in a full module (adds all attributes of that module as potentially read — conservative fallback)
- `module_import_star` ops (adds all attributes of named module — conservative fallback)
- Any `module_get_*` op with a dynamic/non-static attribute name (adds the entire module's attribute set — fail-closed)

**`func_new` edge refinement** in `eliminate_dead_functions`:

Replace the current unconditional treatment:
```rust
"func_new" | "func_new_closure" | "code_new" => {
    // unconditionally add s_value to refs
}
```

With:
```rust
"func_new" | "func_new_closure" => {
    if let Some(name) = op.s_value.as_ref() && defined.contains(name) {
        // Only add if NOT a module-init attr definition, OR if the attr is read
        if !attr_write_map.is_attr_definition(func_name, name)
            || attr_read_set.is_attr_live(func_name, name)
        {
            refs.insert(name.clone());
        }
    }
}
"code_new" => {
    // code_new is used for class bodies and other non-function-attr use cases;
    // keep unconditional for now (conservative)
    if let Some(name) = op.s_value.as_ref() && defined.contains(name) {
        refs.insert(name.clone());
    }
}
```

### Two-phase BFS

The current BFS is single-phase. Per-attribute DCE requires two cooperating passes:

**Phase A — attr-write collection** (single linear scan, O(total ops)):
For each `molt_init_<M>` function, build `(module_name, attr_name) → [callee_symbol]` by tracking `const_str` → `func_new`/`func_new_closure` → `module_set_attr` value-flow.

**Phase B — augmented BFS** (extends the existing BFS):
Standard BFS, but:
1. When a function is added to `reachable`, scan its ops for `module_get_attr`/`module_get_global`/etc. ops. For each static `(module_name, attr_name)` read found, add the `func_new`-defined callees from the write map to the BFS queue.
2. When a `module_import_star` or dynamic read is found, mark the entire module's attr-write set as live (conservative fallback).
3. `import_name` ops referencing a module still make `molt_init_<M>` reachable (the init is always run), but not the individual function bodies defined inside it.

### Fail-closed proof obligations

The conservative rule: if any of the following is true, treat the module's entire attribute set as live (equivalent to old behavior):
1. The module has a `module_import_star` reader anywhere in the reachable set.
2. Any `module_get_attr`/`module_get_global` in the reachable set has a **non-constant** attribute name (dynamic attribute access).
3. The attribute name string is reachable as a `const_str` and then passed to a function whose body does a `module_get_*` on an arbitrary module (opaque reader).
4. `MOLT_DISABLE_DEAD_FUNC_ELIM` env var is set.

Under Tier-0 (no `exec`/`eval`/`compile`/runtime monkeypatching per CLAUDE.md), these cases are rare in practice, but the design must handle them soundly.

---

## 3. Exact Files to Create or Modify

### File 1: `/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs`

**Location**: `eliminate_dead_functions` function, currently lines ~2005–2146.

**Add** a new internal function `collect_module_attr_write_map`:

```rust
/// Scan `molt_init_<M>` functions for the pattern:
///   const_str s_value="attr_name" → out="vN"
///   func_new/func_new_closure s_value="symbol" → out="vM"
///   module_set_attr args=[module, "vN", "vM"]  (any ordering of the name/val pair)
///
/// Returns: BTreeMap<(module_name, attr_name), Vec<callee_symbol>>
///
/// Used by `eliminate_dead_functions` to make func_new liveness conditional
/// on whether the attr is ever read by a reachable function.
fn collect_module_attr_write_map(
    functions: &[FunctionIR],
) -> BTreeMap<(String, String), Vec<String>> { ... }
```

**Add** a new internal function `collect_module_attr_read_set` that runs **during** the BFS (or as a pre-scan of the reachable set):

```rust
/// For a given set of already-reachable functions, collect all
/// (module_name, attr_name) pairs read by any static module_get_attr /
/// module_get_global / module_get_name / import_from op.
/// Returns the read set and a `wildcard_modules` set for modules accessed via
/// import_star or dynamic attr names.
fn collect_module_attr_read_set(
    functions: &[FunctionIR],
    reachable_names: &BTreeSet<String>,
) -> (BTreeSet<(String, String)>, BTreeSet<String>) { ... }
```

**Modify** the BFS in `eliminate_dead_functions` to become an iterative two-phase loop:

```rust
pub fn eliminate_dead_functions(ir: &mut SimpleIR) {
    // ... (existing guard checks) ...

    // Phase A: collect attr write map from module-init functions.
    let attr_write_map = collect_module_attr_write_map(&ir.functions);
    // Reverse map: callee_symbol -> (module_name, attr_name) for fast lookup
    let symbol_to_attr: BTreeMap<String, (String, String)> = build_symbol_to_attr(&attr_write_map);

    // Build the call graph (same as before), EXCEPT:
    // - func_new for symbols in symbol_to_attr are NOT added as unconditional edges;
    //   they are added to a deferred set.
    let (references, deferred_func_new) = build_references_with_attr_deferral(
        &ir.functions, &defined, &symbol_to_attr
    );

    // Phase B: iterative BFS that also discovers attr reads.
    let mut reachable = BTreeSet::new();
    let mut attr_live: BTreeSet<(String, String)> = BTreeSet::new();
    let mut wildcard_modules: BTreeSet<String> = BTreeSet::new();
    let mut queue = VecDeque::new();

    // Seed roots (same as before).
    // ...

    while !queue.is_empty() {
        // Pop, scan for attr reads, potentially unlock deferred func_new edges.
        // Repeat until stable.
    }

    // Drain: any deferred_func_new whose (module, attr) is in attr_live is now reachable.
    // Any whose module is in wildcard_modules is reachable.
    // ...

    ir.functions.retain(|f| reachable.contains(&f.name));
}
```

The design is an augmented BFS that is self-stabilizing: adding a new reachable function may discover new attr reads, which may unlock new `func_new` callees, which adds more functions to the queue. This terminates because the set of functions is finite.

### File 2: `/Users/adpena/Projects/molt/src/molt/cli.py`

**Location**: `_reachable_function_names_for_stdlib_cache` (~line 17057) — the Python-side stdlib cache BFS.

**Mirror the same attr-write / attr-read refinement** so the stdlib cache key computation uses the same liveness oracle as the native DFE. Currently this BFS uses `_DEAD_FUNCTION_ELIM_REFERENCE_KINDS` which includes `func_new` unconditionally.

**Add** a Python-side `_collect_module_attr_write_map(functions)` and `_collect_module_attr_read_set(functions, reachable)` that mirror the Rust logic exactly, applied in `_reachable_function_names_for_stdlib_cache`.

**Critical**: the stdlib cache key must hash the same reachable function set as the backend computes. If there is drift, the cache will contain functions the backend DFE would eliminate, causing over-linking. The Python BFS and Rust BFS must use identical liveness logic.

### File 3: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/module_phase.rs` (minor)

No structural change needed yet. The TIR-level call graph (`CallGraph::build`) already handles this correctly: it only records `StaticDirect` edges for `OpCode::Call`, not for function-value-creating ops. The TIR DCE is already attribute-aware through the effects oracle (`effects.rs`). The per-attribute DCE is a SimpleIR-level optimization (the `eliminate_dead_functions` BFS) that runs before TIR lifting.

### File 4: `/Users/adpena/Projects/molt/runtime/molt-backend/src/wasm.rs` (WASM manifest)

The WASM manifest scanner (`manifest_intrinsic_names` construction in `wasm.rs`) scans `const_str` ops. It does not directly interact with the `func_new` DCE. However, after the DCE eliminates unreachable `molt_init_<M>`-defined functions, the WASM manifest is already naturally trimmed (fewer functions → fewer `const_str` intrinsic refs). No direct change required.

### File 5: New test file: `/Users/adpena/Projects/molt/runtime/molt-backend/src/passes/tests_attr_dce.rs`

Or within the existing `#[cfg(test)] mod tests` in `passes.rs` — add the attr DCE unit tests (see §6).

---

## 4. Soundness Argument

### Invariant: a function is live iff it is reachable through a read chain

**Claim**: under Tier-0 (no `exec`/`eval`/`getattr` with dynamic names / `import *`), a function `F` defined via `func_new` into module attribute `(M, A)` can only be called if some reachable function reads `(M, A)` — either by `module_get_attr(M, A)`, `module_get_global(M, A)`, or `import_from(M, A)` (which resolves to `getattr(module, name)` at the CPython level, with the submodule fallback for circular imports).

**Proof sketch**:
1. `func_new("F")` creates a function object that wraps the symbol `F`.
2. The function object is stored via `module_set_attr(module_M, "A", func_obj)`.
3. Any invocation of `F` requires first retrieving the function object from `(M, A)`.
4. Under Tier-0, there is no `exec`/`eval` that could construct the attribute name `"A"` at runtime from a non-constant string.
5. Therefore, if no reachable code contains a static read of `(M, A)`, the function object is never retrieved, and `F`'s body is never executed.
6. Eliminating `F`'s body is therefore correct.

**Conservative fallback cases** (all produce the old behavior, never a miscompile):
- Any `import *` from module `M` in the reachable set → keep all of `M`'s attrs live.
- Any `module_get_attr` on module `M` with a non-constant attribute name → keep all of `M`'s attrs live.
- Any opaque call (`call_indirect`, `call_method`, `call_func`) in a reachable function → already handled by existing DFE (the callee of an opaque call is unknown, so no DCE of anything it might reference — this is existing behavior and unchanged).

**The critical case: closures that capture module attrs**

A closure over a module attribute does NOT create a new `func_new` in the module dict. It creates a closure object in the parent function's stack. The closure's code body is referenced by `func_new_closure` in the enclosing function's body. That enclosing function must itself be reachable for the closure to exist. So `func_new_closure` edges inside non-init functions are unconditional (as today) — only `func_new`/`func_new_closure` inside `molt_init_<M>` functions that flow into `module_set_attr` get the conditional treatment.

**Conservative classification** of `code_new`:

`code_new` creates a code object (used for class bodies, lambda, comprehension cell objects). It is not a function-object-into-module-dict store. It remains unconditionally live (as today). This is conservative but correct.

---

## 5. Legacy This Arc Deletes

The arc removes **one dual path**:

**Removed**: The implicit "if it's in a `func_new` op in a reachable module-init, it's live" assumption in the DFE BFS (`passes.rs:2022`). The `func_new` arm is NOT removed from the match — it is refined: `func_new` ops that are not module-attr definitions remain unconditional (function objects created in non-init contexts, closures, etc.). The `func_new` arm becomes split:
- Module-init func_new to a module-set-attr: **conditional on attr being read**
- All other func_new: **unconditional** (same as today)

The `_DEAD_FUNCTION_ELIM_REFERENCE_KINDS` constant in `cli.py:17019` remains structurally correct (it already includes `func_new`), but `_reachable_function_names_for_stdlib_cache` gains the same conditional logic that the Rust DFE gets.

**No other pass is deleted** in this arc. The call graph (`tir/call_graph.rs`) is already correct — it only tracks `OpCode::Call`/`OpCode::CallMethod` as call edges, never `func_new`. The TIR-level DCE (`tir/passes/dce.rs`) is value-level, not function-level, and is unaffected.

---

## 6. Test Plan

### Rust Unit Tests (add to `passes.rs` `#[cfg(test)]` section)

**Test: attr_dce_unread_func_new_eliminated**
```rust
// Module init defines "sorted" via func_new, user code never reads it.
// Expected: sorted_body is eliminated.
fn test_unread_module_attr_eliminated()
```

**Test: attr_dce_read_func_new_kept**
```rust
// Module init defines "sorted". User function reads "sorted" from module.
// Expected: sorted_body is retained.
fn test_read_module_attr_kept()
```

**Test: attr_dce_import_star_keeps_all**
```rust
// User code does `import_star` from the module.
// Expected: all module attrs are retained (conservative fallback).
fn test_import_star_conservative()
```

**Test: attr_dce_dynamic_attr_keeps_all**
```rust
// User code does module_get_attr with a non-constant attr name.
// Expected: all attrs of that module are retained (conservative fallback).
fn test_dynamic_attr_conservative()
```

**Test: attr_dce_closure_in_init_conditional**
```rust
// Module init defines a closure (func_new_closure) stored via module_set_attr.
// Not read by user code. Expected: closure body eliminated.
fn test_closure_init_attr_not_read_eliminated()
```

**Test: attr_dce_non_init_func_new_unconditional**
```rust
// A non-init function creates a function object via func_new (not stored in module dict).
// Expected: the callee is kept (unconditional, existing behavior).
fn test_non_init_func_new_unconditional()
```

**Test: attr_dce_chained_modules**
```rust
// Module A's init defines func "f", stored as attr "helper".
// Module B's init reads "helper" from A and stores a derived func.
// User code reads from B but never from A directly.
// Expected: A's "f" is live (transitively read through B's init).
fn test_chained_module_attr_live()
```

**Test: attr_dce_code_new_unconditional**
```rust
// code_new inside a module init. Expected: body is kept (conservative).
fn test_code_new_unconditional()
```

### Differential Python Tests

**Shape 1: empty program**
```python
# empty.py
```
Expected: builtins attrs for `filter`, `sorted`, `zip`, etc. eliminated. Binary size reduction ≥ 150 KB.

**Shape 2: single builtin read**
```python
# reads_sorted.py
x = sorted([3,1,2])
print(x)
```
Expected: `sorted` body retained. `filter`, `zip`, `enumerate`, `vars`, `dir`, etc. eliminated.

**Shape 3: import star**
```python
# star_import.py
from builtins import *
```
Expected: all builtins retained (conservative fallback). Binary size same as today.

**Shape 4: dynamic getattr**
```python
# dynamic_getattr.py
import builtins
name = input()
f = getattr(builtins, name)
```
Expected: all builtins attrs retained (conservative — `getattr` lowers to a dynamic read).

**Shape 5: stdlib import**
```python
import math
x = math.sqrt(2.0)
```
Expected: `math.sqrt` body retained. `math.factorial`, `math.gcd`, etc. (if unread) eliminated.

**Shape 6: adversarial — function read through intermediate variable**
```python
import builtins
f = builtins.sorted
result = f([3,1,2])
```
Expected: `sorted` retained (the `builtins.sorted` is a `module_get_attr("sorted")`).

**Shape 7: bigint correctness (regression guard)**
```python
def apply(f, x, n): return f(x, n)
print(apply(lambda a,b: a**b, 1<<60, 7))
```
Expected: correct bigint result (`1152921504606846983`) unchanged.

**Shape 8: cross-backend parity**
All shapes above run on native + WASM + LLVM targets, verified byte-identical to CPython 3.12/3.13/3.14.

**Shape 9: exception safety**
```python
try:
    from builtins import nonexistent
except ImportError:
    pass
```
Expected: `ImportError` raised correctly. `molt_init_builtins` still runs (reads trigger init), but individual unread bodies eliminated.

**Shape 10: import chain**
```python
import os
os.path.join("a", "b")
```
Expected: `os.path.join` retained. `os.path.exists`, `os.path.abspath`, etc. (if not read) eliminated.

---

## 7. Perf-Gate Plan

### Measurements

**Metric 1: binary size (primary)**
Baseline: `empty.py` → 4.31 MB (post-`ddc4ff73b`).
Target: ≤ 2.5 MB (first cut, conservative; ≤ 2 MB requires also factoring Rust core which is orthogonal).
Measurement: `python3 -m molt build --target native --output /tmp/test_out examples/empty.py && ls -la /tmp/test_out`.

**Metric 2: cold-start latency**
Baseline: 3.58 ms micro / 5.73 ms full (from `project_startup_baseline_20260603.md`).
Expected delta: small decrease (fewer stdlib body relocations on dyld first touch, fewer code-page faults). Measure with: `time /tmp/test_out` × 100 runs.

**Metric 3: per-benchmark correctness gate**
All existing benchmarks (`bench/bench_sum.py`, `bench/bench_dict.py`, `bench/bench_float.py`, `bench/bench_list.py`, `bench/bench_string.py`, `bench/bench_while.py`) must remain faster than CPython on native target.
Measurement: `python3 tools/bench.py --target native`.

**Metric 4: test suite regression gate**
`cargo test -p molt-backend --features native-backend` — all 882 tests pass.

**Profiles**: release-fast (primary), dev-fast (must not regress), debug-with-asserts (must not regress).

**Targets**: native/Cranelift (primary), WASM (secondary — manifest is naturally trimmed), LLVM (secondary — per-attribute DCE fires at SimpleIR layer before LLVM lifting).

### Expected delta breakdown

| Eliminated set | Estimated saving |
|---|---|
| Unread builtins functions | ~150–200 KB |
| Unread sys.py functions | ~200–300 KB |
| Unread stdlib (math, os, etc.) | ~300–600 KB |
| Total (conservative first cut) | ~650 KB – 1.1 MB |

First cut will not achieve <2 MB (Rust core is 1.83 MB fixed). But combined with Rust feature-gating (orthogonal arc), the <2 MB target is reachable.

---

## 8. Risk + Rollback + Dependency Notes

### Risks

**R1: Value-flow tracking in module-init functions**

The `const_str` → `func_new` → `module_set_attr` pattern requires tracking which `const_str` out-variable feeds the `module_set_attr` attribute-name argument. SimpleIR is flat and linear within a function body (no SSA, just named variables), so a single linear scan with a `HashMap<var_name, const_str_value>` suffices. Risk: aliasing. If a `const_str` out-variable is overwritten (`store_var` / `copy`) before being consumed by `module_set_attr`, the naïve scan may miss the write. Mitigation: if the variable tracking is ambiguous, fall back to the conservative case (treat the entire module's attrs as live). This is fail-closed.

**R2: Stdlib cache coherence with native DFE**

The Python BFS (`_reachable_function_names_for_stdlib_cache` in `cli.py`) and the Rust BFS (`eliminate_dead_functions` in `passes.rs`) must produce identical reachable sets, or the stdlib cache will contain functions the native DFE eliminates (over-linking, not miscompile) or the cache will be invalidated on every build (wrong key). Both must be updated atomically in the same commit.

**R3: The stdlib cache schema version**

`_SHARED_STDLIB_CACHE_SCHEMA_VERSION` in `cli.py` must be bumped when the liveness logic changes, to force cache invalidation on the first build after upgrade. This is a `str` constant in `cli.py`; search for it and increment.

**R4: Modules that mutate their own attribute set after init**

Some stdlib modules use `sys.modules[name].__dict__[attr] = ...` patterns after init (Python-level monkeypatching). Under Tier-0, this is prohibited per CLAUDE.md ("no runtime monkeypatching"). If a stdlib module does this, it is a correctness bug to fix at the source, not a reason to widen the liveness set.

**R5: `__getattr__` module hooks (PEP 562)**

A module that defines `__getattr__` can synthesize attributes on demand. The frontend might emit this as a regular function definition in the module init. If `__getattr__` is defined and referenced from `molt_init_<M>`, it must be unconditionally kept (it is the runtime fallback for any attribute miss). Detection: if `molt_init_<M>` contains a `func_new("...__getattr__...")` followed by `module_set_attr(module, "__getattr__", ...)`, mark the entire module's attr set as conservative (all attrs live) because any attr access could be dynamically synthesized. This is a rare edge case but matters for correctness.

### Blocked-by / Unblocks

**Blocked-by**: nothing. This arc is independent of S5 (alias analysis), E1 (inliner), and the Repr migration. It operates entirely at the SimpleIR layer, on `FunctionIR`/`OpIR` structs, before TIR lifting.

**Unblocks**:
- The `RuntimeSurfacePlan` binary-size sprint (per `project_runtimesurfaceplan_sprint.md`) — per-attribute DCE is the fine-grained DCE that complements the coarse subsystem gating.
- W3 in the gap analysis (compiler_foundation_gap_analysis.md:152) is exactly this arc; landing it unblocks W2 (CHA + speculative devirt) since W2 relies on precise reachability.

### Rollback

The rollback is trivially the identity: setting `MOLT_DISABLE_DEAD_FUNC_ELIM=1` disables `eliminate_dead_functions` entirely (this env var already exists at `passes.rs:2006`). A targeted per-arc disable can be added: `MOLT_DISABLE_ATTR_DCE=1` to skip only the attr-conditional logic and fall back to today's behavior for `func_new`, while keeping the rest of DFE. Add this in Phase 1.

---

## 9. Phased Landing Sequence

Each phase is a complete structural piece that passes all tests independently.

### Phase 1: SimpleIR attr-write-map collection (no behavior change)

**Commit content**: Add `collect_module_attr_write_map` to `passes.rs`. Add unit tests that verify the map is correct for synthesized `FunctionIR` inputs representing a module-init-like function. No change to `eliminate_dead_functions` behavior yet.

**Files**: `/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs`

**Acceptance**: `cargo test -p molt-backend` green; new unit tests pass; no behavior change (DFE still eliminates same functions as before).

### Phase 2: Rust DFE augmentation + Python BFS mirror (behavior change lands)

**Commit content**: 
1. Augment `eliminate_dead_functions` with the two-phase BFS (Phase A write-map, Phase B attr-conditional BFS).
2. Mirror the same logic in `_reachable_function_names_for_stdlib_cache` in `cli.py`.
3. Bump `_SHARED_STDLIB_CACHE_SCHEMA_VERSION` in `cli.py`.
4. Add `MOLT_DISABLE_ATTR_DCE=1` rollback env var.
5. Add Rust unit tests (all 8 shapes from §6).

**Files**: 
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs`
- `/Users/adpena/Projects/molt/src/molt/cli.py`

**Acceptance**: 
- `cargo test -p molt-backend --features native-backend` green (882+ tests).
- Differential: all 10 Python shapes from §6 byte-identical to CPython 3.12/3.13/3.14.
- Binary size: `empty.py` ≤ 3.5 MB (conservative target for this phase; builtins + sys reduction).
- All bench/* faster than CPython on native.

### Phase 3: Measurement, tuning, and WASM verification

**Commit content**:
1. Measure binary size delta across `empty.py` + representative app (`examples/hello.py`, a math-using program, an asyncio-using program).
2. Verify WASM target: `molt build --target wasm` produces smaller `.wasm` (fewer functions compiled).
3. Add the perf-gate CI check entry for binary size regression (emit a failure if `empty.py` native binary exceeds a threshold).

**Files**: 
- `/Users/adpena/Projects/molt/tools/verify_native_binary_valid.sh` (add size threshold check)
- Possibly a new `tools/verify_binary_size_gate.sh`

**Acceptance**: Binary size gate integrated; no regressions on any backend/profile combination.

### Phase 4 (follow-up, separate arc): TIR-level attribute read analysis

Once the SimpleIR-level DCE lands, the TIR call graph can be extended with a `ModuleAttrReadSummary` that tracks which module attributes each function reads. This enables more precise liveness at the TIR level (before lowering to SimpleIR) and feeds into the W2 CHA arc. This is a separate arc and must not be conflated with Phases 1–3.

---

## Summary of Key Anchor Points

- **`/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs:2005`** — `eliminate_dead_functions`; the BFS that gets the two-phase augmentation.
- **`/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs:2022`** — the `"func_new" | "func_new_closure" | "code_new"` arm that becomes attribute-conditional for the first two kinds.
- **`/Users/adpena/Projects/molt/src/molt/cli.py:17057`** — `_reachable_function_names_for_stdlib_cache`; the Python BFS mirror.
- **`/Users/adpena/Projects/molt/src/molt/cli.py:17019`** — `_DEAD_FUNCTION_ELIM_REFERENCE_KINDS`; structurally unchanged (it is the reference-detection set for the BFS edges, not the liveness logic per se).
- **`/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/call_graph.rs`** — already correct; does not treat `func_new` as a call edge. No change.
- **`/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/module_phase.rs`** — no change; per-attribute DCE fires before TIR lifting.
- **`/Users/adpena/Projects/molt/src/molt/frontend/__init__.py:5401`** — `_emit_module_attr_set`; this is the frontend emit path that generates `MODULE_SET_ATTR` ops. Its output shape is what the attr-write scanner must recognize in SimpleIR form.
- **`/Users/adpena/Projects/molt/src/molt/frontend/__init__.py:5170`** — `FUNC_NEW` emit site; the `s_value` is the callee symbol name that the attr-write scanner records.
