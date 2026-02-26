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
- **Current Status:** A `libmolt` C-API bootstrap surface is implemented with
  `molt_*` wrapper symbols (`runtime/molt-runtime/src/c_api.rs` + `include/molt/molt.h`).
  CPython `Py*` source-compat symbols remain targets and should still be treated
  as **Missing** until explicit compatibility shims are landed.
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
| `PyObject_RichCompare`| compare | Missing | - |
| `PyObject_GetIter` | iter(o) | Missing | - |
| `PyObject_Next` | next(o) | Missing | - |

### 2.2 Numbers (PyNumber_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyNumber_Add` | a + b | Partial | `include/molt/Python.h` maps to `molt_number_add`. |
| `PyNumber_Subtract` | a - b | Partial | `include/molt/Python.h` maps to `molt_number_sub`. |
| `PyNumber_Multiply` | a * b | Partial | `include/molt/Python.h` maps to `molt_number_mul`. |
| `PyNumber_TrueDivide` | a / b | Partial | `include/molt/Python.h` maps to `molt_number_truediv`. |
| `PyNumber_FloorDivide`| a // b | Partial | `include/molt/Python.h` maps to `molt_number_floordiv`. |
| `PyNumber_Long` | int(o) | Partial | `include/molt/Python.h` maps to `molt_int_as_i64`. |
| `PyNumber_Float` | float(o) | Partial | `include/molt/Python.h` maps to `molt_float_as_f64`. |

### 2.3 Sequences (PySequence_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PySequence_Check` | is sequence? | Missing | - |
| `PySequence_Size` | len(o) | Partial | `include/molt/Python.h` maps to `molt_sequence_length`. |
| `PySequence_GetItem` | o[i] | Partial | `include/molt/Python.h` maps to `molt_sequence_getitem`. |
| `PySequence_SetItem` | o[i] = v | Partial | `include/molt/Python.h` maps to `molt_sequence_setitem`. |
| `PySequence_DelItem` | del o[i] | Missing | - |
| `PySequence_List` | list(o) | Missing | - |
| `PySequence_Tuple` | tuple(o) | Missing | - |

### 2.4 Mapping (PyMapping_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyMapping_Check` | is mapping? | Missing | - |
| `PyMapping_Size` | len(o) | Partial | `include/molt/Python.h` maps to `molt_mapping_length`. |
| `PyMapping_GetItemKey`| o[key] | Partial | `include/molt/Python.h` maps to `molt_mapping_getitem`/`PyMapping_GetItemString`. |
| `PyMapping_SetItemString`| o[key] = v | Partial | `include/molt/Python.h` maps to `molt_mapping_setitem`. |
| `PyMapping_Keys` | o.keys() | Partial | `include/molt/Python.h` maps to `molt_mapping_keys`. |
| `PyMapping_Values` | o.values() | Missing | Returns list. |

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

### 2.6 Types & Modules
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyType_Ready` | Init type | Partial | `include/molt/Python.h` maps to `molt_type_ready`. |
| `PyModule_Create` | Init module | Partial | `include/molt/Python.h` maps to `molt_module_create` (`PyModule_Create2` shim). |
| `PyModule_AddObject` | mod.attr = v | Partial | `include/molt/Python.h` maps to `molt_module_add_object_bytes` (+ int/string/ref helpers). |

### 2.7 Memory & Refcounting
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `Py_Incref` | inc ref | Missing | No-op if RC elided? No, strict. |
| `Py_Decref` | dec ref | Missing | - |
| `PyMem_Malloc` | malloc | Missing | Uses Molt allocator. |
| `PyMem_Free` | free | Missing | - |

### 2.8 Buffer Protocol
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyObject_GetBuffer` | Export buffer | Planned (v0) | 1D buffers first. |
| `PyBuffer_Release` | Release buffer | Planned (v0) | - |

### 2.9 Bytes & Bytearray
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyBytes_FromStringAndSize` | bytes from data | Partial | `include/molt/Python.h` maps to `molt_bytes_from`. |
| `PyBytes_AsStringAndSize` | bytes pointer+len | Partial | `include/molt/Python.h` maps to `molt_bytes_as_ptr`. |
| `PyByteArray_FromStringAndSize` | bytearray from data | Planned (v0) | - |
| `PyByteArray_AsString` | bytearray pointer | Planned (v0) | - |
| `PyByteArray_Size` | bytearray length | Missing | Prefer buffer protocol. |

### 2.10 Argument Parsing
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyArg_ParseTuple` | Parse positional args | Partial | O(n) shim landed in `include/molt/Python.h` for `O,O!,b,B,h,H,i,I,l,k,L,K,n,c,d,f,p,s,s#,z,z#,y#` plus `|`/`$` markers. |
| `PyArg_ParseTupleAndKeywords` | Parse args + kwargs | Partial | O(n + k) shim landed with kwlist-driven keyword lookup, duplicate positional/keyword detection, and optional/keyword-only marker support (`|`, `$`) for the same core format subset. |

## 3. Unsupported / Dangerous
These symbols are deliberately missing or trap.

- `PyEval_GetFrame`: No frame introspection in native code.
- `PyGILState_*`: No GIL in Molt (except bridge). Stubs provided.
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
  `molt_object_hasattr`, `molt_object_call`, `molt_object_repr`,
  `molt_object_str`, `molt_object_truthy`, `molt_object_equal`,
  `molt_object_not_equal`, `molt_object_contains`.
- Numerics: `molt_number_add`, `molt_number_sub`, `molt_number_mul`,
  `molt_number_truediv`, `molt_number_floordiv`, `molt_number_long`,
  `molt_number_float`.
- Sequence/mapping: `molt_sequence_length`, `molt_sequence_getitem`,
  `molt_sequence_setitem`, `molt_mapping_getitem`, `molt_mapping_setitem`,
  `molt_mapping_length`, `molt_mapping_keys`, `molt_tuple_from_array`,
  `molt_list_from_array`, `molt_dict_from_pairs`.
- Buffer/bytes: `molt_buffer_acquire`, `molt_buffer_release`,
  `molt_bytes_from`, `molt_bytes_as_ptr`, `molt_string_from`,
  `molt_string_as_ptr`, `molt_bytearray_from`, `molt_bytearray_as_ptr`.
- Type/module parity: `molt_type_ready`, `molt_module_create`,
  `molt_module_get_dict`, `molt_module_add_object`,
  `molt_module_add_object_bytes`, `molt_module_get_object`,
  `molt_module_get_object_bytes`, `molt_module_add_int_constant`,
  `molt_module_add_string_constant`.

## 5. TODOs
- TODO(c-api, owner:runtime, milestone:SL3, priority:P1, status:partial): Implement the remaining `libmolt` C-API v0 surface per `0214` and keep this matrix aligned with real coverage.
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:partial): Expand `PyArg_ParseTuple`/`PyArg_ParseTupleAndKeywords` shim coverage and tighten edge-case parity diagnostics.
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:planned): Define the `Py_LIMITED_API` version Molt targets (3.10?).
