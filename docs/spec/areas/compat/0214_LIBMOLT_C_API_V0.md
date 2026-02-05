# libmolt C-API v0 (Extension Compatibility)
**Spec ID:** 0214  
**Status:** Draft  
**Owner:** runtime + tooling  
**Goal:** Define the minimal, stable `libmolt` C-API subset that enables
performance-first C-extension compatibility without embedding CPython.

---

## 1. Principles
- Native Molt execution is the default and fastest path.
- `libmolt` is the primary C-extension compatibility path.
- CPython bridge modes are explicit, opt-in escape hatches only.
- No CPython ABI compatibility; extensions must be recompiled.
- Capability gating and determinism rules apply to all extensions.

---

## 2. Non-Goals
- Implementing the full CPython ABI or `libpython` compatibility.
- Allowing implicit fallback to CPython at runtime.
- Supporting extensions that require access to CPython internal structs.

---

## 3. ABI and Stability Contract
- `libmolt` exposes an **opaque handle** model. Extensions never dereference
  Molt object layouts directly.
- All handles are `u64`-compatible values (opaque to the extension).
- A versioned C header defines `MOLT_C_API_VERSION` and symbol availability.
- Symbol availability is tracked in `docs/spec/areas/compat/0212_C_API_SYMBOL_MATRIX.md`.

---

## 4. Core API Surface (v0 target)
### 4.1 Runtime + GIL
- `molt_init`, `molt_shutdown`
- `molt_gil_acquire`, `molt_gil_release`
- `molt_gil_is_held`

### 4.2 Error Handling
- `molt_err_set`, `molt_err_clear`, `molt_err_fetch`
- `molt_err_matches`, `molt_err_format`

### 4.3 Object Protocol
- `molt_object_getattr`, `molt_object_setattr`, `molt_object_hasattr`
- `molt_object_call`
- `molt_object_repr`, `molt_object_str`, `molt_object_truthy`

### 4.4 Numerics
- `molt_number_add`, `molt_number_sub`, `molt_number_mul`
- `molt_number_truediv`, `molt_number_floordiv`
- `molt_number_long`, `molt_number_float`

### 4.5 Sequences + Mappings
- `molt_sequence_length`, `molt_sequence_getitem`, `molt_sequence_setitem`
- `molt_mapping_getitem`, `molt_mapping_setitem`

### 4.6 Buffer + Bytes
- `molt_buffer_acquire`, `molt_buffer_release`
- `molt_bytes_from`, `molt_bytes_as_ptr`
- `molt_bytearray_from`, `molt_bytearray_as_ptr`

---

## 5. Capability and Determinism Rules
- Extensions must declare required capabilities in their metadata.
- Molt enforces capabilities at call boundaries.
- Deterministic builds fail fast if an extension requires disallowed effects.

---

## 6. Packaging and Build Flow
### 6.1 Headers and Tooling
- Provide `molt-config --cflags --libs` for build integration.
- Ship headers under `include/molt/` with stable symbol naming.

### 6.2 Wheel Tags (proposed)
- Wheels for `libmolt` are tagged distinctly from CPython wheels.
- Molt resolves `libmolt` wheels when the target ABI matches the runtime.

### 6.3 Extension Metadata (proposed)
Extensions should declare:
- `molt_c_api_version`
- `capabilities`
- `determinism` requirements
- `abi` target triple

---

## 7. Testing and Validation
- Per-symbol conformance tests.
- Differential tests comparing extension outputs to CPython for supported APIs.
- Fuzz tests for buffer and bytes interfaces.
- Benchmarks for hot-path extension calls.

---

## 8. Migration Guidance
- Prefer using the `Py_LIMITED_API` subset when porting.
- Replace `PyObject*` direct access with `libmolt` accessors.
- Keep native kernels in C/Rust; avoid dependency on CPython internals.

---

## 9. Relationship to Bridge Modes
- The CPython bridge remains an explicit, capability-gated escape hatch.
- `libmolt` is the primary compatibility path and the performance default.

