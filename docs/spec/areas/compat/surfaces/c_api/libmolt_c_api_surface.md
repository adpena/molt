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
- Symbol availability is tracked in `docs/spec/areas/compat/surfaces/c_api/c_api_symbol_matrix.md`.
- Current bootstrap implementation:
  - Runtime symbols: `runtime/molt-runtime/src/c_api.rs`
  - Public header: `include/molt/molt.h`
  - CPython-compat shim headers: `include/Python.h`, `include/molt/Python.h`
  - Current version constant: `MOLT_C_API_VERSION = 1`

---

## 4. Core API Surface (v0 target)
### 4.1 Runtime + GIL
- `molt_init`, `molt_shutdown`
- `molt_gil_acquire`, `molt_gil_release`
- `molt_gil_is_held`

### 4.2 Error Handling
- `molt_err_set`, `molt_err_clear`, `molt_err_pending`, `molt_err_peek`
- `molt_err_fetch`, `molt_err_restore`
- `molt_err_matches`, `molt_err_format`

### 4.3 Scalar Constructors/Accessors
- `molt_none`, `molt_bool_from_i32`
- `molt_int_from_i64`, `molt_int_as_i64`
- `molt_float_from_f64`, `molt_float_as_f64`

### 4.4 Object Protocol
- `molt_object_getattr`, `molt_object_setattr`, `molt_object_hasattr`
- `molt_object_getattr_bytes`, `molt_object_setattr_bytes`
- `molt_object_call`
- `molt_object_repr`, `molt_object_str`, `molt_object_truthy`
- `molt_object_equal`, `molt_object_not_equal`, `molt_object_contains`

### 4.5 Numerics
- `molt_number_add`, `molt_number_sub`, `molt_number_mul`
- `molt_number_truediv`, `molt_number_floordiv`
- `molt_number_long`, `molt_number_float`

### 4.6 Sequences + Mappings
- `molt_sequence_length`, `molt_sequence_getitem`, `molt_sequence_setitem`
- `molt_mapping_getitem`, `molt_mapping_setitem`, `molt_mapping_length`, `molt_mapping_keys`
- `molt_tuple_from_array`, `molt_list_from_array`, `molt_dict_from_pairs`

### 4.7 Buffer + Bytes
- `molt_buffer_acquire`, `molt_buffer_release`
- `molt_bytes_from`, `molt_bytes_as_ptr`
- `molt_string_from`, `molt_string_as_ptr`
- `molt_bytearray_from`, `molt_bytearray_as_ptr`

### 4.8 Types + Modules
- `molt_type_ready`
- `molt_module_create`, `molt_module_import`, `molt_module_get_dict`
- `molt_module_capi_register`, `molt_module_capi_get_def`, `molt_module_capi_get_state`
- `molt_module_state_add`, `molt_module_state_find`, `molt_module_state_remove`
- `molt_module_add_object`, `molt_module_add_object_bytes`
- `molt_module_get_object`, `molt_module_get_object_bytes`
- `molt_module_add_type`
- `molt_module_add_int_constant`, `molt_module_add_string_constant`
- `molt_cfunction_create_bytes`, `molt_module_add_cfunction_bytes`

### 4.9 CPython Source-Compat Shim (partial)
- `PyType_Ready`
- `PyType_FromSpec`, `PyType_FromSpecWithBases`, `PyType_FromModuleAndSpec`
- `PyType_GetModule`, `PyType_GetModuleState`, `PyType_GetModuleByDef`
- `PyModule_New(Object)`, `PyModule_Create(2)`, `PyModuleDef_Init`
- `PyModule_AddObject(Ref)`, `PyModule_Add`, `PyModule_AddType`,
  `PyModule_AddIntConstant`, `PyModule_AddStringConstant`
- `PyModule_GetObject`, `PyModule_GetName(Object)`,
  `PyModule_GetFilename(Object)`, `PyModule_GetDef`, `PyModule_GetState`,
  `PyModule_SetDocString`, `PyModule_AddFunctions`,
  `PyModule_FromDefAndSpec(2)`, `PyModule_ExecDef`, `PyState_*`
- `PyErr_*` core helpers (`Occurred`, `SetString`, `SetObject`, `Clear`,
  `Fetch`, `Restore`, `Matches`, `Format`, `NoMemory`, warning stubs)
- `PySequence_*` / `PyMapping_*` wrappers on top of `libmolt`
- reference/type/memory helper macros and shims used by extension sources
  (`Py_TYPE`, `Py_SETREF`, `Py_CLEAR`, `PyTuple_GET_*`, `PyList_GET_*`,
  `PyMem_*`, `PyObject_GetBuffer`/`PyBuffer_Release`)
- convenience call/build helpers (`PyObject_CallFunctionObjArgs`,
  `PyObject_CallFunction`, `PyObject_CallMethod`, `Py_BuildValue`)
- module/threading shims (`PyThreadState_Get`, `PyGILState_Ensure`,
  `PyGILState_Release`, `PyImport_ImportModule`, `PyCapsule_Import`)
- `PyArg_ParseTuple` / `PyArg_ParseTupleAndKeywords` format coverage for
  `O,O!,b,B,h,H,i,I,l,k,L,K,n,c,d,f,p,s,s#,z,z#,y#` with `|` optional + `$`
  keyword-only markers and kwlist-driven keyword lookup in the keywords path
- `PyArg_UnpackTuple` tuple-arity/object unpack helper
- `PyArg_VaParseTupleAndKeywords` symbol lane (currently fail-fast while full
  `va_list` parity is implemented)
- `PyType_Spec` slot lowering includes selected call/numeric/sequence/getset
  lanes and type-method flag handling for `METH_CLASS` + `METH_STATIC`
- NumPy source-compat include lane (`#include <numpy/arrayobject.h>`) with
  type/shape/flag macros, typenum predicates, dtype/type-object exports,
  `PyDataType_*` and `PyDataMem_*` helpers, `import_array*` capsule wiring,
  upstream-shaped internal include wrappers (`npy_common.h`, `dtype_api.h`,
  `__multiarray_api.h`, `__ufunc_api.h`, `npy_2_compat.h`, `npy_math.h`),
  and fail-fast stubs for unsupported heavy ndarray/ufunc APIs
- Datetime source-compat include lane (`#include <datetime.h>`) with
  `PyDateTimeAPI`, `PyDateTime_IMPORT`, and basic date/datetime/timedelta
  checker shims

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
- Current shipped bootstrap header: `include/molt/molt.h`.
- CPython-compat include path is also available via `#include <Python.h>`,
  implemented by `include/Python.h` forwarding to `include/molt/Python.h`.
- Initial NumPy compatibility headers ship under `include/numpy/` and are
  intentionally partial while we close remaining NumPy C-API gaps.
- Initial datetime compatibility header ships as `include/datetime.h` with a
  partial `PyDateTime` C-API bootstrap.

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
