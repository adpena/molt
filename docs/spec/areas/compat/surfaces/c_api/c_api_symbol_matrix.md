# C API Symbol Matrix
**Spec ID:** 0212
**Status:** Draft
**Owner:** runtime
**Goal:** Define the subset of `Python.h` exposed by Molt's runtime for C extensions and the Bridge.

## 1. Strategy
- **Binary Compatibility:** Molt does *not* aim for ABI compatibility with `libpython.so`. Extensions must be recompiled against `libmolt`.
- **Source Compatibility:** We aim for source compatibility for a high-value subset of the Limited API (Py_LIMITED_API).
- **Primary Path:** `libmolt` is the primary C-extension compatibility path; CPython bridge modes are explicit, opt-in escape hatches.
- **V0 Contract:** The target surface area and semantics are defined in
  `docs/spec/areas/compat/surfaces/c_api/libmolt_c_api_surface.md`.
- **Header Contract:** The stable-vs-compat header boundary is defined in
  `docs/spec/areas/compat/contracts/libmolt_extension_abi_contract.md`.
- **Current Status:** A `libmolt` C-API bootstrap surface is implemented with
  `molt_*` wrapper symbols (`runtime/molt-runtime/src/c_api.rs` + `include/molt/molt.h`),
  and `include/molt/Python.h` now carries a broad partial CPython source-compat
  layer plus expanded NumPy source-compat headers under `include/numpy/`
  covering dtype/type-object exports, `PyDataType_*` and `PyDataMem_*`
  helpers, array flag/conversion/copy/setup shims, generated-config bridge
  headers (`_numpyconfig.h`, `config.h`, `npy_cpu_dispatch_config.h`,
  `numpy/npy_cpu.h`), `numpyconfig.h` / `utils.h`, `NPY_UNUSED` / `NPY_TLS` /
  visibility helpers, `NpyAuxData` lifecycle macros, legacy
  `PyUFunc_Loop1d`/`PyUFuncObject` source shapes, and fail-fast `PyUFunc_*`
  constructor/registration stubs. `include/molt/Python.h` also now covers
  `inttypes.h`, `PY_VERSION_HEX`, `PyErr_BadInternalCall`, selected
  weakref/unicode/private-helper shims, and NumPy-core `PYTHONCAPI_COMPAT`
  suppression so vendored `pythoncapi-compat` does not collide with
  libmolt-owned helpers during `_MULTIARRAYMODULE` / `_UMATHMODULE` builds.
- **Latest scan baseline (2026-03-06):** archived C-source sdist scans report
  missing symbols: NumPy `2.4.2` `239` (coverage `0.631`), pandas `3.0.1` `0`
  (coverage `1.000`). Real `clang -fsyntax-only` checks now pass NumPy
  `limited_api1.c` / `limited_api_latest.c`, NumPy
  `numpy/_core/src/multiarray/npy_static_data.c` (with `-Wno-sign-compare` to
  ignore upstream warning noise), NumPy
  `numpy/_core/src/umath/ufunc_type_resolution.c`, and all checked pandas
  `_libs/src` translation units including `parser/tokenizer.c` without any
  forced `inttypes.h` include. The checked NumPy-core frontier is now split
  between private/generated NumPy build artifacts (`arraytypes.h` after the
  config-header bridge in `conversion_utils.c`) and deeper internal source
  closure in `scalarapi.c` (for example `PyArray_DiscoverDTypeAndShape`,
  `PyArray_AssignFromCache`, `PyArray_NewFromDescr*`, `npy_dtype_info`,
  `NPY_NSCALARKINDS`, and `PyArrayScalar_VAL` macro-family gaps).
- **Tooling Boundary:** `molt extension scan` now consults an explicit
  libmolt header-contract list and `molt extension build` records that contract
  in extension manifests, so stable ABI headers and compatibility overlays are
  reported separately instead of being treated as one undifferentiated header
  tree.
- **Hollow Symbols (future):** Some symbols may exist but return generic errors or empty values if their functionality (e.g., GC inspection) is not supported (TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:planned): define hollow-symbol policy + error surface).

## 2. Symbol Matrix
Status legend:
- **Planned (v0)**: part of the `libmolt` C-API v0 surface (`0214`).
- **Partial**: compatibility shim exists in `include/molt/Python.h` but full CPython parity is not complete.
- **Missing**: not yet implemented.
- **Future**: explicitly out of scope for v0.

### 2.1 Object Protocol (PyObject_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyObject_GetAttr` | getattr(o, name) | Partial | `include/molt/Python.h` maps to `molt_object_getattr`. |
| `PyObject_SetAttr` | setattr(o, name, v) | Partial | `include/molt/Python.h` maps to `molt_object_setattr`. |
| `PyObject_HasAttr` | hasattr(o, name) | Partial | `include/molt/Python.h` maps to `molt_object_hasattr`. |
| `PyObject_Call` | o(*args) | Partial | `include/molt/Python.h` maps to `molt_object_call`; kwargs fast-path not yet surfaced. |
| `PyObject_Repr` | repr(o) | Partial | `include/molt/Python.h` maps to `molt_object_repr`. |
| `PyObject_Str` | str(o) | Partial | `include/molt/Python.h` maps to `molt_object_str`. |
| `PyObject_IsTrue` | bool(o) | Partial | `include/molt/Python.h` maps to `molt_object_truthy`. |
| `PyObject_RichCompare`| compare | Partial | Header shim currently routes through `PyObject_RichCompareBool` and returns `Py_True`/`Py_False` (no `NotImplemented` lane yet). |
| `PyObject_RichCompareBool`| compare bool | Partial | `include/molt/Python.h` maps `==`/`!=` to `molt_object_equal`/`molt_object_not_equal` and falls back to dunder calls for ordered comparisons. |
| `PyObject_GetIter` | iter(o) | Partial | `include/molt/Python.h` maps to `molt_object_get_iter`. |
| `PyIter_Next` | next(o) | Partial | `include/molt/Python.h` maps to `molt_iterator_next`. |

### 2.2 Numbers (PyNumber_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyNumber_Add` | a + b | Partial | `include/molt/Python.h` maps to `molt_number_add`. |
| `PyNumber_Subtract` | a - b | Partial | `include/molt/Python.h` maps to `molt_number_sub`. |
| `PyNumber_Multiply` | a * b | Partial | `include/molt/Python.h` maps to `molt_number_mul`. |
| `PyNumber_TrueDivide` | a / b | Partial | `include/molt/Python.h` maps to `molt_number_truediv`. |
| `PyNumber_FloorDivide`| a // b | Partial | `include/molt/Python.h` maps to `molt_number_floordiv`. |
| `PyNumber_Long` | int(o) | Partial | `include/molt/Python.h` maps to `molt_number_long`. |
| `PyNumber_Float` | float(o) | Partial | runtime/native symbol exists in `runtime/molt-runtime/src/c_api.rs`; `include/molt/Python.h` does not currently expose a source-compat wrapper. |

### 2.3 Sequences (PySequence_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PySequence_Check` | is sequence? | Partial | Header shim checks for `__getitem__` capability. |
| `PySequence_Size` | len(o) | Partial | `include/molt/Python.h` maps to `molt_sequence_length`. |
| `PySequence_GetItem` | o[i] | Partial | `include/molt/Python.h` maps to `molt_sequence_getitem`. |
| `PySequence_SetItem` | o[i] = v | Partial | `include/molt/Python.h` maps to `molt_sequence_setitem`. |
| `PySequence_DelItem` | del o[i] | Partial | Header shim routes through `__delitem__` via `PyObject_CallMethod`. |
| `PySequence_List` | list(o) | Partial | `include/molt/Python.h` maps to `molt_sequence_to_list`. |
| `PySequence_Tuple` | tuple(o) | Partial | `include/molt/Python.h` maps to `molt_sequence_to_tuple`. |

### 2.4 Mapping (PyMapping_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyMapping_Check` | is mapping? | Partial | Header shim checks `dict` or `__getitem__` + `keys` capability. |
| `PyMapping_Size` | len(o) | Partial | `include/molt/Python.h` maps to `molt_mapping_length`. |
| `PyMapping_GetItemString`| o["key"] via C string | Partial | `include/molt/Python.h` maps to `molt_mapping_getitem`. |
| `PyMapping_SetItemString`| o[key] = v | Partial | `include/molt/Python.h` maps to `molt_mapping_setitem`. |
| `PyMapping_Keys` | o.keys() | Partial | Header shim calls `keys()` then normalizes the result to a list; the runtime/native symbol now materializes the same list semantics. |
| `PyMapping_Values` | o.values() | Partial | Header shim calls `values()` then normalizes the result to a list; the runtime/native symbol now materializes the same list semantics. |
| `PyMapping_Items` | o.items() | Partial | Header shim calls `items()` then normalizes the result to a list; the runtime/native symbol now materializes the same list semantics. |
| `PyMapping_HasKey` | `key in o` | Partial | Header shim uses containment and clears exceptions to preserve the historical always-succeeds contract. |
| `PyMapping_HasKeyString` | `key in o` via C string | Partial | Header shim interns a temporary UTF-8 key and clears exceptions to preserve the historical always-succeeds contract. |

### 2.4a Dictionaries (PyDict_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyDict_DelItem` | delete dict[key] | Partial | Header shim now routes through `__delitem__`; runtime/native symbol is implemented. |
| `PyDict_DelItemString` | delete dict[key] via C string | Partial | Header shim now creates a temporary Unicode key and delegates to `PyDict_DelItem`; runtime/native symbol is implemented. |
| `PyDict_Keys` | list(dict.keys()) | Partial | Header shim delegates to `PyMapping_Keys`; runtime/native symbol now materializes a list. |
| `PyDict_Values` | list(dict.values()) | Partial | Header shim delegates to `PyMapping_Values`; runtime/native symbol now materializes a list. |
| `PyDict_Items` | list(dict.items()) | Partial | Header shim delegates to `PyMapping_Items`; runtime/native symbol now materializes a list. |

### 2.5 Exceptions (PyErr_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyErr_Occurred` | Check exc | Partial | Shim landed in `include/molt/Python.h`. |
| `PyErr_SetString` | Raise msg | Partial | Shim landed in `include/molt/Python.h`. |
| `PyErr_SetObject` | Raise obj | Partial | Shim landed in `include/molt/Python.h`; object restore semantics are minimal. |
| `PyErr_Clear` | Clear exc | Partial | Shim landed in `include/molt/Python.h`. |
| `PyErr_Fetch` | Get exc | Partial | Shim landed in `include/molt/Python.h`; traceback slot remains `NULL`. |
| `PyErr_Restore` | Set exc | Partial | Shim landed in `include/molt/Python.h`. |
| `PyErr_Matches` | Type matches | Partial | Shim landed in `include/molt/Python.h`. |
| `PyErr_Format` | Format msg | Partial | Shim landed in `include/molt/Python.h`. |
| `PyErr_NoMemory` | Raise `MemoryError` | Partial | Header shim raises `PyExc_MemoryError` and returns `NULL`. |
| `PyErr_WarnEx` | Emit warning | Partial | Header shim is currently a no-op success path (`0`) to preserve extension control flow. |
| `PyErr_WarnFormat` | Formatted warning | Partial | Header shim formats message and delegates to `PyErr_WarnEx`. |

### 2.6 Types & Modules
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyType_Ready` | Init type | Partial | `include/molt/Python.h` maps to `molt_type_ready`. |
| `PyType_FromSpecWithBases` | Heap-type creation from `PyType_Spec` + bases | Partial | Header shim supports `Py_tp_doc`, `Py_tp_methods`, `Py_tp_base`, `Py_tp_bases`, `Py_tp_new`, selected call/dunder slots (`Py_tp_call`, `Py_tp_iter`, `Py_tp_iternext`, `Py_tp_repr`, `Py_tp_str`), selected numeric/sequence slots (`Py_nb_add`, `Py_nb_subtract`, `Py_nb_multiply`, `Py_sq_concat`), and getter-only `Py_tp_getset`; unsupported slot IDs still raise immediately and `Py_tp_members` remains fail-fast (not yet lowered). |
| `PyType_FromSpec` | Heap-type creation from `PyType_Spec` | Partial | Header shim delegates to `PyType_FromSpecWithBases(spec, NULL)` with the same slot/flag constraints. |
| `PyType_FromModuleAndSpec` | Heap-type creation with associated module | Partial | Header shim delegates to `PyType_FromSpecWithBases` and records module association for later `PyType_GetModule*` lookups. |
| `PyType_GetModule` | Get associated module from heap type | Partial | Header shim returns the associated module token and raises `TypeError` when the type has no associated module. |
| `PyType_GetModuleState` | Get associated module state from heap type | Partial | Header shim maps through `PyType_GetModule` + `PyModule_GetState` semantics. |
| `PyType_GetModuleByDef` | Resolve associated module via `PyModuleDef*` across MRO | Partial | Header shim performs an O(n) MRO walk with fail-fast errors when no matching module definition is associated. |
| `PyImport_ImportModule` | Import module by dotted name | Partial | Header shim maps to `molt_module_import` and returns imported module object on success. |
| `PyCapsule_New` | Create capsule from pointer + optional name | Partial | `include/molt/Python.h` maps to runtime/native `molt_capsule_new`. |
| `PyCapsule_GetName` | Get capsule name bytes | Partial | `include/molt/Python.h` maps to runtime/native `molt_capsule_get_name_ptr`. |
| `PyCapsule_GetPointer` | Resolve capsule pointer with optional name check | Partial | `include/molt/Python.h` maps to runtime/native `molt_capsule_get_pointer`. |
| `PyCapsule_IsValid` | Capsule shape + optional name validation | Partial | `include/molt/Python.h` maps to runtime/native `molt_capsule_is_valid`. |
| `PyCapsule_GetContext` | Get capsule context pointer | Partial | `include/molt/Python.h` maps to runtime/native `molt_capsule_get_context`. |
| `PyCapsule_SetContext` | Set capsule context pointer | Partial | `include/molt/Python.h` maps to runtime/native `molt_capsule_set_context`. |
| `PyCapsule_Import` | Import capsule pointer by dotted path | Partial | `include/molt/Python.h` maps to runtime/native `molt_capsule_import`. |
| `PyThreadState_Get` | Current thread state | Partial | Header shim returns a singleton thread-state token when GIL is held and raises otherwise. |
| `PyGILState_Ensure` | Acquire GIL if needed | Partial | Header shim routes to `molt_gil_is_held` + `molt_gil_acquire`. |
| `PyGILState_Release` | Release ensured GIL | Partial | Header shim conditionally routes to `molt_gil_release`. |
| `PyModule_NewObject` | Init module from name object | Partial | `include/molt/Python.h` maps to `molt_module_create`. |
| `PyModule_New` | Init module from UTF-8 name | Partial | `include/molt/Python.h` maps to `_molt_string_from_utf8` + `molt_module_create`. |
| `PyModule_Create` | Init module from `PyModuleDef` | Partial | `include/molt/Python.h` maps to `molt_module_create` (`PyModule_Create2` shim) and records `PyModuleDef` metadata. |
| `PyModuleDef_Init` | Initialize module definition | Partial | Header shim returns the passed `PyModuleDef` pointer as a `PyObject*` token. |
| `PyModule_AddObjectRef` | `mod.attr = v` (borrowed) | Partial | `include/molt/Python.h` maps to `molt_module_add_object_bytes`. |
| `PyModule_AddObject` | `mod.attr = v` (steals ref) | Partial | Header shim wraps `PyModule_AddObjectRef` and decrefs on success. |
| `PyModule_Add` | Alias of `PyModule_AddObject` | Partial | Header shim alias. |
| `PyModule_AddType` | Add type attribute | Partial | `include/molt/Python.h` maps to `molt_module_add_type`. |
| `PyModule_GetObject` | Get module attribute | Partial | `include/molt/Python.h` maps to `molt_module_get_object_bytes`. |
| `PyModule_GetNameObject` | Get module `__name__` object | Partial | Header shim uses `PyModule_GetObject`. |
| `PyModule_GetName` | Get UTF-8 module name | Partial | Header shim copies UTF-8 bytes into thread-local storage. |
| `PyModule_GetDef` | Get creating `PyModuleDef` | Partial | Header shim returns metadata attached by `PyModule_Create2`; returns `NULL` when unavailable. |
| `PyModule_GetState` | Get per-module state pointer | Partial | Header shim returns metadata attached by `PyModule_Create2`; currently allocates only when `m_size > 0`. |
| `PyModule_SetDocString` | Set module docstring | Partial | Header shim writes `__doc__` via Molt attribute setters. |
| `PyModule_GetFilenameObject` | Get module `__file__` object | Partial | Header shim validates a string-like `__file__` attribute and raises when unavailable. |
| `PyModule_GetFilename` | Get UTF-8 module filename | Partial | Header shim copies UTF-8 bytes into thread-local storage. |
| `PyModule_AddFunctions` | Add `PyMethodDef[]` to module | Partial | Header shim registers callback-backed callables via `molt_module_add_cfunction_bytes` for `METH_VARARGS`, `METH_VARARGS|METH_KEYWORDS`, `METH_NOARGS`, and `METH_O`; class/static method flags (`METH_CLASS`, `METH_STATIC`) are supported in type slot method tables. |
| `PyModule_FromDefAndSpec(2)` | Create module from `ModuleSpec` | Partial | Header shim creates module from `spec.name` (fallback: `m_name`), attaches C-API metadata, and stores `__spec__`. |
| `PyModule_ExecDef` | Execute module definition | Partial | Header shim wires metadata/doc/method registration and `PyState_AddModule`; unsupported callback flag combinations still fail fast. |
| `PyState_AddModule` | Register module by def pointer | Partial | Runtime-backed O(1) registry keyed by `PyModuleDef*` with ref-held module entries. |
| `PyState_FindModule` | Find module by def pointer | Partial | Header shim resolves via runtime-backed state registry and returns `NULL` when absent. |
| `PyState_RemoveModule` | Remove module by def pointer | Partial | Runtime-backed removal with explicit error when entry is missing. |
| `PyModule_AddIntConstant` | Add integer constant | Partial | `include/molt/Python.h` maps to `molt_module_add_int_constant`. |
| `PyModule_AddStringConstant` | Add string constant | Partial | `include/molt/Python.h` maps to `molt_module_add_string_constant`. |

### 2.7 Memory & Refcounting
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `Py_Incref` | inc ref | Partial | Header shim maps to `molt_handle_incref` via `Py_IncRef`/`Py_INCREF`. |
| `Py_Decref` | dec ref | Partial | Header shim maps to `molt_handle_decref` via `Py_DecRef`/`Py_DECREF`. |
| `PyMem_Malloc` | malloc | Partial | Header shim maps to host allocator + `PyErr_NoMemory` on failure. |
| `PyMem_Free` | free | Partial | Header shim maps to host allocator `free`. |

### 2.8 Buffer Protocol
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyObject_GetBuffer` | Export buffer | Partial | Header shim maps to `molt_buffer_acquire` and `Py_buffer` aliases `MoltBufferView`. |
| `PyBuffer_Release` | Release buffer | Partial | Header shim maps to `molt_buffer_release`. |

### 2.9 Bytes & Bytearray
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyBytes_FromStringAndSize` | bytes from data | Partial | `include/molt/Python.h` maps to `molt_bytes_from`. |
| `PyBytes_AsStringAndSize` | bytes pointer+len | Partial | `include/molt/Python.h` maps to `molt_bytes_as_ptr`. |
| `PyByteArray_FromStringAndSize` | bytearray from data | Partial | `include/molt/Python.h` maps to `molt_bytearray_from`. |
| `PyByteArray_AsString` | bytearray pointer | Partial | `include/molt/Python.h` maps to `molt_bytearray_as_ptr`. |
| `PyByteArray_Size` | bytearray length | Partial | Header shim derives length through `molt_bytearray_as_ptr`. |

### 2.10 Argument Parsing
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyArg_UnpackTuple` | Unpack positional tuple into `PyObject**` outputs | Partial | Header shim validates tuple arity bounds and returns borrowed tuple items. |
| `PyArg_ParseTuple` | Parse positional args | Partial | O(n) shim landed in `include/molt/Python.h` for `O,O!,b,B,h,H,i,I,l,k,L,K,n,c,d,f,p,s,s#,z,z#,y#` plus `|`/`$` markers. |
| `PyArg_ParseTupleAndKeywords` | Parse args + kwargs | Partial | O(n + k) shim landed with kwlist-driven keyword lookup, duplicate positional/keyword detection, and optional/keyword-only marker support (`|`, `$`) for the same core format subset. |
| `PyArg_VaParseTupleAndKeywords` | Parse args + kwargs from `va_list` | Partial | Symbol exists and currently fails fast with a standardized runtime error while full varargs parity is implemented. |

## 3. Unsupported / Dangerous
These symbols are deliberately missing or trap.

- `PyEval_GetFrame`: No frame introspection in native code.
- `PyGILState_*`: Header shim routes to `molt_gil_*`; semantics remain partial versus CPython.
- `PyRun_*`: No dynamic execution of strings via C API yet.

## 4. Implemented `libmolt` Bootstrap Symbols (native wrappers)
The following symbols are currently exported and form the active C-API bootstrap
surface:

- Runtime/GIL: `molt_c_api_version`, `molt_init`, `molt_shutdown`,
  `molt_gil_acquire`, `molt_gil_release`, `molt_gil_is_held`.
- Handle lifetime: `molt_handle_incref`, `molt_handle_decref`.
- Scalar constructors/accessors: `molt_none`, `molt_bool_from_i32`,
  `molt_int_from_i64`, `molt_int_as_i64`, `molt_float_from_f64`,
  `molt_float_as_f64`.
- Errors: `molt_err_set`, `molt_err_format`, `molt_err_clear`,
  `molt_err_pending`, `molt_err_peek`, `molt_err_fetch`,
  `molt_err_restore`, `molt_err_matches`.
- Object protocol: `molt_object_getattr`, `molt_object_setattr`,
  `molt_object_getattr_bytes`, `molt_object_setattr_bytes`,
  `molt_object_hasattr`, `molt_object_call`, `molt_object_get_iter`,
  `molt_iterator_next`, `molt_object_repr`, `molt_object_str`,
  `molt_object_truthy`, `molt_object_equal`, `molt_object_not_equal`,
  `molt_object_contains`, `molt_capsule_new`, `molt_capsule_get_name_ptr`,
  `molt_capsule_get_pointer`, `molt_capsule_is_valid`,
  `molt_capsule_get_context`, `molt_capsule_set_context`,
  `molt_capsule_import`.
- Numerics: `molt_number_add`, `molt_number_sub`, `molt_number_mul`,
  `molt_number_truediv`, `molt_number_floordiv`, `molt_number_long`,
  `molt_number_float`.
- Sequence/mapping: `molt_sequence_length`, `molt_sequence_getitem`,
  `molt_sequence_setitem`, `molt_sequence_to_list`, `molt_sequence_to_tuple`,
  `molt_mapping_getitem`, `molt_mapping_setitem`, `molt_mapping_length`,
  `molt_mapping_keys`, `molt_tuple_from_array`, `molt_list_from_array`,
  `molt_dict_from_pairs`.
- Buffer/bytes: `molt_buffer_acquire`, `molt_buffer_release`,
  `molt_bytes_from`, `molt_bytes_as_ptr`, `molt_string_from`,
  `molt_string_as_ptr`, `molt_bytearray_from`, `molt_bytearray_as_ptr`.
- Type/module parity: `molt_type_ready`, `molt_module_create`,
  `molt_module_import`, `molt_module_get_dict`, `molt_module_capi_register`,
  `molt_module_capi_get_def`, `molt_module_capi_get_state`,
  `molt_module_state_add`, `molt_module_state_find`,
  `molt_module_state_remove`, `molt_module_add_object`,
  `molt_module_add_object_bytes`, `molt_module_get_object`,
  `molt_module_get_object_bytes`, `molt_module_add_type`,
  `molt_module_add_int_constant`, `molt_module_add_string_constant`,
  `molt_cfunction_create_bytes`, `molt_module_add_cfunction_bytes`.

## 5. TODOs
- TODO(c-api, owner:runtime, milestone:SL3, priority:P1, status:partial): Implement the remaining `libmolt` C-API v0 surface per `0214` and keep this matrix aligned with real coverage.
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:partial): Expand `PyArg_ParseTuple`/`PyArg_ParseTupleAndKeywords` shim coverage and tighten edge-case parity diagnostics.
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:partial): Keep the documented `Py_LIMITED_API` target aligned with the shipped 3.12 (`0x030C0000`) compatibility lane and document any future version-gated deltas explicitly.
