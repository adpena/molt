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
- Header-tiering is normative per
  `docs/spec/areas/compat/contracts/libmolt_extension_abi_contract.md`:
  - stable ABI: `include/molt/molt.h`
  - CPython source-compat facade: `include/Python.h`,
    `include/molt/Python.h`, and the small legacy forwarding headers
  - ecosystem overlays: bounded compatibility headers such as `include/numpy/*`
    and the top-level NumPy bridge headers
- `MOLT_C_API_VERSION` versions the stable ABI tier only; source-compat overlay
  growth does not imply CPython ABI compatibility.
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
- `molt_object_get_iter`, `molt_iterator_next`
- `molt_object_repr`, `molt_object_str`, `molt_object_truthy`
- `molt_object_equal`, `molt_object_not_equal`, `molt_object_contains`

### 4.4a Capsules
- `molt_capsule_new`
- `molt_capsule_get_name_ptr`, `molt_capsule_get_pointer`
- `molt_capsule_is_valid`
- `molt_capsule_get_context`, `molt_capsule_set_context`
- `molt_capsule_import`

### 4.5 Numerics
- `molt_number_add`, `molt_number_sub`, `molt_number_mul`
- `molt_number_truediv`, `molt_number_floordiv`
- `molt_number_long`, `molt_number_float`

### 4.6 Sequences + Mappings
- `molt_sequence_length`, `molt_sequence_getitem`, `molt_sequence_setitem`
- `molt_sequence_to_list`, `molt_sequence_to_tuple`
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
- `PyCapsule_New`, `PyCapsule_GetName`, `PyCapsule_GetPointer`,
  `PyCapsule_IsValid`, `PyCapsule_GetContext`, `PyCapsule_SetContext`,
  `PyCapsule_Import`
- `PyObject_GetIter`, `PyIter_Next`, `PySequence_*`, list-materializing
  `PyMapping_Keys`/`PyMapping_Values`/`PyMapping_Items`,
  always-succeeds `PyMapping_HasKey*`, and high-use `PyDict_*`
  collection/delete wrappers on top of `libmolt`
- reference/type/memory helper macros and shims used by extension sources
  (`Py_TYPE`, `Py_SETREF`, `Py_CLEAR`, `PyTuple_GET_*`, `PyList_GET_*`,
  `PyMem_*`, `PyObject_GetBuffer`/`PyBuffer_Release`)
- convenience call/build helpers (`PyObject_CallFunctionObjArgs`,
  `PyObject_CallFunction`, `PyObject_CallMethod`, `Py_BuildValue`)
- module/threading shims (`PyThreadState_Get`, `PyGILState_Ensure`,
  `PyGILState_Release`, `PyImport_ImportModule`, `PySys_GetObject`,
  `PyCapsule_Import`)
- selected Unicode helpers (`PyUnicode_InternFromString`)
- `PyArg_ParseTuple` / `PyArg_ParseTupleAndKeywords` format coverage for
  `O,O!,b,B,h,H,i,I,l,k,L,K,n,c,d,f,p,s,s#,z,z#,y#` with `|` optional + `$`
  keyword-only markers and kwlist-driven keyword lookup in the keywords path
- `PyArg_UnpackTuple` tuple-arity/object unpack helper
- `PyArg_VaParseTupleAndKeywords` symbol lane (currently fail-fast while full
  `va_list` parity is implemented)
- `PyType_Spec` slot lowering includes selected call/numeric/sequence/getset
  lanes and type-method flag handling for `METH_CLASS` + `METH_STATIC`
- NumPy source-compat include lane (`#include <numpy/arrayobject.h>` /
  `#include <numpy/ndarrayobject.h>`) with type/shape/flag macros, typenum
  predicates, dtype/type-object exports, `PyDataType_*` and `PyDataMem_*`
  helpers, `import_array*` capsule wiring, the upstream-shipped public contract
  headers (`arrayobject.h`, `dtype_api.h`, `npy_2_compat.h`, `npy_cpu.h`,
  `npy_math.h`, `numpyconfig.h`, `utils.h`), upstream-derived overlay headers
  (`_public_dtype_api_table.h`, `halffloat.h`, `npy_2_complexcompat.h`,
  `npy_3kcompat.h`, `npy_endian.h`, `npy_no_deprecated_api.h`, `npy_os.h`,
  `random/bitgen.h`, `random/distributions.h`), top-level `arrayobject.h`,
  `pymem.h`, `frameobject.h`, generated-config bridge headers
  (`_numpyconfig.h`, `config.h`, `npy_cpu_dispatch_config.h`,
  `numpy/npy_cpu.h`), arrayscalar/object source shapes, utility/visibility
  helpers (`NPY_UNUSED`, `NPY_VISIBILITY_HIDDEN`, `NPY_NO_EXPORT`, `NPY_TLS`),
  `NpyAuxData` lifecycle macros, legacy `PyUFunc_Loop1d` / `PyUFuncObject`
  source shapes, and fail-fast stubs for unsupported heavy ndarray/ufunc APIs.
  Internal generated/source-only bridges such as `templ_common.h` exist to
  keep real NumPy core syntax-only probes moving, but private/generated NumPy
  build artifacts such as `arraytypes.h` and dispatch-generated internal
  headers remain outside the public `libmolt` compatibility contract.
- Datetime source-compat include lane (`#include <datetime.h>`) with
  `PyDateTimeAPI`, `PyDateTime_IMPORT`, and basic date/datetime/timedelta
  checker shims
- Legacy CPython member-definition include lane (`#include <structmember.h>`)
  with `Py_T_*` / `Py_READONLY` constants, deprecated alias macros, and
  fail-fast `PyMember_GetOne` / `PyMember_SetOne` shims

---

## 5. Capability and Determinism Rules
- Extensions must declare required capabilities in their metadata.
- Molt enforces capabilities at call boundaries.
- Deterministic builds fail fast if an extension requires disallowed effects.

---

## 6. Packaging and Build Flow
### 6.1 Headers and Tooling
- Provide `molt-config --cflags --libs` for build integration.
- Stable ABI headers live under `include/molt/`; the current canonical ABI
  header is `include/molt/molt.h`.
- CPython-compat include paths are compatibility facades, not ABI promises:
  `#include <Python.h>` is implemented by `include/Python.h` forwarding to
  `include/molt/Python.h`.
- NumPy compatibility headers under `include/numpy/` and the small top-level
  forwarding/config bridge headers are bounded source-compat overlays.
- `molt extension build` and `molt extension scan` use the declared libmolt
  header contract instead of treating every header under `include/` as equally
  stable.
- libmolt does not ship NumPy's private/generated build graph.

### 6.2 Wheel Tags (proposed)
- Wheels for `libmolt` are tagged distinctly from CPython wheels.
- Molt resolves `libmolt` wheels when the target ABI matches the runtime.

### 6.3 Extension Metadata (proposed)
Extensions should declare:
- `molt_c_api_version`
- `header_contract`
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
