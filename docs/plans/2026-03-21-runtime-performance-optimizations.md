# Molt Runtime Performance Optimizations Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Improve runtime performance by 2-5x for OOP-heavy Python code through inline caching for attribute access and reference counting coalescing, benefiting both native and WASM targets.

**Architecture:** Two complementary optimizations: (1) Inline caching adds per-call-site caches for attribute lookups, avoiding full MRO traversal on repeated access to the same attribute. (2) RC coalescing eliminates redundant inc_ref/dec_ref pairs within basic blocks where a value is assigned then immediately reassigned.

**Tech Stack:** Rust (Molt runtime + backend), Cranelift IR

**Applies to:** Both native and WASM targets

---

### Task 1: Reference counting coalescing in the backend

The backend emits `inc_ref` after every value load and `dec_ref` before every value death. Many of these cancel out — e.g., `x = foo(); x = bar()` emits inc_ref(foo_result), dec_ref(foo_result), inc_ref(bar_result). The dec_ref + inc_ref pair on foo_result is wasted.

**Files:**
- Modify: `runtime/molt-backend/src/lib.rs` — add RC coalescing pass

- [ ] **Step 1: Identify RC emission points**

```bash
grep -n "emit_inc_ref\|emit_dec_ref\|inc_ref_obj\|dec_ref_bits\|ref_adjust" runtime/molt-backend/src/lib.rs | wc -l
```

- [ ] **Step 2: Implement basic block RC analysis**

After compiling all ops in a basic block, scan for patterns:
- `inc_ref(X)` followed by `dec_ref(X)` with no intervening use → eliminate both
- `dec_ref(X)` followed by `inc_ref(X)` with no intervening use → eliminate both
- Multiple `inc_ref(X)` → coalesce to single `inc_ref_n(X, count)`

- [ ] **Step 3: Add `molt_inc_ref_n` and `molt_dec_ref_n` batched helpers**

```rust
#[unsafe(no_mangle)]
pub extern "C" fn molt_inc_ref_n(bits: u64, count: u32) -> u64 {
    // Increment refcount by count in a single atomic operation
}
```

- [ ] **Step 4: Benchmark with a tight loop**

Create a test Python script that does many assignments in a loop and measure before/after.

- [ ] **Step 5: Commit**

---

### Task 2: Inline caching for attribute access

Every `obj.attr` does full MRO traversal. Add per-call-site inline caches that check a type version counter and return the cached result on hit.

**Files:**
- Modify: `runtime/molt-backend/src/lib.rs` — emit inline cache check before attr lookup
- Modify: `runtime/molt-runtime/src/builtins/attr.rs` — add cache update on miss
- Modify: `runtime/molt-runtime/src/object/mod.rs` — add version counter to type objects

- [ ] **Step 1: Add version counter to type objects**

Each class/type gets a monotonic version counter that increments when the class is modified (attribute set/deleted, base class changed).

- [ ] **Step 2: Define inline cache structure**

```rust
/// Per-call-site inline cache for attribute access.
/// Stored in the compiled code's data section.
struct AttrInlineCache {
    type_version: u64,    // Expected type version
    attr_bits: u64,       // Cached attribute name
    result_bits: u64,     // Cached result
    result_offset: u32,   // Offset into type's attribute table
}
```

- [ ] **Step 3: Emit inline cache check in backend**

For each `LOAD_ATTR` op, emit:
```
cache_ptr = address of this call site's cache
if obj.type.version == cache.type_version && attr == cache.attr_bits:
    result = cache.result_bits  // cache hit
else:
    result = molt_load_attr_slow(obj, attr, cache_ptr)  // fills cache
```

- [ ] **Step 4: Implement `molt_load_attr_slow` with cache fill**

On cache miss, do the full MRO lookup, then update the inline cache for next time.

- [ ] **Step 5: Benchmark attribute-heavy code**

Test with code that repeatedly accesses the same attributes on objects of the same type.

- [ ] **Step 6: Commit**

---

### Task 3: WASM code splitting (lazy module loading)

Split the compiled WASM output into per-module chunks that load on demand, reducing initial load time and memory.

**Files:**
- Modify: `src/molt/cli.py` — emit separate WASM modules per stdlib import
- Modify: `runtime/molt-backend/src/lib.rs` — support multi-module WASM output

- [ ] **Step 1: Analyze current WASM output structure**

- [ ] **Step 2: Design module boundary API**

- [ ] **Step 3: Implement lazy loading for stdlib modules**

- [ ] **Step 4: Measure load time improvement**

- [ ] **Step 5: Commit**
