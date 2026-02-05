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
  `docs/spec/areas/compat/0214_LIBMOLT_C_API_V0.md`.
- **Current Status:** No C API layer is implemented in the repo yet; all symbols below are targets and should be treated as **Missing** until a `libmolt` shim lands.
- **Hollow Symbols (future):** Some symbols may exist but return generic errors or empty values if their functionality (e.g., GC inspection) is not supported (TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:planned): define hollow-symbol policy + error surface).

## 2. Symbol Matrix
Status legend:
- **Planned (v0)**: part of the `libmolt` C-API v0 surface (`0214`).
- **Missing**: not yet implemented.
- **Future**: explicitly out of scope for v0.

### 2.1 Object Protocol (PyObject_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyObject_GetAttr` | getattr(o, name) | Planned (v0) | - |
| `PyObject_SetAttr` | setattr(o, name, v) | Planned (v0) | - |
| `PyObject_HasAttr` | hasattr(o, name) | Planned (v0) | - |
| `PyObject_Call` | o(*args) | Planned (v0) | Slow path (boxes args). |
| `PyObject_Repr` | repr(o) | Planned (v0) | - |
| `PyObject_Str` | str(o) | Planned (v0) | - |
| `PyObject_IsTrue` | bool(o) | Planned (v0) | - |
| `PyObject_RichCompare`| compare | Missing | - |
| `PyObject_GetIter` | iter(o) | Missing | - |
| `PyObject_Next` | next(o) | Missing | - |

### 2.2 Numbers (PyNumber_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyNumber_Add` | a + b | Planned (v0) | - |
| `PyNumber_Subtract` | a - b | Planned (v0) | - |
| `PyNumber_Multiply` | a * b | Planned (v0) | - |
| `PyNumber_TrueDivide` | a / b | Planned (v0) | - |
| `PyNumber_FloorDivide`| a // b | Planned (v0) | - |
| `PyNumber_Long` | int(o) | Planned (v0) | - |
| `PyNumber_Float` | float(o) | Planned (v0) | - |

### 2.3 Sequences (PySequence_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PySequence_Check` | is sequence? | Missing | - |
| `PySequence_Size` | len(o) | Planned (v0) | - |
| `PySequence_GetItem` | o[i] | Planned (v0) | - |
| `PySequence_SetItem` | o[i] = v | Planned (v0) | - |
| `PySequence_DelItem` | del o[i] | Missing | - |
| `PySequence_List` | list(o) | Missing | - |
| `PySequence_Tuple` | tuple(o) | Missing | - |

### 2.4 Mapping (PyMapping_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyMapping_Check` | is mapping? | Missing | - |
| `PyMapping_Size` | len(o) | Missing | - |
| `PyMapping_GetItemKey`| o[key] | Planned (v0) | Generic mapping get. |
| `PyMapping_SetItemString`| o[key] = v | Planned (v0) | Generic mapping set. |
| `PyMapping_Keys` | o.keys() | Missing | Returns list. |
| `PyMapping_Values` | o.values() | Missing | Returns list. |

### 2.5 Exceptions (PyErr_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyErr_Occurred` | Check exc | Planned (v0) | Thread-local check. |
| `PyErr_SetString` | Raise msg | Planned (v0) | - |
| `PyErr_SetObject` | Raise obj | Planned (v0) | - |
| `PyErr_Clear` | Clear exc | Planned (v0) | - |
| `PyErr_Fetch` | Get exc | Planned (v0) | No traceback objects yet. |
| `PyErr_Restore` | Set exc | Planned (v0) | - |
| `PyErr_Matches` | Type matches | Planned (v0) | - |
| `PyErr_Format` | Format msg | Planned (v0) | - |

### 2.6 Types & Modules
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyType_Ready` | Init type | Missing | Needed for static types. |
| `PyModule_Create` | Init module | Missing | - |
| `PyModule_AddObject` | mod.attr = v | Missing | - |

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
| `PyBytes_FromStringAndSize` | bytes from data | Planned (v0) | - |
| `PyBytes_AsStringAndSize` | bytes pointer+len | Planned (v0) | - |
| `PyByteArray_FromStringAndSize` | bytearray from data | Planned (v0) | - |
| `PyByteArray_AsString` | bytearray pointer | Planned (v0) | - |
| `PyByteArray_Size` | bytearray length | Missing | Prefer buffer protocol. |

## 3. Unsupported / Dangerous
These symbols are deliberately missing or trap.

- `PyEval_GetFrame`: No frame introspection in native code.
- `PyGILState_*`: No GIL in Molt (except bridge). Stubs provided.
- `PyRun_*`: No dynamic execution of strings via C API yet.

## 4. TODOs
- TODO(c-api, owner:runtime, milestone:SL3, priority:P1, status:missing): Implement the `libmolt` C-API v0 surface per `0214` and update this matrix with real coverage.
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:missing): Implement `PyArg_ParseTuple` for extension argument parsing.
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:planned): Define the `Py_LIMITED_API` version Molt targets (3.10?).
