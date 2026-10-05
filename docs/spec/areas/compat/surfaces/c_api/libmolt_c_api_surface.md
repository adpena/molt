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

Generic C subscription (`PyObject_GetItem`, `PyObject_SetItem`,
`PyObject_DelItem`) uses the runtime's live Python class protocol for every
managed value. Physical storage tags do not select semantics; subclass overrides,
arbitrary keys, zero-valued payloads, and the original callback exception survive
the boundary. Foreign extension objects retain their declared C-slot protocol.
Physical container APIs such as `PyDict_SetItem` remain distinct because their
contract intentionally bypasses subclass subscription overrides. Mutation returns
are statements with borrowed receivers, not owned result values.

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
  - Current version constant: `MOLT_C_API_VERSION = 3`

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

`molt_float_as_f64` is the native scalar extraction contract for compiled code:
it accepts inline floats, heap-backed NaN floats, and integer-compatible values.
Heap-backed NaN floats must round-trip as IEEE NaN values; pointer identity is
not observable through this accessor.

### 4.4 Object Protocol
- `molt_object_getattr`, `molt_object_setattr`, `molt_object_hasattr`
- `molt_object_getattr_bytes`, `molt_object_setattr_bytes`
- `molt_object_call`
- `molt_object_repr`, `molt_object_str`, `molt_object_truthy`
- `molt_object_equal`, `molt_object_not_equal`, `molt_object_contains`
- `molt_c_heap_register`, `molt_c_heap_unregister`, `molt_c_heap_contains`
- `molt_c_heap_type_canonicalize`
- `molt_c_heap_register_buffer_exporter`, `molt_c_heap_register_buffer_releaser`
- `molt_c_heap_export_buffer`, `molt_c_heap_release_buffer`

`molt_c_heap_*` is the public-header C-object provenance lane. It lets
source-compatible headers expose real C heap pointers for extension-local
objects. The source-header `Py_INCREF`/`Py_DECREF`, `Py_REFCNT` and `Py_TYPE`
operations retain the private reference-count and type-pointer representation;
they never interpret the pointer as a Molt handle. Kind canonicalization keeps
that private type-pointer identity across C translation units. Registration
does not create a CPython object prefix, MRO, slot table or Python class
projection. Canonical object inquiry and class-info protocols therefore reject
registered private storage with `TypeError` before any CPython layout access;
predicates return false and error-returning APIs retain their normal failure
sentinels. This admission is owned by the bridge's existing Foreign resolution,
shared by both C headers and runtime-value ingress. The runtime membership hook
uses this same registry and does not hold its lock while constructing errors.
Buffer leases and the source-header representation operations remain admitted;
canonical extension objects use `PyType_FromSpec` and the linked object APIs.

The `molt_c_heap_*_buffer*` lease functions extend that lane to the buffer
protocol: a source-recompiled extension (e.g. the numpy `PyArrayObject`
header) registers a per-kind exporter and releaser keyed on its typed C-heap
header, then `molt_c_heap_export_buffer` hands out a `MoltBufferView` lease only
after the runtime revalidates the descriptor through the same typed strided
storage authority as `molt_memoryview_from_buffer`. A C-heap lease owns its own
backing (`owner == 0`, `base == 0`); a descriptor whose declared length or
strided span does not fit its backing capacity fails closed, draining the lease
through the registered releaser so the exporter's slot-identity bookkeeping
stays balanced. `PyObject_GetBuffer`/`PyBuffer_Release`/`PyObject_CheckBuffer`
route C-heap objects through this lease lane and runtime objects through
`molt_buffer_acquire`/`molt_buffer_release`. Unregistering a C-heap type pointer
via `molt_c_heap_unregister` revokes its canonical type mapping and buffer
exporter/releaser hooks; stale type authority must not keep future buffer
exports alive.

### 4.5 Numerics
- `molt_number_add`, `molt_number_sub`, `molt_number_mul`
- `molt_number_truediv`, `molt_number_floordiv`
- `molt_number_long`, `molt_number_float`

### 4.6 Sequences + Mappings
- `molt_sequence_length`, `molt_sequence_getitem`, `molt_sequence_setitem`
- `molt_mapping_getitem`, `molt_mapping_setitem`, `molt_mapping_length`, `molt_mapping_keys`
- `molt_tuple_from_array`, `molt_list_from_array`, `molt_dict_from_pairs`

### 4.7 Buffer + Bytes
- `molt_buffer_acquire`, `molt_buffer_export`, `molt_buffer_release`
- `molt_bytes_from`, `molt_bytes_as_ptr`
- `molt_string_from`, `molt_string_as_ptr`
- `molt_bytearray_from`, `molt_bytearray_as_ptr`

Both C facades share the canonical CPython-prefix `Py_buffer` and linked entry
points. C-API major 5 rejects artifacts compiled for the former private tail;
rebuild is mandatory and sealing cannot relabel their layout. Runtime MemoryView
is the sole semantic owner, with a stable descriptor in its existing BridgeEntry.
The descriptor has no cache eviction or independent exporter ownership. Native
leases are shared across derived views and traced by mixed GC; release publishes
the view empty before callbacks. Python methods, indexing and shared buffer
acquisition work for Python-created and C-created views alike.

`PyBuffer_FillInfo` remains allocation-free and publishes self-referential
shape/stride pointers. Native memoryviews preserve complete format strings;
byte conversion consumes geometry without interpreting format text. Indirect
suboffset buffers fail closed. Noncontiguous exports require stride metadata.
Bytes and bytearray share buffer-before-iteration selection after the applicable
special-method and index/count policies. Acquisition and release preserve the
original conversion error. `readonly` is a canonical u32
boolean: `0` means writable, `1` means read-only, and every other value fails
descriptor admission.

Buffer acquisition owns an export lifetime, not merely an object reference.
Mutable backing storage must not resize while any live view or C buffer lease
exports it. Derived views retain the storage owner independently: releasing a
parent view does not invalidate a slice, cast, clone or readonly derivative.
Explicit release is idempotent but refuses while that view itself has active C
exports. Normal destruction and GC use the same release accounting. A descriptor
with distinct `owner` and `base` retains both identities without conflating the
storage lease with the Python-visible base object.

Bytearray permits same-length edits under export; BytesIO uses its stricter
contract and rejects writes (including empty writes), truncate and close while
exported. Failed flush/close must preserve pending bytes and leave the object
retryable. Native and WASM I/O consumers use the same ownership authority.
Writable simple-buffer consumers admit C-contiguous typed, shaped and scalar
storage by byte capacity, not element count; noncontiguous destinations are
rejected instead of copied through a strided fallback. BytesIO `readinto` permits
overlapping exports of its own storage and uses overlap-safe copying without
simultaneous mutable/immutable Rust byte-slice references.

Contiguous borrowing, arbitrary-stride gathering and scalar writes share validated shape,
signed extent and backing identity. A bounds failure cannot be retried through
an unchecked copying path. Scalar assignment completes Python conversion before
borrowing writable bytes, then revalidates the view (including release by the
conversion callback). These are implementation contracts, not certification of
every Python-version, platform, package or release matrix cell.

Scalar operation-entry admission rejects released/readonly views before key or
value conversion. After key callbacks, conversion errors retain their documented
precedence over the final release check.
Numeric packing translates conversion `TypeError` to the format-specific type
diagnostic and `OverflowError`/`ValueError` to its value diagnostic; boolean truth testing
preserves the original exception. Buffer C-API contiguity and cached memoryview
flags are distinct CPython surfaces: an empty rank-one strided view can report
`c_contiguous == False` while `PyBuffer_IsContiguous` returns true. Do not use
one of those observations as a proxy for the other.

Ordinary, stepped and C-API memoryview slices share first-axis normalization and
validated storage derivation. They preserve trailing shape/strides, root owner
and native lease, including empty slices; rank-zero slicing is rejected before
index conversion. Derived storage owns its base/format references, counted
root export and native lease before index callbacks run; releasing the parent
inside a callback does not invalidate the slice or permit exporter resizing.
That same ownership pin transfers into the allocated view or unwinds while
preserving the callback error. Stride multiplication may wrap only when the
validated result has at most one first-axis element or no elements at all;
otherwise invalid geometry raises `BufferError`.
Derivation preserves inline format descriptors as well as referenced formats,
and a failed derivation leaves the original geometry intact. Cast dimensions
use the shared checked byte extent; overflow raises the CPython shape-product
error before inspecting later dimensions, rather than silently returning `None`.

Slice assignment uses that same checked geometry and byte traversal. It acquires
the source export before step/start/stop conversion, retains it until copying or
failure cleanup completes, and rechecks destination release after conversion.
It does not pin the destination: callbacks may release it and resize its former
owner. Callback failures precede release; release precedes structural mismatch.
Contiguous overlapping copies use memmove semantics; strided copies stage the
source before any destination write. No unused final stride increment or failed
geometry calculation may silently skip an assignment.

Scalar stores perform Python conversion and native C-width integer admission,
then directly recheck release, then apply the destination range and encode.
Only conversion failures enter numeric error translation: a released-view
error retains its exact message, and small-format range or half-float packing
failures produce the final format value error. Pointer format `P` follows
`PyLong_AsVoidPtr` integer admission without invoking `__index__`. No exporter
pointer is retained across scalar callbacks.

Store entry admits release, format syntax and readonly before key callbacks.
After key conversion, bounds errors, later tuple callbacks, unsupported scalar
formats and value-conversion errors retain precedence over release. Tuple reads
likewise finish per-axis bounds and later callbacks before their final release
check; scalar reads reenter item admission after key conversion. Release keeps
shape/strides alive until view destruction; these callbacks retain no borrowed
exporter data. Allocator pins transfer geometry without extra vector copies,
skip empty cleanup, and drop a redundant base reference plainly only after the
initialized view owns the same exporter. Actual abort/finalizer cleanup still
preserves the pending exception.

Known source-parity limit: the pinned CPython 3.12.13/3.13.11/3.14.3 `c` pack
branch does not recheck release after a key callback and may use its earlier
data pointer. Molt retains a final release check for this case. Character
conversion failures still precede release, but a successful character store
after key-induced release is not claimed as exact CPython parity. This includes
defined cases where another view keeps the storage exported; it is not limited
to CPython's dangling-pointer cases after the final export is released. Molt's
current release operation clears its data and ownership edges. Restoring the
defined cases needs shared, non-exporting observation of storage lifetime;
adding a view/root pin would change permitted release/resize callbacks, and a
native lease clone would defer the exporter's release callback. The exact gap
is tracked in the [type coverage matrix](../language/type_coverage_matrix.md).

Typed frontend loops use the same runtime iterator admission as `iter(view)`:
empty multidimensional or invalid-format
views cannot bypass it. An unexhausted iterator on a released view raises without
advancing; an already exhausted iterator stays exhausted. Unsupported scalar
codes are still deferred until an element is requested.

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
- NumPy source-compat headers are no longer shipped by Molt. Source-recompiled
  NumPy/Scipy extension builds must admit `numpy/*` headers from the package's
  own source/build custody include dirs (for example `numpy/_core/include`).
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
- Molt does not ship NumPy compatibility headers. Package-owned `numpy/*`
  headers are supplied by package/source-plan custody.
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
