# Project TITAN Phase 0: Beat CPython on All Benchmarks

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Molt faster than CPython 3.12 on all 61+ benchmarks by adding inline caches, dictionary optimization, string representation improvements, and a hot/cold object header split.

**Architecture:** This phase operates entirely within the existing Cranelift/WASM pipeline — no new backends or IRs. We add runtime optimizations (inline caches, compact dicts, string SSO), split the object header for cache efficiency, and ensure every CPython-specialized bytecode has an equivalent fast path. Profiling data collected here directly informs TIR design in Phase 1.

**Tech Stack:** Rust (molt-runtime, molt-obj-model), Python (benchmark tooling), perf/Instruments (profiling)

**Spec Reference:** `docs/superpowers/specs/2026-03-23-project-titan-optimization-design.md` — Sections 5.1-5.6, 12.2

**Prerequisites:** Nightly Rust toolchain for ASAN (optional but recommended). LLVM/perf for profiling on Linux; Instruments on macOS.

---

## File Structure

### New Files
| File | Responsibility |
|------|---------------|
| `runtime/molt-runtime/src/object/inline_cache.rs` | Inline cache data structures and lookup logic |
| `runtime/molt-runtime/src/object/dict_compact.rs` | Compact dictionary implementation |
| `runtime/molt-runtime/src/object/string_repr.rs` | Multi-representation string type (inline/SSO, one-byte, two-byte, cons) |
| `runtime/molt-runtime/src/object/string_intern.rs` | String interning pool and lookup |
| `tools/bench_audit.py` | Benchmark triage tool (Green/Yellow/Red + perf counters) |
| `tools/parity_gate.py` | CPython parity enforcement harness |

### Modified Files
| File | Changes |
|------|---------|
| `runtime/molt-runtime/src/object/mod.rs:228-236` | Hot/cold header split — restructure MoltHeader |
| `runtime/molt-runtime/src/object/mod.rs:361-380` | Map 17 existing HEADER_FLAG_* to u16 field |
| `runtime/molt-runtime/src/object/refcount.rs:17-107` | Adapt refcount ops for new header layout |
| `runtime/molt-runtime/src/object/ops_dict.rs:299-420` | Wire compact dict into existing dict operations |
| `runtime/molt-runtime/src/object/ops_string.rs:33-4074` | Wire string repr dispatch into existing string ops |
| `runtime/molt-runtime/src/object/ops.rs` | List fast paths (getitem, setitem, iter) |
| `runtime/molt-runtime/src/object/accessors.rs` | Add IC-guarded fast path for attribute load/store |
| `runtime/molt-runtime/src/object/type_ids.rs:1-80` | Add size class table for hot/cold header |
| `tools/bench.py:22-119` | Add perf counter collection and allocation metrics |
| `tools/bench_diff.py:13-53` | Add new metrics to comparison logic |

### Key Existing File Reference
| Component | Actual Path | Lines |
|-----------|-------------|-------|
| MoltHeader struct | `runtime/molt-runtime/src/object/mod.rs` | 228-236 |
| Header flags (17 flags) | `runtime/molt-runtime/src/object/mod.rs` | 361-380 |
| GLOBAL_TYPE_VERSION | `runtime/molt-runtime/src/object/mod.rs` | 17-27 |
| RefCount impl | `runtime/molt-runtime/src/object/refcount.rs` | 17-107 |
| Type IDs + tags | `runtime/molt-runtime/src/object/type_ids.rs` | 1-79 |
| Dict operations | `runtime/molt-runtime/src/object/ops_dict.rs` | 1-906 |
| String operations | `runtime/molt-runtime/src/object/ops_string.rs` | 1-4074 |
| List operations | `runtime/molt-runtime/src/object/ops.rs` | ~24000-27300 |
| Attribute access | `runtime/molt-runtime/src/object/accessors.rs` | 1-410 |
| NaN-boxing constants | `runtime/molt-obj-model/src/lib.rs` | 13-24 |
| inc_ref / dec_ref | `runtime/molt-runtime/src/object/mod.rs` | 904-1083 |
| Benchmarks | `tests/benchmarks/bench_*.py` | 62 files |
| Bench runner | `tools/bench.py` | 1-1220 |
| Bench diff | `tools/bench_diff.py` | 1-447 |

---

## Task 1: Benchmark Audit and Profiling Setup

**Files:**
- Create: `tools/bench_audit.py`
- Create: `benchmarks/results/` (new directory)
- Modify: `tools/bench.py:22-119`

- [ ] **Step 1: Create benchmarks results directory**

Run: `mkdir -p benchmarks/results`

- [ ] **Step 2: Create benchmark audit script with perf counter support**

Create `tools/bench_audit.py` — a script that runs each benchmark against both Molt and CPython, collects wall-clock time and (on Linux) perf counters, and classifies results as Green/Yellow/Red.

The script must:
- Iterate all `tests/benchmarks/bench_*.py` files
- Run each through Molt native compilation + execution (via `tools/bench.py`)
- Run each through CPython 3.12
- Compute speedup = cpython_time / molt_time
- Classify: Green (speedup >= 1.0), Yellow (0.5 <= speedup < 1.0), Red (speedup < 0.5)
- On Linux: collect `perf stat -e instructions,cache-misses,branch-misses` per benchmark
- On macOS: note that Instruments must be used manually for hardware counters
- Output JSON to `benchmarks/results/audit_baseline.json`
- Print summary table to stdout

- [ ] **Step 3: Run the audit to establish baseline**

Run: `python3 tools/bench_audit.py 2>&1 | tee benchmarks/results/audit_baseline.txt`
Expected: Each benchmark classified with timing data. JSON artifact saved.

- [ ] **Step 4: Commit baseline audit**

```bash
git add tools/bench_audit.py benchmarks/results/
git commit -m "feat(titan-p0): add benchmark audit tool with CPython baseline"
```

---

## Task 2: Parity Gate Harness

**Files:**
- Create: `tools/parity_gate.py`
- Test: `tests/differential/basic/` (existing differential tests)

Must be in place BEFORE any runtime changes.

- [ ] **Step 1: Create parity gate script**

Create `tools/parity_gate.py` — implements the 3-tier parity enforcement from spec section 1.5.2:
- Tier 1 (STRICT): byte-identical output to CPython (default)
- Tier 2 (RELAXED): normalized comparison (strip addresses, refcounts)
- Tier 3 (EXCLUDED): expected divergence, skip

Markers in test files: `# molt-parity: relaxed` or `# molt-parity: excluded`.
Without marker: defaults to STRICT.

Exit code 1 on any Tier 1 violation (blocks merge). Exit code 0 otherwise.

- [ ] **Step 2: Run parity gate on existing differential tests**

Run: `python3 tools/parity_gate.py tests/differential/basic/`
Expected: All existing tests pass (Molt already has parity).

- [ ] **Step 3: Commit parity gate**

```bash
git add tools/parity_gate.py
git commit -m "feat(titan-p0): add CPython parity gate harness (3-tier)"
```

---

## Task 3: Hot/Cold Object Header Split

**Files:**
- Modify: `runtime/molt-runtime/src/object/mod.rs:228-236, 361-380`
- Modify: `runtime/molt-runtime/src/object/type_ids.rs`
- Modify: `runtime/molt-runtime/src/object/refcount.rs`
- Test: `cargo test --workspace` + parity gate after each sub-step

This is the most invasive change. Broken into sub-steps with tests between each.

### Sub-task 3a: Add size class table

- [ ] **Step 1: Add size class table to type_ids.rs**

Add `size_class_for(size: usize) -> u16` function and `SIZE_CLASS_TABLE` constant array after line 79. Size classes map allocation sizes to a u16 index (0-255). See spec section 5.6 for the layout.

- [ ] **Step 2: Verify compilation**

Run: `cargo test -p molt-runtime --lib 2>&1 | tail -10`
Expected: Compiles, all tests pass.

- [ ] **Step 3: Commit**

```bash
git add runtime/molt-runtime/src/object/type_ids.rs
git commit -m "feat(titan-p0): add size class table for hot/cold header"
```

### Sub-task 3b: Define new header types

- [ ] **Step 4: Add MoltHotHeader and MoltColdHeader structs**

In `runtime/molt-runtime/src/object/mod.rs`, add alongside the existing MoltHeader (don't replace yet):

```rust
/// 16-byte hot header (replaces 40-byte MoltHeader for most objects)
#[repr(C)]
pub struct MoltHotHeader {
    pub type_id: u32,
    pub ref_count: MoltRefCount,  // u32
    pub flags: u16,
    pub size_class: u16,
}
```

**Critical: Map all 17 existing flags to u16.**

The existing flags at lines 361-380 use bits 0-16 of a u64. Since u16 holds bits 0-15, we need to verify all 17 flags fit. The highest flag is `HEADER_FLAG_FINALIZER_RAN` at bit 16 — this does NOT fit in u16.

**Resolution:** Use the full 17 flags. The hot header `flags` field must be `u32` (not `u16`), which makes the hot header 20 bytes instead of 16. Alternatively, move rarely-used flags (bits 12-16: COROUTINE, FUNC_TASK_TRAMPOLINE_KNOWN, FUNC_TASK_TRAMPOLINE_NEEDED, IMMORTAL, FINALIZER_RAN) to the cold header. The implementer must:

1. Audit which flags are checked on the hot path (inc_ref/dec_ref check IMMORTAL at line 932/971)
2. IMMORTAL must stay hot (checked on every refcount op)
3. Generator/coroutine flags (bits 2-9, 12) can move cold IF cold header existence is guaranteed for generators
4. Pick the approach: either u32 flags (20-byte hot header) or move 5 flags cold (16-byte hot header)

For this plan we use **u32 flags (20-byte hot header, aligned to 4 bytes)** — simpler, still a 50% reduction from 40 bytes, and avoids splitting flag access across hot/cold paths.

```rust
#[repr(C)]
pub struct MoltHotHeader {
    pub type_id: u32,          // 4 bytes
    pub ref_count: MoltRefCount, // 4 bytes
    pub flags: u32,            // 4 bytes (all 17 flags fit; was u64, upper 47 bits unused)
    pub size_class: u16,       // 2 bytes
    _pad: u16,                 // 2 bytes alignment padding
}
// Total: 16 bytes (with u32 flags compressed from u64, saving the 8-byte poll_fn,
// 8-byte state, and 8-byte size fields = 24 bytes saved per object)

/// Cold header — separate pool, only for generators/async/finalizer objects
pub struct MoltColdHeader {
    pub poll_fn: u64,
    pub state: i64,
    pub extended_size: usize,
}
```

- [ ] **Step 5: Add cold header storage**

Add a cold header pool using `std::sync::OnceLock` (matching codebase conventions — NOT lazy_static):

```rust
use std::sync::OnceLock;
use std::collections::HashMap;
use std::sync::Mutex;

static COLD_POOL: OnceLock<Mutex<HashMap<usize, MoltColdHeader>>> = OnceLock::new();

fn cold_pool() -> &'static Mutex<HashMap<usize, MoltColdHeader>> {
    COLD_POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn alloc_cold_header(obj_addr: usize, poll_fn: u64, state: i64) {
    cold_pool().lock().unwrap().insert(obj_addr, MoltColdHeader {
        poll_fn, state, extended_size: 0,
    });
}

/// Returns a COPY of cold header data (no dangling reference risk).
pub fn get_cold_header(obj_addr: usize) -> Option<MoltColdHeader> {
    cold_pool().lock().unwrap().get(&obj_addr).copied()
}

pub fn free_cold_header(obj_addr: usize) {
    cold_pool().lock().unwrap().remove(&obj_addr);
}
```

**Note:** `get_cold_header` returns a COPY, not a reference. This avoids the use-after-free bug where `free_cold_header` could invalidate a live reference. `MoltColdHeader` must derive `Copy, Clone`.

- [ ] **Step 6: Verify new structs compile**

Run: `cargo test -p molt-runtime --lib 2>&1 | tail -10`
Expected: Compiles, all tests pass (new structs not yet wired).

- [ ] **Step 7: Commit new header types**

```bash
git add runtime/molt-runtime/src/object/mod.rs
git commit -m "feat(titan-p0): add MoltHotHeader (16B) and MoltColdHeader with safe pool"
```

### Sub-task 3c: Migrate allocation path

- [ ] **Step 8: Update allocation functions to use MoltHotHeader**

Find the allocation function (search `molt_alloc` or `alloc_object` in `mod.rs`). Change it to:
1. Compute `size_class = size_class_for(payload_size)`
2. Allocate `size_of::<MoltHotHeader>() + payload_size` instead of `size_of::<MoltHeader>() + payload_size`
3. Initialize hot header: `type_id`, `ref_count`, `flags` (compressed from u64 to u32), `size_class`
4. If object is generator/async: call `alloc_cold_header(addr, poll_fn, state)`

- [ ] **Step 9: Run tests after allocation path change**

Run: `cargo test -p molt-runtime --lib 2>&1 | tail -20`
Expected: Compilation may fail at sites that access old MoltHeader fields. Fix each error.

### Sub-task 3d: Migrate refcount path

- [ ] **Step 10: Update inc_ref/dec_ref for new header layout**

At lines 904-1083 of `mod.rs`, the refcount functions read `header.flags & HEADER_FLAG_IMMORTAL`. Update these to use the u32 flags field of MoltHotHeader. The flag values stay the same (bits 0-16), just the field type changes from u64 to u32.

- [ ] **Step 11: Run tests**

Run: `cargo test -p molt-runtime --lib 2>&1 | tail -20`

### Sub-task 3e: Migrate remaining header accesses

- [ ] **Step 12: Find and update all remaining MoltHeader references**

Run: `grep -rn "MoltHeader\|header\.poll_fn\|header\.state\|header\.size" runtime/molt-runtime/src/ | grep -v "//"`

For each reference:
- `header.poll_fn` → `get_cold_header(addr).map(|c| c.poll_fn).unwrap_or(0)`
- `header.state` → `get_cold_header(addr).map(|c| c.state).unwrap_or(0)`
- `header.size` → `SIZE_CLASS_TABLE[header.size_class as usize]` (or `get_cold_header(addr).map(|c| c.extended_size)` for oversized)
- `header.flags` → same field name, just u32 now

- [ ] **Step 13: Run full test suite**

Run: `cargo test --workspace 2>&1 | tail -30`
Expected: All tests pass.

### Sub-task 3f: Validate header split

- [ ] **Step 14: Run parity gate**

Run: `python3 tools/parity_gate.py tests/differential/basic/`
Expected: All pass.

- [ ] **Step 15: Run benchmark suite to verify no regression and measure improvement**

Run: `python3 tools/bench.py --benchmarks bench_gc_pressure,bench_attr_access,bench_dict_ops --samples 10`
Expected: No regression; improvement on allocation-heavy benchmarks.

- [ ] **Step 16: Commit header migration**

```bash
git add -A runtime/molt-runtime/
git commit -m "feat(titan-p0): complete hot/cold header migration (40B → 16B for common objects)"
```

---

## Task 4: Inline Cache for Attribute Access

**Files:**
- Create: `runtime/molt-runtime/src/object/inline_cache.rs`
- Modify: `runtime/molt-runtime/src/object/accessors.rs`
- Modify: `runtime/molt-runtime/src/object/mod.rs` (register module)

### Sub-task 4a: IC data structure

- [ ] **Step 1: Create inline_cache.rs**

Create `runtime/molt-runtime/src/object/inline_cache.rs` with:
- `InlineCache` struct: `cached_type_id: AtomicU32`, `cached_offset: AtomicU32`, `cached_version: AtomicU64`
- `probe(&self, type_id, current_version) -> Option<u32>` — returns cached offset on hit
- `update(&self, type_id, offset, version)` — populate cache after miss
- `InlineCacheTable` — pre-allocated `Vec<InlineCache>` with capacity 4096
- Global singleton via `OnceLock` (not `lazy_static`)

Use `global_type_version()` (mod.rs:19-22) for staleness detection.

- [ ] **Step 2: Register module**

Add `pub mod inline_cache;` in mod.rs module declarations.

- [ ] **Step 3: Verify compilation**

Run: `cargo test -p molt-runtime --lib 2>&1 | tail -10`
Expected: Compiles.

- [ ] **Step 4: Commit IC data structure**

```bash
git add runtime/molt-runtime/src/object/inline_cache.rs runtime/molt-runtime/src/object/mod.rs
git commit -m "feat(titan-p0): add inline cache data structure with type version staleness"
```

### Sub-task 4b: Wire IC into attribute access

- [ ] **Step 5: Add IC fast path to accessors.rs**

Read `runtime/molt-runtime/src/object/accessors.rs` fully to understand the current attribute load flow. Add a new `molt_getattr_ic` function that:
1. Extracts type_id from object header
2. Calls `GLOBAL_IC_TABLE.get(ic_index).probe(type_id, global_type_version())`
3. On hit: direct slot load (1 memory access)
4. On miss: call through to existing getattr, then `update` the cache

- [ ] **Step 6: Run tests**

Run: `cargo test --workspace 2>&1 | tail -20`
Expected: All pass.

- [ ] **Step 7: Commit IC wiring in accessors**

```bash
git add runtime/molt-runtime/src/object/accessors.rs
git commit -m "feat(titan-p0): wire inline cache into attribute access hot path"
```

### Sub-task 4c: Wire IC index into frontend

- [ ] **Step 8: Add IC index to GETATTR operations in frontend**

In `src/molt/frontend/__init__.py`, find the GETATTR emission code (around lines 5278-9342). Add a monotonically increasing `ic_index` to each GETATTR operation's metadata dict:

```python
# At class level or module level:
_ic_counter = 0

# In GETATTR emission:
op.metadata["ic_index"] = _ic_counter
_ic_counter += 1
```

- [ ] **Step 9: Commit frontend IC index**

```bash
git add src/molt/frontend/__init__.py
git commit -m "feat(titan-p0): assign IC site indices to GETATTR operations in frontend"
```

### Sub-task 4d: Wire IC into Cranelift backend

- [ ] **Step 10: Pass IC index through Cranelift codegen**

In `runtime/molt-backend/src/native_backend/function_compiler.rs`, find where GETATTR ops are lowered to Cranelift calls. Pass the `ic_index` metadata value as an additional argument to `molt_getattr_ic`.

- [ ] **Step 11: Run benchmarks to measure impact**

Run: `python3 tools/bench.py --benchmarks bench_attr_access,bench_class_hierarchy --samples 10`
Expected: >= 1.5x improvement on bench_attr_access.

- [ ] **Step 12: Run parity gate**

Run: `python3 tools/parity_gate.py tests/differential/basic/`
Expected: All pass.

- [ ] **Step 13: Commit backend wiring**

```bash
git add runtime/molt-backend/src/native_backend/function_compiler.rs
git commit -m "feat(titan-p0): wire IC index through Cranelift backend for GETATTR"
```

---

## Task 5: String Interning

**Files:**
- Create: `runtime/molt-runtime/src/object/string_intern.rs`
- Modify: `runtime/molt-runtime/src/object/mod.rs` (register module)
- Modify: `runtime/molt-runtime/src/object/ops_string.rs` (auto-intern identifier-like strings)

String interning is a prerequisite for both dictionary optimization (Task 7) and string representation (Task 6).

- [ ] **Step 1: Create string_intern.rs**

Implement:
- `intern(s: &str) -> &'static str` — deduplicates by leaking (interned strings live forever)
- `get_interned(s: &str) -> Option<&'static str>` — check if already interned
- `is_identifier_like(s: &str) -> bool` — matches `[a-zA-Z_][a-zA-Z0-9_]*`
- `intern_pool_size() -> usize` — diagnostics
- Pool storage: `OnceLock<Mutex<HashSet<&'static str>>>`

**Important:** The `eq_interned` helper should only be used when BOTH arguments are known to come from the intern pool. Document this clearly. For mixed comparisons, fall through to byte equality.

- [ ] **Step 2: Register module and wire auto-interning**

Add `pub mod string_intern;` to mod.rs.

In the string creation path (find where Rust `&str` values become MoltObject strings), add:
```rust
if string_intern::is_identifier_like(content) {
    let interned = string_intern::intern(content);
    // Create string object with FLAG_INTERNED set in header
    return create_interned_string(interned);
}
```

- [ ] **Step 3: Run tests and parity gate**

Run: `cargo test --workspace 2>&1 | tail -20`
Run: `python3 tools/parity_gate.py tests/differential/basic/`
Expected: All pass.

- [ ] **Step 4: Commit string interning**

```bash
git add runtime/molt-runtime/src/object/string_intern.rs runtime/molt-runtime/src/object/mod.rs
git commit -m "feat(titan-p0): add string interning with auto-intern for identifiers"
```

---

## Task 6: String Representation Overhaul

**Files:**
- Create: `runtime/molt-runtime/src/object/string_repr.rs`
- Modify: `runtime/molt-runtime/src/object/ops_string.rs:33-4074`
- Modify: `runtime/molt-obj-model/src/lib.rs` (if inline/SSO strings need NaN-boxing changes)

This task implements the tagged union string representation from spec section 5.4.

### Sub-task 6a: Define string representation types

- [ ] **Step 1: Create string_repr.rs with tagged union**

```rust
//! Multi-representation string type.
//!
//! MoltStringRepr selects the optimal storage for each string:
//! - Inline: <= 23 bytes, stored directly in the allocation (no separate heap buffer)
//! - OneByte: ASCII-only content, 1 byte per character (50% memory savings vs UTF-16)
//! - TwoByte: BMP content, 2 bytes per character
//! - General: standard UTF-8 Rust String (fallback for supplementary characters)
//! - Interned: deduplicated, pointer equality for comparison (see string_intern.rs)

/// Discriminant tag stored in the object's type metadata.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StringReprKind {
    Inline = 0,    // <= 23 bytes, stored in object body directly
    OneByte = 1,   // ASCII-only, heap-allocated u8 array
    TwoByte = 2,   // BMP, heap-allocated u16 array
    General = 3,   // Standard UTF-8 String
    Interned = 4,  // Pointer into intern pool (see string_intern.rs)
}

/// Inline string storage — 24 bytes (1 byte length + 23 bytes data).
/// Avoids heap allocation for short strings.
/// Covers ~80% of strings in typical Python code (variable names, small text, dict keys).
#[repr(C)]
pub struct InlineString {
    pub len: u8,
    pub data: [u8; 23],
}

impl InlineString {
    pub fn try_new(s: &str) -> Option<Self> {
        if s.len() > 23 { return None; }
        let mut data = [0u8; 23];
        data[..s.len()].copy_from_slice(s.as_bytes());
        Some(Self { len: s.len() as u8, data })
    }

    pub fn as_str(&self) -> &str {
        unsafe { std::str::from_utf8_unchecked(&self.data[..self.len as usize]) }
    }
}

/// Determine the best representation for a given string.
pub fn classify_string(s: &str) -> StringReprKind {
    if s.len() <= 23 {
        return StringReprKind::Inline;
    }
    if s.is_ascii() {
        return StringReprKind::OneByte;
    }
    // Check if all characters are BMP (no supplementary characters)
    if s.chars().all(|c| (c as u32) <= 0xFFFF) {
        return StringReprKind::TwoByte;
    }
    StringReprKind::General
}
```

- [ ] **Step 2: Register module**

Add `pub mod string_repr;` to mod.rs.

- [ ] **Step 3: Verify compilation**

Run: `cargo test -p molt-runtime --lib 2>&1 | tail -10`
Expected: Compiles.

- [ ] **Step 4: Commit string repr types**

```bash
git add runtime/molt-runtime/src/object/string_repr.rs runtime/molt-runtime/src/object/mod.rs
git commit -m "feat(titan-p0): add multi-representation string types (inline/SSO, one-byte, two-byte)"
```

### Sub-task 6b: Wire into string creation

- [ ] **Step 5: Update string creation to use classified representation**

Find the string creation function in the runtime (where `&str` becomes a MoltObject string). Update it to:
1. Call `classify_string(s)` to determine representation
2. For `Inline`: store data directly in the object body (no heap allocation)
3. For `OneByte`: allocate a `Vec<u8>` (1 byte per char)
4. For `TwoByte`/`General`: use existing representation
5. For `Interned`: handled by Task 5's auto-interning path

- [ ] **Step 6: Update string operations to dispatch on representation**

In `runtime/molt-runtime/src/object/ops_string.rs`, the key functions that need dispatch:
- `molt_string_find()` (line 33): check repr, use direct byte search for OneByte
- `molt_string_split()` (line 2159): check repr, use byte-level split for ASCII
- `molt_string_join()` (line 863): **fix the heap corruption bug** (see bench_str_join known issue), then optimize for inline strings
- `molt_string_count()` (line 733): byte-level counting for OneByte

For each function, add a dispatch at the top:
```rust
let repr_kind = get_string_repr_kind(s);
match repr_kind {
    StringReprKind::Inline | StringReprKind::OneByte => {
        // Fast path: operate directly on byte array
        return fast_path_impl(s);
    }
    _ => {
        // Existing implementation (unchanged)
    }
}
```

- [ ] **Step 7: Run tests and parity gate**

Run: `cargo test --workspace 2>&1 | tail -20`
Run: `python3 tools/parity_gate.py tests/differential/basic/`
Expected: All pass.

- [ ] **Step 8: Benchmark string operations**

Run: `python3 tools/bench.py --benchmarks bench_str_split,bench_str_find,bench_str_join,bench_str_count --samples 10`
Expected: >= 1.5x improvement on string benchmarks. bench_str_join should no longer crash.

- [ ] **Step 9: Commit string representation wiring**

```bash
git add runtime/molt-runtime/src/object/ops_string.rs runtime/molt-runtime/src/object/string_repr.rs
git commit -m "feat(titan-p0): wire multi-repr strings into string operations + fix str_join corruption"
```

---

## Task 7: Compact Dictionary Layout

**Files:**
- Create: `runtime/molt-runtime/src/object/dict_compact.rs`
- Modify: `runtime/molt-runtime/src/object/ops_dict.rs:1-906`

- [ ] **Step 1: Create dict_compact.rs**

Implement `CompactDict` with:
- **Small dict optimization (≤8 entries):** Linear probe with no hash table. Keys, values, and hashes stored in parallel arrays. Fits in 1-2 cache lines.
- **Standard dict (>8 entries):** Separate indices array (u8/u16/u32 sized by capacity) + keys/values/hashes arrays.
- **String-key fast path:** When `all_keys_interned` flag is true, key comparison is pointer equality (1 instruction).
- **Version tag:** Monotonic counter incremented on mutation.

Key methods:
- `get(&self, key: u64, key_hash: u64) -> Option<u64>`
- `set(&mut self, key: u64, value: u64, key_hash: u64)`
- `delete(&mut self, key: u64, key_hash: u64) -> Option<u64>`
- `len(&self) -> usize`
- `keys(&self) -> &[u64]`
- `values(&self) -> &[u64]`

**Critical: `insert_into_index` must be fully implemented.** Use open addressing with linear probing. The index table maps `hash % capacity → slot_index`. Empty slots marked with 0xFF (u8), 0xFFFF (u16), or 0xFFFFFFFF (u32) sentinels.

```rust
fn insert_into_index(&mut self, slot: usize, key_hash: u64) {
    let indices = self.indices.as_mut().unwrap();
    let capacity = self.index_capacity();
    let mut idx = (key_hash as usize) % capacity;
    for _ in 0..capacity {
        if self.read_index(idx) == self.empty_sentinel() {
            self.write_index(idx, slot as u32);
            return;
        }
        idx = (idx + 1) % capacity;
    }
    // Should never reach here if load factor < 0.75
    self.resize_and_reindex();
    self.insert_into_index(slot, key_hash);
}
```

- [ ] **Step 2: Register module and wire into ops_dict.rs**

Add `pub mod dict_compact;` to mod.rs.

In `ops_dict.rs`, replace the internal dict backing store with `CompactDict`. The extern "C" API functions (`molt_dict_get`, `molt_dict_set`, `molt_dict_pop`, etc.) stay the same — only the internal implementation changes.

- [ ] **Step 3: Run tests and parity gate**

Run: `cargo test --workspace 2>&1 | tail -20`
Run: `python3 tools/parity_gate.py tests/differential/basic/`
Expected: All pass.

- [ ] **Step 4: Benchmark dict operations**

Run: `python3 tools/bench.py --benchmarks bench_dict_ops,bench_counter_words,bench_csv_parse --samples 10`
Expected: >= 1.5x improvement on dict-heavy benchmarks.

- [ ] **Step 5: Commit compact dict**

```bash
git add runtime/molt-runtime/src/object/dict_compact.rs runtime/molt-runtime/src/object/ops_dict.rs runtime/molt-runtime/src/object/mod.rs
git commit -m "feat(titan-p0): compact dict with small-dict linear probe and interned key fast path"
```

---

## Task 8: Fast Path Completeness

**Files:**
- Modify: `runtime/molt-runtime/src/object/ops.rs` (list operations at ~line 24000+)
- Modify: Various backend/runtime files as needed

Systematically verify and add missing fast paths for each CPython-specialized bytecode.

### Sub-task 8a: List index fast path

- [ ] **Step 1: Add `BINARY_SUBSCR_LIST_INT` fast path**

In `runtime/molt-runtime/src/object/ops.rs`, find the list getitem function (around line 24277 where `IndexError` is raised). Add a fast path before the general implementation:

```rust
/// Fast path: if index is NaN-boxed int, extract and bounds-check directly.
/// Avoids the full getitem protocol (type checks, negative indexing, slice support).
#[inline]
pub extern "C" fn molt_list_getitem_fast(list_bits: u64, index_bits: u64) -> u64 {
    let idx_obj = MoltObject(index_bits);
    if idx_obj.is_int() {
        let idx = idx_obj.as_int_unchecked();
        // Handle negative indexing
        let len = get_list_len(list_bits);
        let actual_idx = if idx < 0 { idx + len as i64 } else { idx };
        if actual_idx >= 0 && (actual_idx as usize) < len {
            return unsafe { get_list_data(list_bits, actual_idx as usize) };
        }
    }
    // Fall through to full getitem (handles slices, non-int indices, etc.)
    molt_list_getitem_full(list_bits, index_bits)
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --workspace 2>&1 | tail -20`

- [ ] **Step 3: Commit**

```bash
git add runtime/molt-runtime/src/object/ops.rs
git commit -m "feat(titan-p0): add list getitem fast path for int index"
```

### Sub-task 8b: Integer comparison fast path

- [ ] **Step 4: Verify `COMPARE_OP_INT` fast path exists**

Check the comparison codegen in the Cranelift backend. When both operands have `fast_int` set, the comparison should compile to a direct `i64` compare instruction (not a runtime function call). If it doesn't, add this fast path.

- [ ] **Step 5: Verify `COMPARE_OP_STR` uses interned pointer equality**

For interned strings, `a == b` should compile to `ptr_a == ptr_b` (1 instruction). Verify this path exists in string comparison code. If not, add it by checking FLAG_INTERNED on both operands before byte comparison.

### Sub-task 8c: FOR_ITER fast paths

- [ ] **Step 6: Verify FOR_ITER_LIST uses pointer increment**

Check that iterating a list uses a pointer/index increment (not a full `__next__` call). This should already exist in the iterator implementation. If not, add it.

- [ ] **Step 7: Verify FOR_ITER_RANGE is lowered to counter loop**

Check that `for i in range(n)` compiles to a simple counter loop (not a range object + iterator). This should already be handled in the frontend. Verify.

- [ ] **Step 8: Run full benchmark suite**

Run: `python3 tools/bench.py --benchmarks all --samples 5`
Expected: All benchmarks faster than CPython.

- [ ] **Step 9: Run parity gate**

Run: `python3 tools/parity_gate.py tests/differential/basic/`
Expected: All pass.

- [ ] **Step 10: Commit fast path additions**

```bash
git add -A runtime/
git commit -m "feat(titan-p0): complete fast path coverage for CPython-specialized operations"
```

---

## Task 9: Final Validation and Phase 0 Sign-Off

**Files:**
- All benchmark and testing infrastructure

- [ ] **Step 1: Run complete benchmark audit**

Run: `python3 tools/bench_audit.py 2>&1 | tee benchmarks/results/phase0_final.txt`
Expected: ALL 61+ benchmarks classified as Green (Molt faster than CPython).

- [ ] **Step 2: Run complete parity gate**

Run: `python3 tools/parity_gate.py tests/differential/basic/`
Run: `python3 tools/parity_gate.py tests/differential/stdlib/`
Expected: Zero Tier 1 violations.

- [ ] **Step 3: Compare with baseline**

```bash
python3 tools/bench_diff.py benchmarks/results/audit_baseline.json benchmarks/results/phase0_final.json
```

**Note:** Ensure both `bench_audit.py` runs output JSON (not just .txt via tee). The JSON artifact is the machine-readable comparison input.

Expected: All benchmarks show improvement or no regression.

- [ ] **Step 4: Run ASAN (requires nightly Rust)**

```bash
# Optional but recommended. Requires: rustup run nightly ...
RUSTFLAGS="-Z sanitizer=address" cargo +nightly test --workspace 2>&1 | tail -30
```
Expected: No memory safety violations. If nightly unavailable, skip and note in results.

- [ ] **Step 5: Commit Phase 0 results**

```bash
git add benchmarks/results/
git commit -m "docs(titan-p0): Phase 0 complete — all benchmarks faster than CPython"
```

---

## Task Dependency Summary

```
Task 1 (Audit) ──────────────┐
Task 2 (Parity Gate) ────────┤
                              ▼
Task 3 (Header Split) ───────┐
                              ├──→ Task 4 (Inline Cache) ──→ ┐
Task 5 (String Interning) ───┤                               │
                              ├──→ Task 6 (String Repr) ──→  │
                              ├──→ Task 7 (Compact Dict) ──→ ├──→ Task 8 (Fast Paths)
                              │                               │         │
                              └───────────────────────────────┘         ▼
                                                               Task 9 (Validation)
```

- Tasks 1-2: MUST be first (safety net)
- Task 3: Most invasive, do early
- Tasks 4, 5, 6, 7: Can be parallelized after Task 3 and Task 5 (5 must precede 7)
- Task 8: After all runtime changes, catch remaining gaps
- Task 9: Final gate
