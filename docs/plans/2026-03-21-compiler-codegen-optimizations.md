# Molt Compiler Codegen Optimizations Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce compiled binary size by ~40MB and improve compilation speed by outlining common codegen patterns in the Molt backend, eliminating redundant code emission for class definitions, tuple unpacking, and string formatting.

**Architecture:** The Molt backend (`runtime/molt-backend/src/lib.rs`) compiles Python IR ops into Cranelift IR. Currently, several common patterns generate large inline code sequences per use site. This plan outlines each pattern into a shared runtime helper function, reducing per-site emission from hundreds of Cranelift IR instructions to a single function call. This follows the same pattern as the successful `molt_guarded_call` optimization (commit `19af212b`).

**Tech Stack:** Rust (Cranelift IR builder), Molt runtime (`runtime/molt-runtime/src/`)

**Applies to:** Both native and WASM targets

---

## File Structure

```
runtime/molt-backend/src/
├── lib.rs                    — MODIFY: outline class_def, unpack_sequence, format_string ops
runtime/molt-runtime/src/
├── object/
│   ├── ops.rs                — MODIFY: add molt_guarded_class_def, molt_unpack_sequence
│   └── mod.rs                — reference for MoltHeader layout
├── builtins/
│   └── functions.rs          — MODIFY: add molt_format_string helper
└── lib.rs                    — MODIFY: export new helpers
```

---

### Task 1: Outline class definition codegen

The `"class_def"` op in the backend generates inline code for metaclass resolution, `__init_subclass__`, descriptor setup, `__set_name__`, MRO computation, and slot allocation. This is the single largest per-op codegen pattern remaining — a single class definition chunk in `threading.py` generates 12.56MB.

**Files:**
- Modify: `runtime/molt-runtime/src/object/ops.rs` — add `molt_guarded_class_def`
- Modify: `runtime/molt-backend/src/lib.rs` — replace inline class_def with call to helper

- [ ] **Step 1: Find the class_def op in the backend**

Search for the class definition compilation:
```bash
grep -n '"class_def"\|"build_class"\|CLASS_DEF\|class.*def.*op' runtime/molt-backend/src/lib.rs | head -10
```

Count the lines of Cranelift IR builder code it generates:
```bash
# Find the start and end of the class_def handling block
```

- [ ] **Step 2: Create `molt_guarded_class_def` runtime helper**

In `runtime/molt-runtime/src/object/ops.rs`, add:
```rust
/// Outlined class definition helper. Handles:
/// - Metaclass resolution (type or custom)
/// - Namespace dict creation
/// - Base class MRO computation
/// - __init_subclass__ dispatch
/// - __set_name__ for descriptors
/// - Slot allocation
///
/// Arguments:
///   name_bits: class name string
///   bases_bits: tuple of base classes
///   namespace_bits: class body namespace dict
///   metaclass_bits: explicit metaclass or 0 for default
///   kwargs_bits: keyword arguments to metaclass
///
/// Returns: the new class object bits
#[unsafe(no_mangle)]
pub extern "C" fn molt_guarded_class_def(
    name_bits: u64,
    bases_bits: u64,
    namespace_bits: u64,
    metaclass_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    // Extract the inline class construction logic from the backend
    // and implement it here as runtime code
}
```

- [ ] **Step 3: Replace inline class_def in backend with call to helper**

In `runtime/molt-backend/src/lib.rs`, find the class_def op and replace the multi-block inline sequence with:
```rust
// Spill class construction args to stack
// Call molt_guarded_class_def(name, bases, namespace, metaclass, kwargs)
// Use result as the new class object
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p molt-backend -p molt-runtime`
Expected: compiles with no errors

- [ ] **Step 5: Build mawn app and measure**

```bash
cd /Users/adpena/Projects/mawn
rm -rf ~/Library/Caches/molt/ /Applications/Mawn.app target/.molt_state/build_locks/
bash scripts/build-mac-launcher.sh 2>&1 | tail -3
ls -lh /Applications/Mawn.app/Contents/MacOS/mawn-ui
```
Expected: binary size reduction of 10-15MB

- [ ] **Step 6: Verify app launches**

```bash
open /Applications/Mawn.app
sleep 15
ps aux | grep mawn-ui | grep -v grep
```
Expected: mawn-ui process running

- [ ] **Step 7: Commit**

```bash
cd /Users/adpena/Projects/molt
git add -A
git commit -m "perf(backend): outline class definition codegen into molt_guarded_class_def"
```

---

### Task 2: Outline tuple/sequence unpacking codegen

The `"unpack_sequence"` or equivalent op generates per-element extraction + type checking code. A 6-element unpack produces ~130KB of native code. An outlined helper collapses this to a single call.

**Files:**
- Modify: `runtime/molt-runtime/src/object/ops.rs` — add `molt_unpack_sequence`
- Modify: `runtime/molt-backend/src/lib.rs` — replace inline unpack with call

- [ ] **Step 1: Find the unpack op in the backend**

```bash
grep -n '"unpack"\|UNPACK_SEQUENCE\|unpack.*sequence\|"starred_unpack"' runtime/molt-backend/src/lib.rs | head -10
```

- [ ] **Step 2: Create `molt_unpack_sequence` runtime helper**

```rust
/// Outlined sequence unpacking. Validates length, extracts elements.
/// Returns a pointer to a stack-allocated array of u64 element bits.
/// Raises ValueError if sequence length doesn't match expected count.
#[unsafe(no_mangle)]
pub extern "C" fn molt_unpack_sequence(
    seq_bits: u64,
    expected_count: u64,
    output_ptr: *mut u64,
) -> u64 {
    // 1. Get iterator from seq
    // 2. Validate length == expected_count
    // 3. Write elements to output_ptr[0..expected_count]
    // 4. Return 0 on success, raise ValueError on mismatch
}
```

- [ ] **Step 3: Replace inline unpack in backend**

- [ ] **Step 4: Verify and measure**

- [ ] **Step 5: Commit**

---

### Task 3: Outline string formatting codegen

f-strings and `.format()` generate inline concatenation code. An outlined formatter reduces this to a single call per format expression.

**Files:**
- Modify: `runtime/molt-runtime/src/builtins/functions.rs` — add `molt_format_values`
- Modify: `runtime/molt-backend/src/lib.rs` — replace inline format with call

- [ ] **Step 1: Find the format/fstring op in the backend**

```bash
grep -n '"format_value"\|FORMAT_VALUE\|"build_string"\|BUILD_STRING\|fstring' runtime/molt-backend/src/lib.rs | head -10
```

- [ ] **Step 2: Create `molt_format_values` runtime helper**

```rust
/// Outlined string formatting. Takes an array of values and format specs,
/// returns the formatted string.
#[unsafe(no_mangle)]
pub extern "C" fn molt_format_values(
    parts_ptr: *const u64,  // alternating: literal_bits, value_bits, spec_bits
    nparts: u64,
) -> u64 {
    // Build the formatted string from parts
}
```

- [ ] **Step 3: Replace inline format in backend**

- [ ] **Step 4: Verify and measure**

- [ ] **Step 5: Commit**

---

### Task 4: Outline isinstance/type-checking codegen

`isinstance(x, (int, float, str))` generates a separate branch per type. An outlined helper checks against a type tuple in a single call.

**Files:**
- Modify: `runtime/molt-runtime/src/object/ops.rs` — add `molt_isinstance_tuple`
- Modify: `runtime/molt-backend/src/lib.rs` — optimize isinstance with tuple arg

- [ ] **Step 1: Find isinstance handling in backend**

```bash
grep -n '"isinstance"\|ISINSTANCE\|is_instance' runtime/molt-backend/src/lib.rs | head -10
```

- [ ] **Step 2: Create outlined isinstance helper for tuple args**

- [ ] **Step 3: Replace inline isinstance-tuple in backend**

- [ ] **Step 4: Verify and measure**

- [ ] **Step 5: Commit**

---

### Task 5: Dead stdlib module elimination (precise approach)

Gate unused stdlib Python source files so they're not compiled into the binary. This is different from the Cargo dependency gating (which removes Rust deps) — this removes the compiled PYTHON code for unused stdlib modules.

**Files:**
- Modify: `src/molt/cli.py` — add import-graph-based module exclusion
- Modify: `src/molt/frontend/__init__.py` — skip compilation of excluded modules

- [ ] **Step 1: Build the import graph**

After frontend visiting, compute the transitive closure of imports starting from the entry module. Any stdlib module NOT in this closure can be excluded.

- [ ] **Step 2: Skip compilation of unreachable modules**

In the module compilation loop, check if each module is in the reachable set. If not, skip it.

- [ ] **Step 3: Verify and measure**

Expected: 20-30MB reduction from eliminating ~270 unused stdlib modules.

- [ ] **Step 4: Commit**

---

### Task 6: Final verification and benchmarking

- [ ] **Step 1: Clean build with all optimizations**

```bash
cd /Users/adpena/Projects/molt && rm -rf target/
cd /Users/adpena/Projects/mawn
rm -rf ~/Library/Caches/molt/ /Applications/Mawn.app target/.molt_state/build_locks/
bash scripts/build-mac-launcher.sh 2>&1 | tail -3
```

- [ ] **Step 2: Measure final binary size**

```bash
ls -lh /Applications/Mawn.app/Contents/MacOS/mawn-ui
du -sh /Applications/Mawn.app
du -sh /Applications/CodexBar.app
```

- [ ] **Step 3: Verify app works**

```bash
open /Applications/Mawn.app
sleep 15
ps aux | grep mawn-ui | grep -v grep
curl -s http://127.0.0.1:5773/health | python3 -m json.tool
```

- [ ] **Step 4: Run Molt tests**

```bash
cd /Users/adpena/Projects/molt
cargo test -p molt-runtime --lib
cargo test -p molt-backend --lib
```

- [ ] **Step 5: Commit final state**
