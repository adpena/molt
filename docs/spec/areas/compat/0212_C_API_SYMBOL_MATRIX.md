# C API Symbol Matrix
**Spec ID:** 0212
**Status:** Draft
**Owner:** runtime
**Goal:** Define the subset of `Python.h` exposed by Molt's runtime for C extensions and the Bridge.

## 1. Strategy
- **Binary Compatibility:** Molt does *not* aim for ABI compatibility with `libpython.so`. Extensions must be recompiled against `libmolt`.
- **Source Compatibility:** We aim for source compatibility for a high-value subset of the Limited API (Py_LIMITED_API).
- **Current Status:** No C API layer is implemented in the repo yet; all symbols below are targets and should be treated as **Missing** until a `libmolt` shim lands.
- **Hollow Symbols (future):** Some symbols may exist but return generic errors or empty values if their functionality (e.g., GC inspection) is not supported (TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:planned): define hollow-symbol policy + error surface).

## 2. Symbol Matrix

### 2.1 Object Protocol (PyObject_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyObject_GetAttr` | getattr(o, name) | Missing | - |
| `PyObject_SetAttr` | setattr(o, name, v) | Missing | - |
| `PyObject_HasAttr` | hasattr(o, name) | Missing | - |
| `PyObject_Call` | o(*args) | Missing | Slow path (boxes args). |
| `PyObject_Repr` | repr(o) | Missing | - |
| `PyObject_Str` | str(o) | Missing | - |
| `PyObject_IsTrue` | bool(o) | Missing | - |
| `PyObject_RichCompare`| compare | Missing | - |
| `PyObject_GetIter` | iter(o) | Missing | - |
| `PyObject_Next` | next(o) | Missing | - |

### 2.2 Numbers (PyNumber_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyNumber_Add` | a + b | Missing | - |
| `PyNumber_Subtract` | a - b | Missing | - |
| `PyNumber_Multiply` | a * b | Missing | - |
| `PyNumber_TrueDivide` | a / b | Missing | - |
| `PyNumber_FloorDivide`| a // b | Missing | - |
| `PyNumber_Long` | int(o) | Missing | - |
| `PyNumber_Float` | float(o) | Missing | - |

### 2.3 Sequences (PySequence_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PySequence_Check` | is sequence? | Missing | - |
| `PySequence_Size` | len(o) | Missing | - |
| `PySequence_GetItem` | o[i] | Missing | - |
| `PySequence_SetItem` | o[i] = v | Missing | - |
| `PySequence_DelItem` | del o[i] | Missing | - |
| `PySequence_List` | list(o) | Missing | - |
| `PySequence_Tuple` | tuple(o) | Missing | - |

### 2.4 Mapping (PyMapping_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyMapping_Check` | is mapping? | Missing | - |
| `PyMapping_Size` | len(o) | Missing | - |
| `PyMapping_GetItemKey`| o[key] | Missing | - |
| `PyMapping_SetItemString`| o[key] = v | Missing | - |
| `PyMapping_Keys` | o.keys() | Missing | Returns list. |
| `PyMapping_Values` | o.values() | Missing | Returns list. |

### 2.5 Exceptions (PyErr_*)
| Symbol | Semantics | Status | Notes |
| --- | --- | --- | --- |
| `PyErr_Occurred` | Check exc | Missing | Thread-local check. |
| `PyErr_SetString` | Raise msg | Missing | - |
| `PyErr_SetObject` | Raise obj | Missing | - |
| `PyErr_Clear` | Clear exc | Missing | - |
| `PyErr_Fetch` | Get exc | Missing | No traceback objects yet. |
| `PyErr_Restore` | Set exc | Missing | - |

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

## 3. Unsupported / Dangerous
These symbols are deliberately missing or trap.

- `PyEval_GetFrame`: No frame introspection in native code.
- `PyGILState_*`: No GIL in Molt (except bridge). Stubs provided.
- `PyRun_*`: No dynamic execution of strings via C API yet.

## 4. TODOs
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:missing): Implement a `libmolt` C API shim and update this matrix with real coverage (no symbols exist today).
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:missing): Implement buffer protocol (`PyObject_GetBuffer`).
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:missing): Implement `PyArg_ParseTuple` for extension argument parsing.
- TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:planned): Define `Py_LIMITED_API` version Molt targets (3.10?).
