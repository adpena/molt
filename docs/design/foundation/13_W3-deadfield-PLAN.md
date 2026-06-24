<!-- Wave-3 recon implementation plan (wf_00af7480-2ba, 2026-06-04), live-code-verified. -->

# W3: Whole-Program Per-Attribute / Dead-Field DCE — Complete Implementation Plan

## Orientation: What the Existing Code Does and Where the Gap Is

### The unconditional `func_new` DCE root (the gap)

`eliminate_dead_functions` in `/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs:1786` runs a BFS from entry roots. At line 1803:

```rust
"call" | "call_internal" | "func_new" | "func_new_closure" | "func_new_builtin"
| "code_new" | "call_guarded" => {
    if let Some(name) = op.s_value.as_ref() && defined.contains(name) {
        refs.insert(name.clone());
    }
}
```

`func_new` is treated identically to `call`. This means every function body created with `func_new` in any live function is kept alive, including the hundreds of builtins defined in `molt_init_builtins`, `sys.py` functions in `molt_init_sys`, and every other stdlib module-init function — even when user code never reads those module attributes. This is the structural cause of the 4.31 MB binary for `empty.py`.

### IR shape that must be analyzed

In `molt_init_builtins` (and every other `molt_init_<M>` function), the pattern emitted by the frontend (frontend serialization at `/Users/adpena/Projects/molt/src/molt/frontend/__init__.py:33870`) is:

```json
{ "kind": "func_new", "s_value": "sorted__builtins", "value": 1, "out": "v42" }
...
{ "kind": "const_str", "s_value": "sorted", "out": "v43" }
...
{ "kind": "module_set_attr", "args": ["module_obj", "v43", "v42"], "out": "none" }
```

The attr name `"sorted"` is not directly in the `module_set_attr` op — it is in `args[1]`, an SSA variable reference to a preceding `const_str` op. The value-flow chain `const_str("sorted") → v43 → module_set_attr[1]` must be tracked within the function.

### The Python-side BFS mirror

`_reachable_function_names_for_stdlib_cache` at `/Users/adpena/Projects/molt/src/molt/cli.py:17120` runs the same BFS to compute the stdlib cache key. It uses `_DEAD_FUNCTION_ELIM_REFERENCE_KINDS` (line 17082) which includes `"func_new"` and `"func_new_closure"` unconditionally. This must be updated identically to the Rust DFE, or the cache key will diverge from the live set the backend computes.

### What `module_get_attr` / read sites look like in SimpleIR

`module_get_attr` ops (lowered from `MODULE_GET_ATTR` at frontend line 34190) serialize as:
```json
{ "kind": "module_get_attr", "args": ["module_obj", "v99"], "out": "v100" }
```
where `v99` is a `const_str s_value="sorted"` that precedes the get. Same SSA-variable-reference pattern. The static attr name is in `args[1]` pointing back to a `const_str`.

### Module name association

`molt_init_<M>` functions are named with a `molt_init_` prefix. The module name `M` is recoverable from the function name. All `module_set_attr` ops inside `molt_init_<M>` bind to module `M`. A read `module_get_attr(module_obj, "sorted")` inside any reachable function reads from whatever module `module_obj` represents; the module identity is tracked via `const_str` → `module_cache_get` / `module_import` / direct use of `__molt_module_obj__`.

For the conservative first cut: the module identity in the write map is the function-name prefix (`molt_init_<M>` → module name `M`). For reads, the module is identified by the `const_str` that feeds `module_cache_get` which produces the module reference consumed by `module_get_attr`. This requires tracking one more level of value flow. **The safe conservative fallback**: if any value flow cannot be statically resolved to a specific `(M, A)` pair, mark the entire module's attrs as live.

---

## End-State Design

### Data Structures

**`ModuleAttrWriteMap`** — built by a single linear pass over all `molt_init_*` functions:
```rust
// (module_name, attr_name) -> Vec<callee_function_symbol>
BTreeMap<(String, String), Vec<String>>
```
Built by tracking within each `molt_init_<M>` function body:
- `const_str_values: HashMap<SSA_var, String>` — maps variable name to the literal string from `const_str` ops
- `func_new_values: HashMap<SSA_var, String>` — maps variable name to the function symbol from `func_new`/`func_new_closure` ops
- On each `module_set_attr args=[module_var, attr_var, val_var]`:
  - Lookup `attr_var` in `const_str_values` → get `attr_name`
  - Lookup `val_var` in `func_new_values` → get `callee_symbol`
  - If both resolved: insert `(module_name, attr_name) → callee_symbol` into map
  - If either unresolved: mark `module_name` as wildcard (conservative)

**`symbol_to_attr: BTreeMap<String, (String, String)>`** — reverse map: `callee_symbol → (module_name, attr_name)`. Used in the BFS to check if a `func_new` is an attr-definition.

**`ModuleAttrReadSet`** — built incrementally during the BFS:
```rust
attr_live: BTreeSet<(String, String)>       // (module_name, attr_name) pairs confirmed read
wildcard_modules: BTreeSet<String>          // modules where all attrs are live (conservative)
```

**`deferred_func_new: HashMap<String, (String, String)>`** — `callee_symbol → (module_name, attr_name)` for `func_new` ops that are module-attr definitions and cannot be unconditionally added to refs. Added to reachable set lazily when the attr becomes live.

### The Augmented BFS

Replace single-pass BFS with a two-phase iterative BFS that terminates when neither the reachable set nor the attr_live set grows.

**Phase A** (pre-BFS, O(total ops)):
- Scan all `molt_init_*` functions to build `ModuleAttrWriteMap` and `symbol_to_attr`.

**Phase B** (the BFS itself):
Standard BFS but with these augmentations:
1. When scanning a function's ops to collect `refs`:
   - `func_new`/`func_new_closure` with `s_value = callee`: if `callee` is in `symbol_to_attr` → do NOT add to `refs` unconditionally; add to `deferred_func_new` instead.
   - `func_new`/`func_new_closure` with `callee` NOT in `symbol_to_attr` → add unconditionally (same as today).
   - `module_get_attr`, `module_get_global`, `module_get_name`, `import_from` ops: resolve the static `(module_name, attr_name)` pair from the preceding `const_str` context; add to `attr_live`. If resolution fails, add `module_name` to `wildcard_modules`.
   - `module_import_star` ops: add referenced module to `wildcard_modules`.
2. After each BFS drain: drain `deferred_func_new` — any `callee` whose `(module_name, attr_name)` is in `attr_live`, or whose `module_name` is in `wildcard_modules`, is now reachable. Seed those callees and re-run BFS.
3. Repeat until stable (no new functions added, no new attrs unlocked).

**Termination**: the total number of functions is finite and bounded; the attr_live set is monotone-growing. Each iteration must add at least one new function or one new attr. Terminates in `O(|functions| + |attr_write_map|)` iterations.

### Fail-Closed Rules

A module's entire attr set is kept alive (equivalent to current behavior) when:
1. `module_import_star` from module `M` is reachable.
2. Any `module_get_attr`/`module_get_global` on module `M` with a non-constant (or unresolvable) attribute name is reachable.
3. The `__getattr__` attribute is defined in module `M` (PEP 562 hook — if present, any attribute access could route through it).
4. `MOLT_DISABLE_ATTR_DCE=1` env var is set (new rollback gate, coarse-grained).
5. `MOLT_DISABLE_DEAD_FUNC_ELIM=1` already disables DFE entirely (existing gate at passes.rs:1787).

Under Tier-0 (no `exec`/`eval`/runtime monkeypatching per CLAUDE.md), cases 1–3 are rare in practice but the design handles them soundly.

---

## Phased Landing — Each Phase is a Complete Structural Piece

### Phase 1: SimpleIR attr-write-map collection (no behavior change)

**Files changed**:
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs`

**What lands**:
Add `collect_module_attr_write_map` as an internal function above `eliminate_dead_functions` (insert at approximately line 1785). The function signature:

```rust
/// Scan every `molt_init_<M>` function for the 3-op pattern:
///   const_str s_value="attr_name", out="vA"
///   func_new/func_new_closure s_value="callee_sym", out="vF"
///   module_set_attr args=[module_obj, "vA", "vF"]  (third arg must be the func val)
///
/// Returns a map (module_name, attr_name) → Vec<callee_symbol>
/// and a wildcard_modules set for modules where attr-granularity is impossible.
fn collect_module_attr_write_map(
    functions: &[FunctionIR],
) -> (BTreeMap<(String, String), Vec<String>>, BTreeSet<String>) {
```

Value-flow tracking within each `molt_init_*` function:
- `const_str_map: HashMap<&str, &str>` — SSA var → string literal (from `const_str` ops)
- `func_val_map: HashMap<&str, &str>` — SSA var → callee symbol (from `func_new`/`func_new_closure` ops)
- On `module_set_attr` with `args.len() == 3`: resolve `args[1]` via `const_str_map` and `args[2]` via `func_val_map`. If both resolve, add to write map. If `args[1]` fails to resolve: add module to wildcard.

The module name is extracted from the function name: `"molt_init_foo" → "foo"`. Only functions whose name starts with `"molt_init_"` are scanned.

Add unit tests in `passes.rs #[cfg(test)]` block:
- `attr_write_map_basic`: synthesize a `FunctionIR` named `molt_init_foo` with the exact `const_str/func_new/module_set_attr` sequence; assert the map contains `("foo", "bar") → ["bar__foo"]`.
- `attr_write_map_dynamic_attr_name_wildcard`: unresolvable attr name → `"foo"` in wildcard set.
- `attr_write_map_non_init_function_ignored`: a non-`molt_init_*` function with same ops → no entries.
- `attr_write_map_getattr_hook_wildcard`: `func_new("...__getattr__")` stored as `"__getattr__"` → module in wildcard.

**Acceptance**: `cargo test -p molt-backend --features native-backend` passes; no behavior change in DFE.

---

### Phase 2: Rust DFE augmentation + Python BFS mirror (behavior change lands)

**Files changed**:
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs`
- `/Users/adpena/Projects/molt/src/molt/cli.py`

**Rust changes** to `eliminate_dead_functions` starting at line 1786:

Replace the single BFS with the two-phase iterative BFS described above. Key structural change: add `MOLT_DISABLE_ATTR_DCE` escape hatch at the top of `eliminate_dead_functions` (checked after `MOLT_DISABLE_DEAD_FUNC_ELIM`) that skips the attr-conditional logic and falls back to today's unconditional `func_new` treatment. This is the targeted rollback gate.

The modified `func_new`/`func_new_closure` arm in the reference-building loop (currently lines 1803–1809):

```rust
"func_new" | "func_new_closure" => {
    if let Some(callee) = op.s_value.as_ref() && defined.contains(callee.as_str()) {
        if symbol_to_attr.contains_key(callee.as_str()) {
            // Module-attr definition: defer; made live only when the attr is read.
            deferred_by_symbol.insert(callee.clone());
        } else {
            // Non-init func_new (closure creation, etc.): unconditional, same as today.
            refs.insert(callee.clone());
        }
    }
}
```

Add a new arm in the reference-building loop for attr-read ops:

```rust
"module_get_attr" | "module_get_global" | "module_get_name"
| "module_import_from" => {
    // Try to resolve (module_name, attr_name) from the const_str value-flow
    // tracked in a per-function linear pass. Add to pending_attr_reads.
    if let Some((module_name, attr_name)) = resolve_static_attr_read(op, &const_str_map_for_func) {
        pending_attr_reads.insert((module_name, attr_name));
    } else {
        // Dynamic or unresolvable: add the module to wildcard_modules (conservative).
        if let Some(module_name) = module_name_from_module_var(op, &module_var_map_for_func) {
            pending_wildcard_modules.insert(module_name);
        }
        // If module identity also unresolvable: keep-all conservative fallback
        // (add ALL modules to wildcard_modules — this is extremely conservative
        // but only hits on code like `getattr(some_dynamic_module, dynamic_name)`).
    }
}
"module_import_star" => {
    if let Some(module_name) = resolve_module_name_from_star(op, &const_str_map_for_func) {
        pending_wildcard_modules.insert(module_name);
    }
}
```

The iterative drain loop after BFS stabilizes:

```rust
loop {
    let mut new_reachable = false;
    // Drain deferred_by_symbol: any callee whose attr is now live.
    let mut still_deferred = Vec::new();
    for callee in deferred_by_symbol.drain() {
        let (mod_name, attr_name) = symbol_to_attr[&callee].clone();
        if attr_live.contains(&(mod_name.clone(), attr_name.clone()))
            || wildcard_modules.contains(&mod_name)
        {
            if reachable.insert(callee.clone()) {
                queue.push_back(callee);
                new_reachable = true;
            }
        } else {
            still_deferred.push(callee);
        }
    }
    deferred_by_symbol.extend(still_deferred);
    if !new_reachable { break; }
    // Run BFS drain again (may discover new attr reads, unlocking more callees).
    while let Some(current) = queue.pop_front() { /* existing BFS body */ }
}
```

Add observability at the end:

```rust
if std::env::var("MOLT_DEBUG_ATTR_DCE").is_ok() {
    let eliminated = original_count - ir.functions.len();
    for func in &ir.functions { /* nothing changed */ }
    // Report: per-module dropped attrs
    eprintln!("[attr-dce] module='{}' attr_live={} wildcard_modules={} deferred_eliminated={}",
              ...);
}
```

**Python changes** to `_reachable_function_names_for_stdlib_cache` at cli.py:17120:

Add `_collect_module_attr_write_map(functions)` helper (mirrors the Rust logic):

```python
def _collect_module_attr_write_map(
    functions: list[Mapping[str, Any]],
) -> tuple[dict[tuple[str, str], list[str]], set[str]]:
    """
    Returns:
      write_map: {(module_name, attr_name): [callee_symbol]}
      wildcard_modules: set of module names where all attrs are live
    """
```

Scans each function whose name starts with `"molt_init_"`, tracks `const_str` → SSA var → `module_set_attr` chains, same logic as Rust.

Modify the BFS in `_reachable_function_names_for_stdlib_cache`:
- Build `write_map, wildcard_modules_from_write = _collect_module_attr_write_map(functions)`.
- Build `symbol_to_attr` reverse map.
- During BFS scan, recognize `"module_get_attr"`, `"module_get_global"`, `"module_import_from"` ops in `ops`. Extract static attr names from args (requires a per-function `const_str` tracking pass before BFS scan).
- Implement the same iterative deferred drain.

Bump `_SHARED_STDLIB_CACHE_SCHEMA_VERSION` at cli.py:27602 from `"stdlib-v2"` to `"stdlib-v3"`. This forces all existing stdlib caches to be invalidated on next build after landing.

Add `MOLT_DISABLE_ATTR_DCE` check in the Python BFS to mirror Rust (both must use the same logic).

**Unit tests added to `passes.rs`** (8 tests covering the design):

1. `attr_dce_unread_func_new_eliminated` — init defines attr, user never reads it; body eliminated.
2. `attr_dce_read_func_new_kept` — init defines attr; user reads it via `module_get_attr`; body kept.
3. `attr_dce_import_star_keeps_all` — `module_import_star` in user code; all attrs of that module kept.
4. `attr_dce_dynamic_attr_name_conservative` — `module_get_attr` with non-constant attr var; all attrs kept.
5. `attr_dce_closure_in_init_attr_not_read_eliminated` — `func_new_closure` stored as module attr, unread; eliminated.
6. `attr_dce_non_init_func_new_unconditional` — `func_new` inside non-init function (not stored into module dict); callee kept unconditionally.
7. `attr_dce_chained_module_attr_live` — module A defines `f`, stored as `"helper"`; module B reads `"helper"` from A in its init; user reads from B; `f` is live transitively.
8. `attr_dce_getattr_hook_wildcard` — `__getattr__` defined in a module → all that module's attrs live.

**Differential Python test shapes** (run on native + WASM + LLVM, verified byte-identical to CPython 3.12/3.13/3.14):

1. `empty.py` — binary size gate: empty.py native binary ≤ 3.0 MB after this phase.
2. `reads_sorted.py` — `sorted` retained; `filter`, `zip`, `enumerate`, etc. eliminated.
3. `star_import.py` — `from builtins import *` → all builtins kept.
4. `dynamic_getattr.py` — `getattr(builtins, name)` where name is dynamic → all builtins kept.
5. `reads_math.py` — `import math; x = math.sqrt(2.0)` → `sqrt` kept, `factorial`/`gcd`/etc. eliminated.
6. `func_as_variable.py` — `f = builtins.sorted; f([3,1,2])` → `sorted` kept.
7. `bigint_correctness.py` — `lambda a,b: a**b; apply(lambda, 1<<60, 7)` → `1152921504606846983` unchanged.
8. `exception_safety.py` — `from builtins import nonexistent` raises `ImportError` correctly.
9. `import_chain.py` — `import os; os.path.join("a","b")` → join kept, unread functions eliminated.
10. `cross_backend_parity.py` — shapes 1–9 verified on native/WASM/LLVM.

**Acceptance**:
- `cargo test -p molt-backend --features native-backend` passes (882+ tests).
- All 10 differential shapes byte-identical to CPython 3.12/3.13/3.14 on all 3 backends.
- `empty.py` native binary ≤ 3.0 MB (conservative; specific target depends on `__getattr__` coverage).
- All existing benchmarks faster than CPython on native.

---

### Phase 3: Measurement, Observability, and Size Gate

**Files changed**:
- `/Users/adpena/Projects/molt/tools/verify_native_binary_valid.sh` (or new `tools/verify_binary_size_gate.sh`)
- Possibly `/Users/adpena/Projects/molt/Makefile` / CI config

**What lands**:
Add binary size gate: after `molt build --target native empty.py`, assert binary ≤ `BINARY_SIZE_GATE_BYTES` (first setting: 3,500,000 bytes = 3.5 MB). This is conservative; tighten as measurements confirm.

Instrument `MOLT_DEBUG_ATTR_DCE=1` output with:
- Per-module eliminated function count and names.
- Total functions eliminated vs. kept.
- Whether any module hit the wildcard/conservative path.

Add WASM size verification: `molt build --target wasm empty.py` → assert `.wasm` file ≤ threshold (first setting: 1.5 MB, to be calibrated).

**Acceptance**: size gate CI check green; WASM size verified; `MOLT_DEBUG_ATTR_DCE=1` output parseable and accurate.

---

## Soundness Argument

**Claim**: under Tier-0 (no `exec`/`eval`/`compile`/runtime monkeypatching; all attribute accesses are either static constant-name or covered by conservative fallback), a function body `F` defined by `func_new("F")` into `(module M, attr A)` is never executed unless some reachable code reads `(M, A)`.

**Proof**:
1. `func_new("F")` stores a function object wrapping symbol `F` into SSA variable `v`.
2. `module_set_attr(module_M, const_str("A"), v)` writes this object into `M.__dict__["A"]`.
3. Any call to `F` requires: (a) reading `M.__dict__["A"]` via `module_get_attr`/`module_get_global`/etc., (b) then calling the result.
4. Under Tier-0: no `exec`/`eval` can construct `"A"` dynamically; no `__getattr__` hooks (if `__getattr__` is defined, the module is conservatively kept whole); no `import *` from `M` in reachable code (if present, conservative fallback).
5. Therefore, if no reachable function reads `(M, A)`, step 3a never happens, and `F` is never invoked.
6. Eliminating `F` is safe.

**Conservative cases covered**: wildcard_modules (import_star, dynamic attr, __getattr__), unresolvable const_str chain, non-init func_new (unconditional), code_new (unconditional).

---

## Legacy Deleted

The arc does not remove the `func_new` arm from `eliminate_dead_functions` — it refines it. What is removed:

- The **implicit assumption** that every `func_new` inside a reachable function keeps the referenced body alive regardless of whether the resulting function object is ever retrieved via a module attribute read. This assumption is replaced with the two-phase BFS.

No Rust type or public API is deleted. The `_DEAD_FUNCTION_ELIM_REFERENCE_KINDS` constant in cli.py is structurally unchanged (it covers the BFS edge types for the stdlib cache, which is now augmented but not changed in its membership).

---

## Observability Instrument

**`MOLT_DEBUG_ATTR_DCE=1`** (Rust side, in `eliminate_dead_functions`):
```
[attr-dce] module_attr_write_map: N entries across M modules
[attr-dce] deferred_func_new: K symbols
[attr-dce] attr_live: {("builtins","sorted"), ...}  (N_live entries)
[attr-dce] wildcard_modules: {"os", ...}  (N_wild modules)
[attr-dce] per-module: builtins: 87 live / 312 total; sys: 12 live / 89 total; ...
[attr-dce] total: F_kept kept, F_elim eliminated from F_total
```

**`MOLT_DEBUG_ATTR_DCE=1`** (Python side, mirrored in `_reachable_function_names_for_stdlib_cache`):
```
[attr-dce-py] stdlib_cache_reachable: N functions
[attr-dce-py] deferred_eliminated: K function symbols
[attr-dce-py] attr_live: N_live (module,attr) pairs
```

The two-sided instrumentation is the safeguard against drift: if both sides print the same eliminated set for a given program, the cache key and the backend DFE are coherent.

---

## Cache Coherence and Schema Version

The Python BFS (`_reachable_function_names_for_stdlib_cache`, cli.py:17120) drives the stdlib cache key. The Rust DFE (`eliminate_dead_functions`, passes.rs:1786) drives the actual elimination. If they diverge:
- Python BFS eliminates MORE than Rust: cache key uses a smaller function set; Rust retains those functions; the cache object is valid but the main binary has extra functions (over-linking, not miscompile; but confusing).
- Python BFS eliminates LESS than Rust: cache key includes functions Rust will eliminate; the stdlib cache object is over-built but the linker will dead-strip unused symbols (not a correctness issue; minor size overhead).

**The mandate**: both BFSs must implement identical logic. The Phase 2 commit is atomic — both files change together. The `_SHARED_STDLIB_CACHE_SCHEMA_VERSION` bump from `"stdlib-v2"` to `"stdlib-v3"` at cli.py:27602 invalidates all cached stdlib objects on first post-upgrade build.

---

## Current Live Anchors (Verified)

- **`/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs:1786`** — `pub fn eliminate_dead_functions(ir: &mut SimpleIR)` — the BFS entry point that gets the two-phase augmentation.
- **`/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs:1803`** — `"call" | "call_internal" | "func_new" | "func_new_closure" | "func_new_builtin" | "code_new" | "call_guarded"` — the arm that splits `func_new`/`func_new_closure` into conditional vs. unconditional.
- **`/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs:1787`** — `MOLT_DISABLE_DEAD_FUNC_ELIM` escape hatch (already exists); add `MOLT_DISABLE_ATTR_DCE` immediately after.
- **`/Users/adpena/Projects/molt/src/molt/cli.py:17082`** — `_DEAD_FUNCTION_ELIM_REFERENCE_KINDS = frozenset(...)` — structurally unchanged.
- **`/Users/adpena/Projects/molt/src/molt/cli.py:17120`** — `def _reachable_function_names_for_stdlib_cache(ir)` — the Python BFS mirror that gains `_collect_module_attr_write_map` and per-function attr-read tracking.
- **`/Users/adpena/Projects/molt/src/molt/cli.py:27602`** — `_SHARED_STDLIB_CACHE_SCHEMA_VERSION = "stdlib-v2"` — bump to `"stdlib-v3"` in Phase 2 commit.
- **`/Users/adpena/Projects/molt/src/molt/frontend/__init__.py:33870`** — `elif op.kind == "FUNC_NEW":` JSON serialization — confirms `s_value` is the callee symbol, `args` is empty; `module_set_attr` serializes args as SSA variable names.
- **`/Users/adpena/Projects/molt/src/molt/frontend/__init__.py:34233`** — `elif op.kind == "MODULE_SET_ATTR":` JSON serialization — confirms `args = [module_var, attr_var, value_var]` (all SSA variable names; attr name requires const_str back-trace).
- **`/Users/adpena/Projects/molt/src/molt/frontend/__init__.py:34190`** — `elif op.kind == "MODULE_GET_ATTR":` JSON serialization — same structure as `module_set_attr` (attr name in args[1] via const_str).
- **`/Users/adpena/Projects/molt/src/molt/frontend/__init__.py:5401`** — `def _emit_module_attr_set(self, name, value, *, defer=True)` — frontend emitter that produces `MODULE_SET_ATTR` ops with a const_str for the name; this is the shape the write-map scanner must recognize.
- **`/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/module_phase.rs`** — no changes; per-attribute DCE fires at SimpleIR layer before TIR lifting.
- **`/Users/adpena/Projects/molt/runtime/molt-backend/src/wasm.rs:2223`** — `eliminate_dead_functions(&mut ir)` call in WASM path — inherits the refined DFE automatically; no WASM-specific changes required.
- **`/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/simple_backend.rs:2666`** — second `eliminate_dead_functions` call site in the native non-split path — inherits automatically.
- **`/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/simple_backend.rs:1779`** — `eliminate_dead_functions` call in `prune_and_partition_native_stdlib` — inherits automatically; this is the stdlib-split-object path and is the most important call site (it runs before the stdlib/user-code partition).
- **`/Users/adpena/Projects/molt/runtime/molt-backend/src/ir.rs:46`** — `OpIR` struct definition; `s_value: Option<String>` carries the callee symbol for `func_new`; `args: Option<Vec<String>>` carries the SSA var names for `module_set_attr`/`module_get_attr`.

---

## Essential Files for Implementation

1. `/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs` — primary change target (DFE BFS augmentation + unit tests)
2. `/Users/adpena/Projects/molt/src/molt/cli.py` — Python BFS mirror + schema version bump
3. `/Users/adpena/Projects/molt/runtime/molt-backend/src/ir.rs` — `OpIR` struct reference (read-only)
4. `/Users/adpena/Projects/molt/src/molt/frontend/__init__.py` — IR shape reference for `FUNC_NEW`/`MODULE_SET_ATTR`/`MODULE_GET_ATTR` serialization (read-only)
5. `/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/simple_backend.rs` — DFE call sites (inherit automatically; verify line 1779, 2666)
6. `/Users/adpena/Projects/molt/runtime/molt-backend/src/wasm.rs` — DFE call site (inherit automatically; verify line 2223)
7. `/Users/adpena/Projects/molt/docs/design/foundation/09_W3-deadfield.md` — architect's blueprint (authoritative design source; this plan extends and concretizes it with live line numbers)
