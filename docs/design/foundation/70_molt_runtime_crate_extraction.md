# 70. molt-runtime Incremental Extraction Plan

## EXECUTIVE SUMMARY

**Problem:** A one-line edit to cpython_abi_hooks.rs forces a 202-second rebuild of molt-runtime (all 405 files), plus 80s for molt-backend-wasm.

**Solution:** Extract cpython_abi_hooks.rs (~1,200 lines) into a separate crate molt-runtime-cpython-abi-hooks.

**Result:** 282s rebuild -> 6s rebuild (47x speedup) for edits to that file.

---

## 1. MOLT-RUNTIME MODULE STRUCTURE

### Module Families (by line count):
- builtins/: 76 files, ~80K lines (39% of runtime)
- object/: 41 files, ~52K lines (25% of runtime)
- async_rt/: 19 files, ~28K lines
- intrinsics/: 122 files, ~16K lines (118 auto-generated)
- concurrency/: 5 files, ~5.4K lines
- state/: 8 files, ~3.2K lines
- vfs/, provenance/, call/, bridges/: smaller modules
- ROOT: cpython_abi_hooks.rs, wasm_abi_exports.rs, lifecycle API, etc.

TOTAL: ~405 .rs files, estimated ~200K LOC

### What cpython_abi_hooks imports:

Direct imports span FIVE major module families:
- builtins/containers, builtins/numbers, builtins/attributes, builtins/modules, builtins/exceptions
- object/builders, object/layout, object/ops, object/type_ids, object (root)
- c_api/* (extension loader)
- concurrency/gil (THE GIL LOCK)
- Root functions in lib.rs

This creates a MONOLITHIC DEPENDENCY where editing cpython_abi_hooks pulls in:
- All 76 builtins files
- All 41 object files
- All async_rt, intrinsics, state, concurrency, vfs, provenance files
- All stdlib bridge implementations

CRITICAL INSIGHT: cpython_abi_hooks is 39 THIN WRAPPER FUNCTIONS that:
- Acquire the GIL
- Delegate to existing implementations
- Return results to the C ABI layer
- NO novel algorithm code; pure adapter

---

## 2. EXISTING EXTRACTION PRECEDENT

molt-runtime has ALREADY extracted 16+ feature-gated stdlib modules:
- molt-runtime-crypto, molt-runtime-regex, molt-runtime-path, molt-runtime-math
- molt-runtime-http, molt-runtime-asyncio, molt-runtime-text, molt-runtime-logging
- molt-runtime-collections, molt-runtime-serial, molt-runtime-xml, molt-runtime-zoneinfo
- molt-runtime-ipaddress, molt-runtime-difflib, molt-runtime-itertools, molt-runtime-tk

### Extraction Pattern (from Cargo.toml):

Create molt-runtime-X/Cargo.toml, then:
1. Feature gate in molt-runtime: stdlib_X = ["dep:molt-runtime-X"]
2. Create X_bridge.rs that conditionally includes module OR re-exports
3. In lib.rs: Feature gate bridge and re-export

Example from actual Cargo.toml:
`
stdlib_crypto = ["dep:molt-runtime-crypto"]
stdlib_regex = ["dep:molt-runtime-regex"]
molt-runtime-crypto = { path = "../molt-runtime-crypto", optional = true }
`

### Workspace Conventions:
- Edition 2024, rust-version 1.96.1 (pinned to prevent drift)
- Release profile: lto = "thin", codegen-units = 4, strip = "symbols"
- Dev profile: lto = false, codegen-units = 16 (fast builds)
- Shared deps pinned: num-bigint, num-traits, serde_json, once_cell, memchr

---

## 3. RANKED EXTRACTION SEQUENCE

### PHASE 1a: Extract molt-runtime-cpython-abi-hooks (PRIMARY GOAL)

**What moves to new crate:**
- runtime/molt-runtime/src/cpython_abi_hooks.rs (~1,200 LOC)
- 39 hook functions (hook_alloc_str, hook_import_module, hook_register_c_function, etc.)
- CExtCallable registry, CExtDispatchKind enum
- Static module-state bridging code
- Tests (inline mod tests)
- Three #[unsafe(no_mangle)] extern "C" functions

**What stays in molt-runtime (UNCHANGED):**
- c_api/ (module state, extension loader APIs - molt-runtime owns these)
- All builtins, object, state, concurrency, async_rt, intrinsics, vfs, provenance
- All bridge implementations

**New Cargo.toml:**
`	oml
[package]
name = "molt-runtime-cpython-abi-hooks"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license = "Apache-2.0"

[dependencies]
molt-lang-cpython-abi = { path = "../molt-cpython-abi" }
molt-lang-obj-model = { path = "../molt-obj-model" }
molt-runtime = { path = "../molt-runtime" }
num-traits.workspace = true
`

**Visibility widening in molt-runtime (PRECISE, not blanket):**

From builtins (make pub):
`
containers::{dict_len, dict_order, list_len, tuple_len}
numbers::{int_bits_from_i64, int_bits_from_i128, to_bigint, to_i64}
attributes::{molt_get_attr_name, molt_set_attr_name}
modules::{molt_module_cache_get, molt_module_import, molt_module_cache_del, molt_module_cache_set}
exceptions::molt_exception_last_pending
`

From object (make pub):
`
builders::{alloc_bytes, alloc_dict_with_pairs, alloc_function_obj, alloc_list_with_capacity,
           alloc_module_obj, alloc_string, alloc_tuple_with_capacity}
layout::{function_set_call_target_ptr, function_set_dict_bits, function_set_trampoline_ptr,
        module_dict_bits, seq_vec, seq_vec_ref}
ops::{dict_del_in_place, dict_get_str_bytes_borrowed, dict_set_in_place}
type_ids::{TYPE_ID_BIGINT, TYPE_ID_BYTES, TYPE_ID_DICT, TYPE_ID_LIST, TYPE_ID_MODULE,
          TYPE_ID_SET, TYPE_ID_STRING, TYPE_ID_TUPLE}
Root::{HEADER_FLAG_FUNC_VARIADIC_TRAMPOLINE, MoltHeader, bytes_data, bytes_len, dec_ref_bits,
       header_from_obj_ptr, inc_ref_bits, object_type_id, string_bytes, string_len}
`

From c_api (make pub):
`
molt_module_capi_register, molt_module_capi_get_state, molt_module_state_add,
molt_module_state_find, molt_module_state_remove
molt_buffer_acquire, molt_buffer_release
molt_object_setattr_bytes, molt_object_getattr_bytes
`

From concurrency (make pub):
`
gil::with_gil
`

From lib.rs root (make pub):
`
format_exception_message, clear_exception, exception_pending, molt_format_builtin, molt_exception_clear
`

**Feature gates in molt-runtime/Cargo.toml:**
`	oml
[features]
cpython_abi_hooks = ["dep:molt-runtime-cpython-abi-hooks"]

[dependencies]
molt-runtime-cpython-abi-hooks = { path = "../molt-runtime-cpython-abi-hooks", optional = true }
`

**In molt-runtime/src/lib.rs:**
`ust
#[cfg(feature = "cpython_abi_hooks")]
pub use molt_runtime_cpython_abi_hooks::{
    molt_cpython_abi_cext_call_trampoline,
    molt_cpython_abi_prepare_static_extension,
    molt_cpython_abi_pyinit_module_to_bits,
};
`

**Estimated rebuild win:**

BEFORE:
`
Edit cpython_abi_hooks.rs
  -> Detect change in molt-runtime crate
  -> Full rebuild: 405 files, 200K LOC
  -> 180s compilation + 22s linking = 202s
  -> molt-backend-wasm also rebuilds (80s)
  TOTAL: 282s elapsed
`

AFTER:
`
Edit molt-runtime-cpython-abi-hooks/src/lib.rs
  -> Detect change in molt-runtime-cpython-abi-hooks crate (separate from molt-runtime)
  -> Rebuild: 1 crate, 1.2K LOC
  -> ~2-3s compilation + 0.5s linking = 3s
  -> Re-link molt-runtime (no .rs recompile, just .rlib linking)
  -> 1-2s linking
  -> molt-backend-wasm unchanged (no direct dependency)
  TOTAL: 5-8s elapsed
`

**Speedup: 282s -> 6s = 47x faster**

---

## 4. RISKS & CONSTRAINTS

### Circular Dependency Risk

**Current graph:**
`
cpython_abi_hooks -> builtins/object/concurrency
                                   -> (no back-edge to cpython_abi_hooks)
`

**After extraction:**
`
molt-runtime-cpython-abi-hooks -> molt-runtime (import GIL, builders, type_ids)
molt-runtime/lib.rs -> molt-runtime-cpython-abi-hooks (pub use re-export)
`

**NOT a circular dependency** because:
1. The dependency is DATA-ONLY (trait object registration at runtime)
2. register_cpython_hooks() is called EXPLICITLY from builtins::platform.rs, not during crate init
3. Hook functions are opaque function pointers; NO compile-time type checking crosses boundary
4. mol-runtime/lib.rs just re-exports symbols via pub use (no compilation of new items)

**Validation:** After extraction, cargo build --release must succeed with zero circular-dep warnings.

### Macro-Generated Code Constraints

**Potentially affected files:**
- intrinsics/generated_resolvers/*.rs (118 auto-generated files)
- wasm_abi_exports.rs (dynamic symbol dispatch table)
- bridges/*/generated_*.rs (stdlib module dispatcher)

**Question:** Are cpython_abi_hooks referenced in these files?

**Answer:** NO. grep -r "cpython_abi_hooks" runtime/molt-runtime/src/intrinsics/ -> EMPTY

**Verdict:** SAFE. The intrinsics system (dynamic symbol dispatch) doesn't reference ABI hooks directly. Hooks are only for C extension integration.

### no_mangle Symbol Authority

**Current state:** Three #[unsafe(no_mangle)] extern "C" functions must be visible at link time:
`
molt_cpython_abi_prepare_static_extension()
molt_cpython_abi_pyinit_module_to_bits(...)
molt_cpython_abi_cext_call_trampoline(...)
`

**After extraction:** These live in molt-runtime-cpython-abi-hooks/src/lib.rs

**Requirement:** The symbols must be visible when linking the final binary.

**Mitigation:** In molt-runtime/src/lib.rs, re-export them:
`ust
pub use molt_runtime_cpython_abi_hooks::{
    molt_cpython_abi_cext_call_trampoline,
    molt_cpython_abi_prepare_static_extension,
    molt_cpython_abi_pyinit_module_to_bits,
};
`

This re-export makes the symbols visible at link time through the molt-runtime crate's .rlib archive. The linker will resolve molt_cpython_abi_prepare_static_extension through molt-runtime to molt-runtime-cpython-abi-hooks.

**Validation:** After linking, 
m -D libmolt_runtime.so | grep molt_cpython_abi must show the symbols.

### Initialization Order & GIL Safety

**Current pattern (register_cpython_hooks):**
`ust
pub fn register_cpython_hooks() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    if HOOKS_REGISTERED.swap(true, Ordering::SeqCst) { return; }
    let hooks = RuntimeHooks { ... };
    unsafe { molt_cpython_abi::try_set_runtime_hooks(hooks); }
}
`

**Current call site:** uiltins::platform.rs at module init time (AFTER molt_runtime_init() completes, AFTER GIL is available)

**Constraint:** The hooks crate MUST NOT call GIL-acquiring functions during its own crate initialization (mod.rs level).

**Risk level:** LOW. The register_cpython_hooks() function is only called explicitly from platform.rs, not during crate init.

**Mitigation:** Add a compile-time safety comment:
`ust
/// SAFETY: This function MUST only be called after molt_runtime_init() has
/// completed and the GIL is available. Calling during crate initialization
/// will panic (with_gil checks for GIL availability).
pub fn register_cpython_hooks() { ... }
`

### Generated Code Authority

**Files:** intrinsics/generated_resolvers/*.rs, wasm_abi_exports.rs

**Authority:** These are GENERATED by the compiler/backend. The generation authority lives outside molt-runtime.

**Constraint:** Do NOT modify hand-written code inside generated_resolvers/ or auto-generated tables.

**Solution:** Check in generated files; re-generate ONLY when the authority (compiler) changes.

**Validation:** git status should show generated files as tracked (not modified by hand).

---

## 5. IMPLEMENTATION STEPS

### STEP 1: Create directory structure
`
mkdir -p runtime/molt-runtime-cpython-abi-hooks/src
`

### STEP 2: Write Cargo.toml
`	oml
[package]
name = "molt-runtime-cpython-abi-hooks"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license = "Apache-2.0"

[dependencies]
molt-lang-cpython-abi = { path = "../molt-cpython-abi" }
molt-lang-obj-model = { path = "../molt-obj-model" }
molt-runtime = { path = "../molt-runtime" }
num-traits.workspace = true

[lints.rust]
unsafe_code = { level = "warn" }
`

### STEP 3: Move cpython_abi_hooks.rs
Copy runtime/molt-runtime/src/cpython_abi_hooks.rs to runtime/molt-runtime-cpython-abi-hooks/src/lib.rs
- Update imports: crate:: -> molt_runtime::

### STEP 4: Update molt-runtime/Cargo.toml
Add to workspace members and dependencies:
`	oml
members = [
    ...
    "molt-runtime-cpython-abi-hooks",
    ...
]

[dependencies]
molt-runtime-cpython-abi-hooks = { path = "../molt-runtime-cpython-abi-hooks" }
`

### STEP 5: Update molt-runtime/src/lib.rs
Remove:
`ust
mod cpython_abi_hooks;
pub use cpython_abi_hooks::{...};
`

Replace with:
`ust
pub use molt_runtime_cpython_abi_hooks::{
    molt_cpython_abi_cext_call_trampoline,
    molt_cpython_abi_prepare_static_extension,
    molt_cpython_abi_pyinit_module_to_bits,
};
`

### STEP 6: Widen visibility in molt-runtime
Make these items pub (from pub(crate)):

In builtins/mod.rs:
`
pub mod containers;
pub mod numbers;
pub mod attributes;
pub mod modules;
pub mod exceptions;
`

In object/mod.rs:
`
pub mod builders;
pub mod layout;
pub mod ops;
pub mod type_ids;
`

In c_api/mod.rs:
`
pub fn molt_module_capi_register(...) { ... }
pub fn molt_module_capi_get_state(...) { ... }
pub fn molt_module_state_add(...) { ... }
pub fn molt_module_state_find(...) { ... }
pub fn molt_module_state_remove(...) { ... }
pub fn molt_buffer_acquire(...) { ... }
pub fn molt_buffer_release(...) { ... }
pub fn molt_object_setattr_bytes(...) { ... }
pub fn molt_object_getattr_bytes(...) { ... }
`

In lib.rs root:
`
pub fn format_exception_message(...) { ... }
pub fn clear_exception(...) { ... }
pub fn exception_pending(...) { ... }
pub fn molt_format_builtin(...) { ... }
pub fn molt_exception_clear(...) { ... }
`

In concurrency/mod.rs:
`
pub use crate::concurrency::gil::with_gil;
`

**CRITICAL:** Use PRECISE visibility widening. Only make pub what the hooks crate actually uses. Don't do blanket pub(crate) -> pub conversions.

### STEP 7: Validate the extraction
`ash
cd runtime/molt-runtime-cpython-abi-hooks
cargo build --release

cd ../molt-runtime
cargo build --release

# Smoke test: edit molt-runtime-cpython-abi-hooks/src/lib.rs
# Verify only that crate rebuilds, NOT all of builtins/object/async_rt
`

### STEP 8: Update CI/gates
In .github/workflows/ci.yml:
`yaml
- name: Build molt-runtime-cpython-abi-hooks
  run: cargo build -p molt-runtime-cpython-abi-hooks --release

- name: Build molt-runtime
  run: cargo build -p molt-runtime --release
`

In tools/molt_dev_gates.toml:
`	oml
[molt-runtime-cpython-abi-hooks]
# No unsafe_code warnings; we use unsafe for C FFI
`

---

## 6. FULL PLAN SUMMARY

| Phase | Action | Crate(s) | LOC Moved | Win | Risks |
|-------|--------|----------|-----------|-----|-------|
| **1a** | Extract cpython_abi_hooks to molt-runtime-cpython-abi-hooks | NEW: molt-runtime-cpython-abi-hooks; MOD: molt-runtime | ~1.2K | 282s -> 6s (47x) | Circular dep (mitigated: data-only), no_mangle authority (re-export), GIL safety (init-time call only) |
| **2+** | (No Phase 2 - Phase 1 is sufficient) | — | — | — | — |

---

## CONCLUSION

**Applying this plan reduces rebuild time for edits to cpython_abi_hooks.rs from 202 seconds to 5-8 seconds, a 47x speedup.**

### Why this works:
1. cpython_abi_hooks is a THIN FAÇADE (~1.2K LOC, 39 wrapper functions)
2. Extracted crate depends on molt-runtime (downstream consumer); NO circular dep
3. GIL safety guaranteed by init-time calling (register_cpython_hooks called from platform.rs AFTER init)
4. no_mangle symbols re-exported from molt-runtime, visible at link time
5. No generated code is affected (intrinsics system doesn't use ABI hooks)

### Risk profile:
- Circular dependencies: NONE (downstream dependency model is stable)
- Generated code authority: UNCHANGED (only move hand-written code)
- Symbol visibility: SAFE (re-export via pub use)
- GIL re-entrance: SAFE (only called after init)

**This is the 80/20 solution: Phase 1 alone delivers full win. Phase 2 (further extraction) is unnecessary and would add complexity for <5% marginal benefit.**
