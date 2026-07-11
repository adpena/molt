# CPython 3.12 C-API — Complete Coverage Matrix (behavioral / function tier)

> **Scope.** This is the *behavioral / functional* coverage map of Molt's `molt-cpython-abi` surface against **CPython 3.12** (`Include/object.h`, `abstract.h`, `longobject.h`, `floatobject.h`, `complexobject.h`, `dictobject.h`, `listobject.h`, `tupleobject.h`, `setobject.h`, `unicodeobject.h`, `bytesobject.h`, `bytearrayobject.h`, `codecs.h`, `pyerrors.h` and their `cpython/` + Stable-ABI companions). It is the *spec-first* replacement for reactive leaf-discovery: enumerate the surface, mark every symbol, then close it in batches. Every FAITHFUL claim was written against the primary CPython source (M06); the rows carrying an adversarial **verdict** were independently re-audited and several did **not** survive.

> **Companion docs.** The *binary / layout* tier (struct layout, exported data-symbol bytes, flag/slot-ID values) lives in [`CPYTHON_ABI_BINARY_CONTRACT_MATRIX.md`](CPYTHON_ABI_BINARY_CONTRACT_MATRIX.md) (79 items, 46 MATCH). The *per-defect* divergence ledger is [`CPYTHON_ABI_DIVERGENCE_LEDGER.md`](CPYTHON_ABI_DIVERGENCE_LEDGER.md) (248 rows). The *witness symbol frontier* is [`NATIVE_DISCOVERY_FRONTIERS.md`](NATIVE_DISCOVERY_FRONTIERS.md) (the 14 Tier-A symbols). This doc is the union coverage map that ties them together.

---

## 1. Executive summary

### 1.1 The honest number — how complete is Molt's CPython-3.12 C-API?

Across the **345 audited entries (≈510 named symbols)** of the object/data-model core surveyed here, **230 entries (67%) are gaps** and only **115 (33%) are even *claimed* faithful.** But the claimed-faithful bucket is inflated by unverified claims, so the honest answer is a **band, not a point**:

| honest coverage of the surveyed surface | entries | % | what it means |
|---|---:|---:|---|
| **Verified-faithful floor** | 11 | **3%** | passed an adversarial re-audit against CPython 3.12 source (CONFIRMED) |
| **Audit-projected faithful** | ≈49 | **≈14%** | floor + the 104 unverified claims discounted by the observed 37% confirm-rate |
| **Claimed-faithful ceiling** | 115 | **33%** | every FAITHFUL claim taken at face value (optimistic — do not use) |

**The load-bearing finding:** of the **30 FAITHFUL claims that were adversarially re-audited, 19 were DOWNGRADED** (partial/divergent) and only **11 were CONFIRMED — a 37% confirm-rate.** The remaining **104 FAITHFUL claims have never been adversarially verified.** Extrapolating the 63% downgrade rate, the true faithful coverage is almost certainly in the **~3–14% band, not the 33% ceiling.** Do not round up: *symbol-present ≠ behaviorally-faithful* — this is the entire reason the matrix exists.

> **Two scope caveats, stated plainly.**
> 1. **This surveyed surface is itself a subset of the whole C-API.** It covers the object/data-model core (object-protocol, type-slots, numbers, containers, strings/bytes, errors) — the surface numpy/scipy/cython actually hit — and excludes import/module/gil/eval/gc/capsule/memory/sys/frame/code/gen domains. Molt's ABI exports **605 `Py*` symbols** total (per the witness sweep); the full CPython 3.12 public+stable surface is ~4× that. So even the 33% ceiling is *over the object-model core only.*
> 2. **The errors-exc domain is enumerated only partially here** (4 function rows + a domain aggregate) because the source audit stream was truncated mid-domain. Its aggregate (§1.4, §2.6) is folded into the honest picture but its per-symbol rows are incomplete. Treat errors-exc as *worse* than the 5 fully-enumerated domains, not better.

### 1.2 Counts by status × severity (claimed, entries)

| claimed status | high | med | low | **total** | notes |
|---|---:|---:|---:|---:|---|
| **IMPLEMENTED_FAITHFUL** | 62 | 44 | 28 | **134** | claimed faithful; **only 11 CONFIRMED, 19 DOWNGRADED, 104 unverified** |
| **DIVERGENT** | 9 | 10 | 9 | **28** | present but wrong semantics for real input classes |
| **STUB_THEATER** | 0 | 1 | 0 | **1** | present, looks real, does nothing (`PyObject_ClearWeakRefs`) |
| **ALIAS_NEEDED** | 4 | 2 | 1 | **7** | impl exists under another name / as inline — needs an `#[export_name]`/header decl |
| **MISSING** | 17 | 80 | 78 | **175** | not exported, not in any header |
| | | | | **345** | |

### 1.3 Effective status after applying the adversarial verdicts

The 19 DOWNGRADED rows move out of *faithful* into *partial/divergent*. The honest gap-total is therefore **230**, not the 211 you'd get from the raw status column:

| effective status | entries | gap? |
|---|---:|:--:|
| FAITHFUL (verified, CONFIRMED) | 11 | no |
| FAITHFUL (claimed, unverified) | 104 | unproven |
| PARTIAL/DIVERGENT (was FAITHFUL → DOWNGRADE) | 19 | **yes** |
| DIVERGENT | 28 | **yes** |
| STUB_THEATER | 1 | **yes** |
| ALIAS_NEEDED | 7 | **yes** |
| MISSING | 175 | **yes** |
| **gap total** | **230** | |

### 1.4 errors-exc domain aggregate (partial data)

The errors/exceptions domain has a **fundamental model limit** that no single symbol row captures: the pending error is a thread-local **`(type_bits, String)` pair** — there is **no real exception *instance*, no `args`, no `__cause__`/`__context__` chain, and no traceback**. Every `PyErr_Fetch`/`Restore`/`Normalize` round-trip collapses the value to a string and drops the traceback. So even the 'implemented' error functions are systematically **DIVERGENT** on chaining/traceback/instance semantics.

| errors-exc aggregate | count |
|---|---:|
| domain spec functions (`PyErr_*`/`PyException_*`/`PyUnicode*Error_*`/`_PyErr_*`) | 101 |
| `no_mangle` functions implemented (errors.rs + shim variadics + Py_FatalError) | 25 (many DIVERGENT per the model limit) |
| exception-type `PyExc_*` data objects total | 69 |
| &nbsp;&nbsp;→ IMPLEMENTED_FAITHFUL | 49 |
| &nbsp;&nbsp;→ MISSING | 20 |

Witness-critical error gaps captured explicitly below: `PyErr_NewException`, `PyErr_NewExceptionWithDoc` (A-EXC), `_Py_FatalErrorFunc` (A-FATAL), `PyErr_Fetch` (DIVERGENT model limit).

### 1.5 Where the gaps concentrate

| domain | entries | biggest gap class |
|---|---:|---|
| object-protocol | 32 | number-protocol dispatch (`PyNumber_Add`) + vectorcall kwnames drop + witness A-SIZE/A-CYTHON aliases |
| type-slots | 33 | single-inheritance-only `PyType_Ready`/`FromSpec` (no HEAPTYPE, no MRO, no PEP-573 module state) |
| numbers | 88 | `PyUnstable_Long_*` (Cython-3 link blocker) + `PyLong_FromString` + type-object shells (`PyLong_Type` etc.) |
| containers | 30 | `PyDictProxy_New` stub + `PyList_Append` silent-success + `_PyList_Extend` + frozenset-refcount + view/iter types |
| strings-bytes | 158 | **UTF-8 validation missing in `FromString*`** (silent wrong answer) + `AsUTF8` NUL-termination (heap over-read) + str builders |
| errors-exc | 4 | no real exception instance/traceback/chaining; `PyErr_NewException` missing |

---

## 2. The full matrix (grouped by domain)

Within each domain, rows are ordered gaps-first by severity (high → med → low), then faithful rows. `✓verified` = CONFIRMED re-audit; `✗→PARTIAL/DIVERGENT` = the FAITHFUL claim was DOWNGRADED; `(claimed)` = asserted faithful, not independently re-audited.

### 1. object-protocol — `PyObject_*` / `PyNumber_*` / `PySequence_*` / `PyMapping_*` / `PyIter_*` / buffer / call

*32 entries — 23 gap, 9 faithful.*

| symbol | status | sev | needed_by | note |
|---|---|:--:|---|---|
| `PyNumber_Add (+ Subtract/Multiply/MatrixMultiply/FloorDivide/TrueDivide/Remainder/Divmod/Power/Lshift/Rshift/And/Or/Xor + ALL InPlace variants)` | DIVERGENT | high | numpy/scipy/cython | abstract_number.rs:59/150: binary_op() resolves BOTH operands via GLOBAL_BRIDGE.pyobj_to_handle and, on any bridge-miss (a foreign C object e.g. a numpy scalar/array), calls ensure_exception_set()->SystemError + returns NULL. It NEVER dispatches the operand type's tp_as_number nb_add/nb_multiply/... slots (contrast unary PyNumber_Long/Float/Index which DO dispatch foreign slots). PyNumber_Add(pyint, np_scalar) hard-fails where CPython succeeds. InPlace* additionally delegate to the non-in-place op (call __add__, not __iadd__). |
| `PyNumber_Long / PyNumber_Float / PyNumber_Index / PyNumber_AsSsize_t / PyIndex_Check` | FAITHFUL ✗→PARTIAL/DIVERGENT | high | numpy | abstract_number.rs:390+ native fast path + foreign nb_int/nb_float/nb_index slot dispatch (PyNumber_Long unblocked _multiarray_umath init); int boxing via int_from_i64 (no 47-bit trunc). DOWNGRADE: PyNumber_Long float fast path `v as i64` saturates NaN->0/inf->i64::MAX/1e300->trunc (silent wrong answer, class M36/M37); Long/Float reject str/bytes (CPython accepts) while error msg claims to accept them; AsSsize_t ignores `_exc` (no clamp/no raise contract) + i64->i32 wasm32 truncation; Float omits nb_index fallback. |
| `PyObject_Call / CallObject / CallNoArgs / CallOneArg / CallMethod{No,One}Arg / Vectorcall{Dict,Method} / PyVectorcall_Call` | FAITHFUL ✗→PARTIAL/DIVERGENT | high | numpy/cython | object.rs:2049+ tp_call dispatch + bridged object_call with C-tuple marshaling; variadic CallFunction/CallMethod in shim. DOWNGRADE: PyObject_Vectorcall drops kwnames (bare NULL, no exc); VectorcallMethod inherits that stub; PyVectorcall_Call delegates to PyObject_Call (CPython reads tp_vectorcall_offset; and PyVectorcall_Call in a tp_call slot -> unbounded recursion); VectorcallDict ignores PyVectorcall_NARGS mask (OFFSET flag -> negative nargs -> NULL); PyObject_Call never tries vectorcall-first; CallObject omits non-tuple TypeError. Positional-tuple no-kw path faithful. |
| `PyObject_GetBuffer` | FAITHFUL ✗→PARTIAL/DIVERGENT | high | numpy | buffer.rs:469 real typed strided descriptor via runtime buffer_acquire for natives AND dispatches foreign tp_as_buffer->bf_getbuffer (numpy PyArray_Type), honors PyBUF_WRITABLE/contiguity. DOWNGRADE: (1) non-exporter molt object -> BufferError 'object does not export a buffer' where CPython raises TypeError 'a bytes-like object is required' (and molt's own FOREIGN no-slot path correctly raises TypeError — internally inconsistent); (2) PyBuffer_IsContiguous F-order strides==NULL / len==0 edge cases wrong. Refcount/steal + foreign dispatch faithful. |
| `PyObject_GetItem / PyObject_SetItem / PyObject_DelItem` | FAITHFUL ✗→PARTIAL/DIVERGENT | high | numpy/cython | object.rs:1580/1674/1708 native dict/list/tuple fast lanes + foreign mp_subscript/sq_item dispatch with __index__ conversion + sq_length negative adjust. DOWNGRADE: NULL-arg contract broken (bare NULL/-1, no SystemError); __class_getitem__/Py_GenericAlias branch missing (list[int] wrong TypeError); native container tp_as_mapping/sequence are NULL so slice keys + foreign-key-into-native-dict dead-end in wrong TypeError; native dict SetItem discards runtime result (unhashable key reported as success). |
| `PyObject_Vectorcall` | DIVERGENT | high | cython/numpy (3.12 vectorcall) | object.rs:2457: `if !kwnames.is_null() { return null }` returns NULL WITHOUT setting an exception for every keyword-bearing fast-call (ABI-contract violation: NULL must carry an exception); silently drops all keyword args. Otherwise materializes a tuple and delegates to PyObject_Call rather than a true vectorcall. |
| `PySequence_Fast / Fast_GET_SIZE / Fast_GET_ITEM / Fast_ITEMS` | FAITHFUL ✗→PARTIAL/DIVERGENT | high | numpy | abstract_sequence.rs:1244 materializes any iterable into an ABI-layout tuple. DOWNGRADE: CPython returns SAME object for list/tuple and a NEW *list* for other iterables; molt returns a FRESH TUPLE COPY for native tuple + every list (wrong identity+type). Fast_ITEMS returns NULL for a real list, and include/molt/Python.h hard-#defines `PySequence_Fast_ITEMS(obj) ((PyObject**)NULL)` (numpy walking items gets NULL). Fast_GET_ITEM non-tuple branch returns OWNED ref (CPython borrows) -> leak; inverted PyList_Check vs PyTuple_Check discriminant; subtype mishandling. |
| `PyVectorcall_Function` | MISSING | high | scipy-cython (witness Tier-A A-CYTHON) | Not defined in src, shim, or either header. Named in NATIVE_DISCOVERY_FRONTIERS.md as one of the 14 Tier-A witness blockers gating scipy Cython (_ni_label, _nd_image). |
| `_PyObject_CallFunction_SizeT` | ALIAS_NEEDED | high | numpy/scipy compute-time (witness Tier-A A-SIZE) | ABI-identical PY_SSIZE_T_CLEAN alias of shim PyObject_CallFunction (pyarg_variadic.c:641); molt is already ssize-clean -> pure #[export_name]/alias. Pending A-SIZE lane. |
| `_PyObject_CallMethod_SizeT` | ALIAS_NEEDED | high | numpy/scipy compute-time (witness Tier-A A-SIZE) | ssize-clean alias of shim PyObject_CallMethod (pyarg_variadic.c:677); pending A-SIZE alias export. |
| `PyBuffer_FromContiguous` | MISSING | med | cython typed memoryviews | Not present. Stable-ABI copy-in counterpart of ToContiguous. |
| `PyBuffer_GetPointer` | MISSING | med | numpy/cython strided access | Not present. Stable-ABI helper computing a pointer into a strided/suboffset buffer. |
| `PyBuffer_ToContiguous` | MISSING | med | cython typed memoryviews | Not present in src/headers. CPython stable ABI. Cython memoryview copy-out uses it. |
| `PyObject_ClearWeakRefs` | STUB_THEATER | med | numpy/cython subtype dealloc | object.rs:704 effectively a no-op: reads tp_weaklistoffset but the ABI tier never CREATES weakrefs (PyWeakref_Check always 0) so the list head is always NULL and nothing is cleared/fired; a type with real weakref semantics gets none. TRACKED: POISON_ORPHAN_LEDGER #16 (fails honestly today; unreachable-defensive). |
| `PyObject_CopyData` | MISSING | med | cython/numpy buffer copies | Not present. Stable-ABI bulk buffer-to-buffer copy. |
| `PyObject_GetTypeData` | MISSING | med | PEP 697 variable-size extension types | 3.12 stable ABI. Not present. Backs negative-basicsize / extra-type-data. |
| `PyAIter_Check` | DIVERGENT | low | async extensions | Python.h:12163 header inline hardcodes `return 0` — never inspects tp_as_async->am_anext, so no async iterator is ever recognized. |
| `PyBuffer_SizeFromFormat` | MISSING | low | struct/memoryview format sizing | Not present. Stable-ABI itemsize-from-struct-format-string. |
| `PyMapping_GetOptionalItemString` | MISSING | low | newer extensions | 3.13 addition; PyMapping_GetOptionalItem (obj-key) IS implemented (abstract_mapping.rs:210) but the *String variant is absent. |
| `PyObject_DelItemString` | MISSING | low | generic C extensions | Stable-ABI real fn (Objects/abstract.c). Not exported/macro (only PyObject_DelItem exists). |
| `PyObject_GetItemData` | MISSING | low | PEP 697 var-size types | 3.12 stable ABI, not present. |
| `PyObject_LengthHint` | DIVERGENT | low | list/container preallocation | object.rs:1351 returns PyObject_Size or default; never consults __length_hint__, so generators/iterators exposing only __length_hint__ fall to default. Correctness-neutral (hint only) but a perf/semantics divergence. |
| `PySequence_DelSlice` | MISSING | low | generic C extensions | Stable-ABI. Python.h has GetSlice+SetSlice inlines but NO DelSlice. |
| `PyBuffer_Release` | FAITHFUL ✓verified | high | numpy/cython | buffer.rs:536 releases runtime-registered views AND calls foreign bf_releasebuffer before obj DECREF; balanced refcount, resets view. Verified against Objects/abstract.c; Miri-audited (MIRI-BUFFER-UB, ARRAY-BUFFER-EXPORT-INTERLOCK). Stale ledger row claiming 'never calls bf_releasebuffer' superseded. |
| `PyObject_GetAttr / GetAttrString / SetAttr / SetAttrString / HasAttr* / GenericGetAttr / GenericSetAttr / GenericGetDict / GenericSetDict / GetOptionalAttr*` | FAITHFUL (claimed) | high | numpy/cython | object.rs:52+ bridge object_get/set_attr + tp_getattro/tp_getattr + tp_setattro/tp_setattr fallback; data/non-data descriptor + instance-dict tiering in GenericGetAttr; honest AttributeError/TypeError; GetOptionalAttr propagates non-AttributeError (numpy __array__ coercion). (No adversarial verdict on record.) |
| `PyBuffer_FillInfo / PyBuffer_IsContiguous / PyObject_CheckBuffer / PyBuffer_FillContiguousStrides` | FAITHFUL (claimed) | med | numpy/cython | buffer.rs:620/597/567 FillInfo (writable/readonly + view==NULL BufferError), IsContiguous (suboffset=>never, len0=>always, C/F stride walk), CheckBuffer (side-effect-free bf_getbuffer test). FillContiguousStrides is a header inline. (NOTE: PyBuffer_IsContiguous edge divergences flagged under PyObject_GetBuffer DOWNGRADE.) |
| `PyIter_Check / PyIter_Next / PyObject_GetIter / PyObject_SelfIter` | FAITHFUL (claimed) | med | numpy/cython | object.rs:1791+ GetIter validates result is an iterator + index-based seqiter fallback for sq_item-only objects; PyIter_Next clears end-of-iteration StopIteration. |
| `PyMapping_Check/Size/GetItemString/GetOptionalItem/SetItemString/HasKey*/Keys/Values/Items + HasKeyWithError/StringWithError` | FAITHFUL (claimed) | med | cython | abstract_mapping.rs + object.rs:2392 real impls; *WithError (3.13) present, propagate exceptions. |
| `PyObject_IsTrue / PyObject_Not / PyObject_Size / PyObject_Length` | FAITHFUL (claimed) | med | numpy/cython | object.rs:1002/1276/1293 singleton + native-container-len fast paths, then foreign nb_bool->mp_length->sq_length dispatch (fixes empty-container-truthy + bool(np.array)); Size dispatches sq_length then mp_length with 'has no len()' TypeError. |
| `PyNumber_ToBase / PyIter_Send / PyNumber_InPlacePower / PyNumber_InPlaceMatrixMultiply / PyNumber_Matmul / PyObject_HashNotImplemented / PyObject_GetAIter` | FAITHFUL (claimed) | low | source-compiled witness extensions | Provided as include/molt/Python.h static-inline. CAVEAT: header inlines, NOT exported library symbols — correct for source-recompiled witness but a precompiled/stable-ABI consumer linking the symbol name fails to resolve. |
| `PyObject_DelAttr / PyObject_DelAttrString / PyMapping_DelItem / PySequence_ITEM / PyVectorcall_NARGS` | FAITHFUL (claimed) | low | source-compiled witness extensions | Header macros over exported primitives; faithful for source builds; macros not present for symbol-level linkers. |
| `PySequence_GetSlice / PySequence_SetSlice / PyMapping_DelItemString` | FAITHFUL (claimed) | low | source-compiled witness extensions | Python.h inlines (13458/13472/12319) composing PySlice_New + GetItem/SetItem/DelItem. Same header-inline-only caveat. |

### 2. type-slots — `PyType_*` / `tp_*` slot contract / metatype / heap types

*33 entries — 16 gap, 17 faithful.*

| symbol | status | sev | needed_by | note |
|---|---|:--:|---|---|
| `PyType_FromModuleAndSpec` | DIVERGENT | high | cython limited-API / numpy 2.x multiphase module state | typeobj.rs:1139 ignores the `_module` arg entirely, delegates to FromSpecWithBases — created heap type carries no module association; PyType_GetModule/GetModuleState can never return it (PEP 573 broken). |
| `PyType_FromSpec` | ALIAS_NEEDED | high | cython (type-specs/limited-API), general C extensions | The most-common stable-ABI type constructor (3.2). Trivially FromSpecWithBases(spec, NULL) which IS implemented, but no export AND not declared in include/Python.h (only FromSpecWithBases). A C ext calling PyType_FromSpec fails to compile/link. |
| `PyType_FromSpecWithBases` | FAITHFUL ✗→PARTIAL/DIVERGENT | high | cython/heap-type C extensions | typeobj.rs:1074 applies 81 spec slot ids, resolves base, installs alloc/new defaults, runs Ready. DOWNGRADE (fires every call): MISSING Py_TPFLAGS_HEAPTYPE (so instance-owns-type INCREF skipped -> use-after-free per molt's own memory.rs comment); bases/tp_base/tp_bases stored WITHOUT INCREF (dangles when caller decrefs — violates refcount contract); member-offset __dictoffset__/__weaklistoffset__/__vectorcalloffset__ special-casing absent; no best_base/BASETYPE validation; negative/relative basicsize (PEP 697) unimplemented; tp_dealloc not defaulted to subtype_dealloc; name/ht_name aliased not copied. |
| `PyType_GenericAlloc` | FAITHFUL ✗→PARTIAL/DIVERGENT | high | numpy (instance allocation) | typeobj.rs:690 -> molt_object_alloc(tp,nitems). Object-init half faithful (refcnt=1, ob_type, HEAPTYPE incref, ob_size, zero, OOM->NoMemory). DOWNGRADE: allocation-size half diverges — MISSING mandatory nitems+1 over-alloc (under-alloc every var-object by one itemsize -> heap overflow at index nitems); MISSING GC/managed-dict pre-header (HAVE_GC/MANAGED_DICT -> OOB before allocation); no GC track; no void* size round-up; Init/InitVar branch selects on nitems not tp_itemsize (ob_size divergence). Correct for fixed-size non-GC nitems==0; wrong for var-size + GC/managed-dict heap types. |
| `PyType_GetModule` | MISSING | high | PEP 573 module state (cython limited-API, numpy 2.x) | Not exported/declared. Cannot work until FromModuleAndSpec stores the module. Needed by ext methods fetching per-module state via defining_class. |
| `PyType_GetModuleState` | MISSING | high | PEP 573 module state (cython limited-API, numpy 2.x) | Not exported/declared. Pairs with GetModule; heavily used by modern Cython-generated module methods. |
| `PyType_GetSlot` | MISSING | high | cython / limited-API (PEP 384) extensions reading slots back | Not exported anywhere, not in any header. Read-back counterpart of apply_spec_slots; the ONLY way limited-API code retrieves tp_* from a spec-created type. All 81 slot fields are populated -> straightforward. |
| `PyType_Ready` | FAITHFUL ✗→PARTIAL/DIVERGENT | high | numpy/scipy static-type init | typeobj.rs:46 full pipeline: default tp_base, inherit_slots_from_base, tp_free/tp_alloc, build tp_dict from methods/members/getset, MRO, metatype, READY, bridge-register. DOWNGRADE: single-inheritance MRO only (no C3) — dual-inherit numpy scalars broken; tp_bases never set; no add_operators (dunder slot-wrappers absent from tp_dict); no add_subclasses; no DISALLOW_INSTANTIATION/tp_new default; no unhashable __hash__=None; inherit_slots ignores CPython pair/guard rules; doesn't ready an unready base; side-effect writes metatype tp_call. TRACKED: POISON_ORPHAN #2 (add_methods_to_dict fail-open method-drop). |
| `Py_<slot> PyType_Slot id constants (typeslots.h)` | DIVERGENT | high | numpy/scipy/cython spec tables (nb_*, sq_*, bf_getbuffer) | Runtime apply_spec_slots dispatches ALL 81 ids faithfully, BUT include/Python.h exposes only 12/81 slot-id macros. A C ext whose spec uses Py_nb_add/Py_sq_item/Py_bf_getbuffer/Py_tp_hash/Py_tp_init/Py_am_* won't compile. Header slot-id table must be completed to 81. |
| `PyType_CheckExact` | MISSING | med | general C extensions | Absent from header (only in comments). CPython macro Py_IS_TYPE(op,&PyType_Type); a C ext using it won't compile. Trivial macro add. |
| `PyType_FromMetaclass` | DIVERGENT | med | cython 3 limited-API custom-metaclass types | typeobj.rs:1148 ignores `_metaclass`, delegates to FromModuleAndSpec — result metatype is always PyType_Type instead of the requested metaclass. |
| `PyType_GetModuleByDef` | MISSING | med | cython multiphase modules | Not exported/declared. Walks MRO to find the type whose module matches a PyModuleDef; recovers module state from a subclass instance. |
| `PyObject_GetTypeData (PEP 697)` | MISSING | low | PEP 697 (3.12) opaque-type-data extensions | Not exported. Companion to PyType_GetTypeDataSize. (Also listed in object-protocol.) |
| `PySuper_Type` | MISSING | low | — | Stable-ABI data symbol for `super`. Not exported/declared. Rarely referenced directly by C extensions. |
| `PyType_ClearCache` | MISSING | low | — | Not exported. molt has no type method cache so a stub returning the version tag/0 is acceptable; currently absent entirely. |
| `PyType_GetTypeDataSize` | MISSING | low | PEP 697 (3.12) opaque-type-data extensions | Not exported. New 3.12 variable-size-extension API; rarely used by current numpy/scipy. |
| `PyBaseObject_Type` | FAITHFUL (claimed) | high | every C extension (object base) | abi_types.rs:1316 exported #[no_mangle] static; default tp_base for readied types. Declared Python.h:824. |
| `PyDescr_NewGetSet` | FAITHFUL (claimed) | high | numpy getset tables (PyArrayDescr_Type etc.) | typeobj.rs:1310 mints a real getset_descriptor backed by getset_get/set closures; consumed by Ready add_getset_to_dict. tp_getset slot contract. |
| `PyDescr_NewMember` | FAITHFUL (claimed) | high | numpy member tables | typeobj.rs:1342 mints a real member_descriptor backed by member_get/set + PyMember_GetOne/SetOne; consumed by Ready add_members_to_dict. tp_members slot contract. |
| `PyObject_TypeCheck` | FAITHFUL (claimed) | high | numpy (PyArray_DescrCheck / PyArray_Check) | typeobj.rs:1932 = exact-type match OR PyType_IsSubtype(Py_TYPE(op), tp). Declared in header. |
| `PyType_Check` | FAITHFUL (claimed) | high | numpy metaclass checks | typeobj.rs:1158 walks the metatype subtype chain (PyType_IsSubtype(meta,&PyType_Type)) so C metaclass instances pass, not just exact `type`. |
| `PyType_GenericNew` | FAITHFUL ✓verified | high | numpy (default tp_new) | typeobj.rs:701 dispatches the type's own tp_alloc(tp,0), fallback GenericAlloc — matches typeobject.c; HEAPTYPE-gated type INCREF present; OOM/overflow -> MemoryError. GC pre-header/track omitted by systemic no-GC design (self-consistent, no wrong observable result for classes that use GenericNew). Verified. (Separate broken inline PyType_GenericAlloc in include/molt/Python.h:13570 is NOT used by the abi tier.) |
| `PyType_GetFlags` | FAITHFUL ✓verified | high | numpy/cython feature tests | typeobj.rs:2064 returns tp_flags directly — identical to CPython `return type->tp_flags`; c_ulong width matches, benign NULL->0 guard. Verified. |
| `PyType_IsSubtype` | FAITHFUL ✓verified | high | numpy (isinstance / descriptor checks) | typeobj.rs:2029 walks full tp_mro when present (dual-inherit scalars), falls back to tp_base chain with object terminal — line-for-line port of type_is_subtype_base_chain. Verified against Objects/typeobject.c; borrowed refs preserved. |
| `PyType_Type` | FAITHFUL (claimed) | high | every C extension (metatype base) | abi_types.rs:1311 exported #[no_mangle] static; init_static_types sets tp_call=molt_type_call + tp_getattro=type_getattro. Declared Python.h:823. |
| `PyType_Type.tp_call (type_call metatype slot)` | FAITHFUL (claimed) | high | numpy DType class instantiation (BoolDType() etc.) | molt_type_call, typeobj.rs:546 verbatim CPython type_call: type(x) 1-arg case, 'takes 1 or 3 arguments', null-tp_new 'cannot create instances', tp_new->TypeCheck->tp_init, _Py_CheckFunctionResult fail-closed. Installed on PyType_Type + back-filled onto metatypes with null tp_call in Ready. |
| `Py_TYPE / Py_SET_TYPE / Py_IS_TYPE` | FAITHFUL (claimed) | high | every C extension | Header macros present (~Python.h:1670s). Core object->type access underpinning all type checks. |
| `PyObject_Type` | FAITHFUL (claimed) | med | C extensions calling type(obj) | typeobj.rs:1906 returns Py_TYPE(op) with an incref (new reference), matching CPython. |
| `PyType_GetName` | FAITHFUL (claimed) | med | numpy (dtype/type name reporting) | typeobj.rs:2072 correct _PyType_Name dotted-suffix stripping. CAVEAT: exported but NOT declared in include/Python.h -> C caller needs a manual prototype. |
| `PyType_HasFeature` | FAITHFUL (claimed) | med | numpy/cython feature gating | typeobj.rs:2195 (tp_flags & feature); declared Python.h:958. PyType_IS_GC builds on it. |
| `PyType_FastSubclass` | FAITHFUL (claimed) | low | — | Header macro (Python.h:1725) tp_flags & flag test; used by PyExceptionClass_Check / Py<T>_Check fast paths. |
| `PyType_GetQualName` | FAITHFUL (claimed) | low | — | typeobj.rs:2189 delegates to GetName; qualname==name correct for non-heap static types molt handles. |
| `PyType_Modified` | FAITHFUL (claimed) | low | — | typeobj.rs:1180 no-op — CORRECT: molt has no type attribute cache (_PyType_Lookup resolves fresh), nothing to invalidate. |

### 3. numbers — `PyLong_*` / `PyFloat_*` / `PyComplex_*` / `PyBool_*` / `PyOS_*` numeric

*88 entries — 52 gap, 36 faithful.*

| symbol | status | sev | needed_by | note |
|---|---|:--:|---|---|
| `PyFloat_Type` | DIVERGENT | high | numpy (references &PyFloat_Type) | abi_types.rs:727 same name+READY shell, no basicsize/flags/number-slots. |
| `PyLong_FromString` | MISSING | high | cython/numpy (witness A-CYTHON) | Not declared in abi authority nor implemented in numbers.rs. Exists ONLY as a divergent static inline in the to-be-deleted overlay include/molt/Python.h:7905 (routes through int(); sets *pend unconditionally -> trailing-garbage/partial-parse contract wrong). |
| `PyLong_Type` | DIVERGENT | high | numpy (references &PyLong_Type) | abi_types.rs:724 zeroed then patched only tp_name='int' + tp_flags=READY. Missing tp_basicsize, Py_TPFLAGS_LONG_SUBCLASS, tp_as_number slots, tp_hash/tp_richcompare, tp_base. numpy PyType_HasFeature(...,LONG_SUBCLASS) fast-paths + slot inheritance see a shell. |
| `PyUnstable_Long_CompactValue` | MISSING | high | cython 3.x (witness A-CYTHON) | 3.12 fast-int accessor absent; pairs with IsCompact. |
| `PyUnstable_Long_IsCompact` | MISSING | high | cython 3.x (witness A-CYTHON) | 3.12 fast-int accessor absent. Cython 3.x GENERATES calls to IsCompact/CompactValue for fast int unboxing on 3.12; every cython-compiled ext fails to link/compile without it. |
| `PyBool_Type` | DIVERGENT | med | numpy/cython | abi_types.rs:763 name+READY shell; tp_base is NOT set to &PyLong_Type, so a foreign subtype walk bool->int would fail (masked today because bool objects are bridge-resolved). |
| `PyComplex_Type` | DIVERGENT | med | numpy/cmath | abi_types.rs:730 name+READY+tp_dealloc but no basicsize/number-slots/richcompare. |
| `PyFloat_GetInfo` | MISSING | med | sys.float_info / numpy | Absent from abi authority; only overlay:13681 static inline builds the structseq. |
| `PyFloat_Pack2` | MISSING | med | numpy float16 / struct | Half-float pack absent; numpy float16 and struct 'e' depend on Pack2/Unpack2. |
| `PyFloat_Pack4` | MISSING | med | struct/numpy | Absent; struct 'f'. |
| `PyFloat_Pack8` | MISSING | med | struct/numpy | Absent; struct 'd'. |
| `PyFloat_Unpack2` | MISSING | med | numpy float16 / struct | Half-float unpack absent. |
| `PyFloat_Unpack4` | MISSING | med | struct/numpy | Absent. |
| `PyFloat_Unpack8` | MISSING | med | struct/numpy | Absent. |
| `PyLong_AsSize_t` | MISSING | med | numpy (sizes) | Absent from abi authority; overlay:7976 static-inline routes through PyLong_AsLongLong so (2^63,2^64) overflows instead of converting. |
| `PyLong_AsUnsignedLongLongMask` | MISSING | med | cython/hashing | Absent from abi authority; overlay:8002 uses AsLongLong then clears error for bignums (returns 0 on any pending error) rather than masking the true low 64 bits. |
| `PyLong_AsUnsignedLongMask` | MISSING | med | cython/hashing | Absent from abi authority; overlay:13701 delegates to LongLongMask which raises for bignums instead of returning the low-bits mask CPython guarantees. |
| `PyLong_CheckExact` | DIVERGENT | med | cython/numpy fast-path dispatch | Header macro (Python.h:1089) #defines CheckExact -> PyLong_Check, so CheckExact(True)==1 and a foreign int subclass passes; CPython requires exact int identity. |
| `PyLong_FromSsize_t` | DIVERGENT | med | numpy (shapes) / Windows gating | numbers.rs:513 does PyLong_FromLong(v as c_long): truncates >2^31 on LLP64 Windows-x64 (M02 gated target); correct on wasm32/LP64. Should route py_long_from_i64(v as i64). |
| `PyLong_GetInfo` | MISSING | med | sys/introspection | No sys.int_info structseq provider in abi crate. |
| `_PyLong_AsByteArray` | DIVERGENT | med | pickle/struct/cython | numbers.rs:1123 serializes correctly within +/-2^64 (i128 widen) but a genuine >64-bit bignum raises OverflowError instead of writing the true bytes; bignum path unimplemented. |
| `_PyLong_FromByteArray` | DIVERGENT | med | pickle/marshal/int.from_bytes-C consumers | numbers.rs:666 reads only n.min(8) bytes: SILENTLY truncates arrays >8 bytes to 64 bits (a 16-byte value returns a wrong int). CPython builds an arbitrary-precision int. |
| `_PyLong_NumBits` | MISSING | med | marshal/pickle | Bit-length query absent; used by marshal/pickle and int.bit_length-adjacent C. |
| `_PyLong_Sign` | MISSING | med | cython/numpy | Sign query absent; used by numpy/cython and marshal. |
| `_PyLong_Size_t_Converter` | MISSING | med | stdlib-C/argument-clinic | Argument-clinic converter absent. |
| `_PyLong_UnsignedInt_Converter` | MISSING | med | stdlib-C/argument-clinic | Argument-clinic converter absent. |
| `_PyLong_UnsignedLongLong_Converter` | MISSING | med | stdlib-C/argument-clinic | Argument-clinic converter absent. |
| `_PyLong_UnsignedLong_Converter` | MISSING | med | stdlib-C/argument-clinic | Argument-clinic converter absent. |
| `_PyLong_UnsignedShort_Converter` | MISSING | med | stdlib-C/argument-clinic | Argument-clinic converter absent; many stdlib-C fns compiled through the ABI reference it. |
| `_Py_FalseStruct` | ALIAS_NEEDED | med | stable-ABI / third-party C | As _Py_TrueStruct: alias _Py_FalseStruct -> Py_False. |
| `_Py_TrueStruct` | ALIAS_NEEDED | med | stable-ABI / third-party C | CPython's canonical exported bool datum is _Py_TrueStruct (PyLongObject), Py_True==((PyObject*)&_Py_TrueStruct). Molt exports Py_True directly; consumers referencing _Py_TrueStruct won't resolve. Alias _Py_TrueStruct -> Py_True. |
| `_Py_c_abs` | MISSING | med | cython complex / cmath / abs(complex) | Absent (hypot-based abs). |
| `_Py_c_diff` | MISSING | med | cython complex / cmath | Absent. |
| `_Py_c_pow` | MISSING | med | cython complex / cmath | Absent. |
| `_Py_c_prod` | MISSING | med | cython complex / cmath | Absent. |
| `_Py_c_quot` | MISSING | med | cython complex / cmath | Absent (division-by-zero/inf edge semantics). |
| `_Py_c_sum` | MISSING | med | cython complex / cmath | Complex arithmetic primitive absent; cython-generated complex + cmath link _Py_c_*. |
| `PyComplex_AsCComplex` | DIVERGENT | low | cmath/cython complex | numbers.rs:1442 real-complex read + unaligned wasm32 read + PyFloat_AsDouble fallback. Residual: does NOT run the __complex__ special-method probe CPython tries first, so an object defining __complex__ but not __float__/__index__ fails. |
| `PyComplex_CheckExact` | DIVERGENT | low | fast-path dispatch | Header macro Python.h:1107 #defines CheckExact -> PyComplex_Check (subtype-walking), so a complex subclass wrongly passes CheckExact. |
| `PyFloat_GetMax` | MISSING | low | sys.float_info consumers | Absent from abi authority; only overlay:11788 static inline. |
| `PyFloat_GetMin` | MISSING | low | sys.float_info consumers | Absent from abi authority; only overlay:11792. |
| `PyLong_AS_LONG` | MISSING | low | rare C fast-paths | Unsafe direct-access macro absent from abi header. |
| `PyLong_FromPid / PyLong_AsPid` | MISSING | low | os/posix modules | pid_t platform macros absent. |
| `PyOS_string_to_double` | DIVERGENT | low | numpy/float parsing | C shim pyarg_variadic.c:933 uses libc strtod: unlike _Py_dg_strtod accepts leading whitespace, locale-dependent decimal point, C99 hex floats; sets overflow_exception only on ERANGE. Adequate for numpy's typical calls. |
| `_PyLong_DivmodNear` | MISSING | low | builtins round | round(int) helper absent. |
| `_PyLong_Format` | MISSING | low | repr paths | Base-format helper absent. |
| `_PyLong_Frexp` | MISSING | low | math/float(int) | Bignum frexp absent. |
| `_PyLong_FromBytes` | MISSING | low | cython | String/bytes->int helper absent (distinct from _PyLong_FromByteArray). |
| `_PyLong_GCD` | MISSING | low | math | math.gcd C helper absent. |
| `_PyLong_Lshift` | MISSING | low | cython | Absent. |
| `_PyLong_Rshift` | MISSING | low | cython | Absent. |
| `_Py_c_neg` | MISSING | low | cython complex | Absent. |
| `PyFloat_AsDouble` | FAITHFUL (claimed) | high | numpy (witness Tier-A) | numbers.rs:1247 exact float, nb_float then nb_index fallback, bignum via authority, TypeError 'must be real number' with -1.0 (no silent NaN); ledger fake-NaN fixed. |
| `PyFloat_Check` | FAITHFUL (claimed) | high | numpy | numbers.rs:1587 walks foreign float subtypes (numpy.float64); ledger [M] subtype fixed. |
| `PyFloat_FromDouble` | FAITHFUL (claimed) | high | numpy/core-correctness | numbers.rs:1200 MoltObject::from_float; faithful. |
| `PyLong_AsDouble` | FAITHFUL (claimed) | high | numpy | numbers.rs:745 exact for i64/u64, bignum via v/1 TrueDivide raising past f64; TypeError non-int. |
| `PyLong_AsInt` | FAITHFUL (claimed) | high | cython (3.13 API, provided) | numbers.rs:1102 -> _PyLong_AsInt via AsLongAndOverflow; platform-independent i64 range test; ledger silent-(-1) fixed. |
| `PyLong_AsLong` | FAITHFUL ✓verified | high | cython/numpy/core-correctness | numbers.rs:713 checked c_long::try_from (platform width), __index__ dispatch via PyNumber_Index, OverflowError exact msg, -1-only-with-exc. Verified vs longobject.c; silent-trunc (#8) fixed. |
| `PyLong_AsLongAndOverflow` | FAITHFUL (claimed) | high | cython | numbers.rs:776 correct *overflow=+/-1 no-exc contract. MINOR: beyond +/-2^64 OutOf64 assumes positive (ov=1) so a <-2^64 bignum reports wrong sign. |
| `PyLong_AsLongLong` | FAITHFUL (claimed) | high | numpy/cython | numbers.rs:850 checked + OverflowError; ledger silent-trunc fixed. |
| `PyLong_AsSsize_t` | FAITHFUL (claimed) | high | numpy (shape/stride/index) / witness A-SIZE | numbers.rs:818 checked isize::try_from + OverflowError; py_long_as_ssize_clamped err==NULL clamp path. Ledger #7 silent-trunc fixed. |
| `PyLong_AsUnsignedLong` | FAITHFUL (claimed) | high | numpy | numbers.rs:900 negatives/non-int raise, width overflow raises, (unsigned long)-1 sentinel; ledger silent-wrap fixed. |
| `PyLong_AsUnsignedLongLong` | FAITHFUL (claimed) | high | numpy | numbers.rs:935 strict; ledger [M] negative-wrap fixed. |
| `PyLong_Check` | FAITHFUL (claimed) | high | everywhere/core-correctness | numbers.rs:1559 true for int, bool (subtype), heap bignum, foreign int subclasses via PyType_IsSubtype. |
| `PyLong_FromDouble` | FAITHFUL ✓verified | high | numpy (int(np.float64)) | numbers.rs:546 exact bignum (mantissa<<shift) for \|v\|>=2^63 via runtime authority; NaN->ValueError, inf->OverflowError; truncate toward zero. Verified vs longobject.c incl 1e300/largest-double. (Isolated crate w/ stub hooks fails LOUD, documented; not shipping config.) |
| `PyLong_FromLong` | FAITHFUL ✓verified | high | numpy/cython/core-correctness | numbers.rs:507 -> py_long_from_i64; inline-int fast path + bignum hook fallback; full range, no 47-bit trunc. Verified vs 3.12; only OOM/no-hooks NULL path omits MemoryError (minor). |
| `PyLong_FromLongLong` | FAITHFUL ✓verified | high | numpy/cython | numbers.rs:523; lossless i64 widening, full range incl LLONG_MIN via int_bits_from_i128, no truncation. Verified; only OOM NULL omits MemoryError. |
| `PyLong_FromSize_t` | FAITHFUL ✓verified | high | numpy (shapes/sizes) | numbers.rs:518 -> py_long_from_u64; exact over full size_t incl the >2^63 sign-danger zone; correct 0 handling; new-ref. Verified. |
| `PyLong_FromUnsignedLongLong` | FAITHFUL ✓verified | high | numpy/cython | numbers.rs:535; u64->i128 lossless, u64::MAX -> exact BigInt (no sign flip); new-ref contract. Verified. |
| `Py_True / Py_False` | FAITHFUL (claimed) | high | everywhere/core-correctness | abi_types.rs:692/698 exported immortal PyObject data, ob_type=&PyBool_Type after init; header maps Py_True/False/IsTrue/IsFalse/RETURN_TRUE/FALSE correctly. |
| `_PyLong_AsInt` | FAITHFUL (claimed) | high | cython/stdlib-C | numbers.rs:1085 TypeError non-int, OverflowError out-of-int. |
| `PyBool_Check` | FAITHFUL (claimed) | med | cython | numbers.rs:1603 (is_bool); bool is non-subclassable so exact test suffices. |
| `PyBool_FromLong` | FAITHFUL (claimed) | med | numpy/cython/core-correctness | numbers.rs:1528 returns &Py_True/&Py_False by truthiness. |
| `PyComplex_Check` | FAITHFUL (claimed) | med | numpy/cmath | numbers.rs:1512 PyObject_TypeCheck subtype walk; ledger [L] exact-identity fixed. |
| `PyComplex_FromCComplex` | FAITHFUL (claimed) | med | cython complex | numbers.rs:1423 -> FromDoubles. |
| `PyComplex_FromDoubles` | FAITHFUL (claimed) | med | cmath/numpy complex | numbers.rs:1411 boxes a real PyComplexObject. |
| `PyFloat_CheckExact` | FAITHFUL (claimed) | med | numpy fast-path | Header macro Python.h:1098 = Py_IS_TYPE(op,&PyFloat_Type): correct exact-identity (unlike Long/Complex CheckExact aliases). |
| `PyFloat_FromString` | FAITHFUL (claimed) | med | float()/numpy | numbers.rs:1206 real parse w/ inf/nan/underscore + CPython error shapes. |
| `PyLong_AsLongLongAndOverflow` | FAITHFUL (claimed) | med | cython | numbers.rs:870 correct no-exc overflow (same positive-assumption caveat beyond +/-2^64). |
| `PyLong_AsVoidPtr` | FAITHFUL (claimed) | med | ctypes | numbers.rs:960 sets TypeError/OverflowError before NULL; ledger [L] silent-NULL fixed. |
| `PyLong_FromUnicodeObject` | FAITHFUL (claimed) | med | int()/numpy | numbers.rs:619 folds arbitrary-precision literals via build_big_int_from_literal past 64-bit; ledger overflow-cap divergence fixed. |
| `PyLong_FromUnsignedLong` | FAITHFUL (claimed) | med | numpy/cython | numbers.rs:529 -> py_long_from_u64; faithful. |
| `PyLong_FromVoidPtr` | FAITHFUL (claimed) | med | ctypes/capsule | numbers.rs:541; faithful. |
| `_Py_HashDouble` | FAITHFUL (claimed) | med | set/dict of floats (core-correctness) | numbers.rs:1359 full frexp-based CPython hash incl inf/nan/pointer-hash-for-nan. |
| `PyComplex_ImagAsDouble` | FAITHFUL (claimed) | low | cython complex | numbers.rs:1496 returns 0.0 for non-complex WITHOUT setting error; ledger [M] live-TypeError fixed. |
| `PyComplex_RealAsDouble` | FAITHFUL (claimed) | low | cython complex | numbers.rs:1478 real part or PyFloat_AsDouble; unaligned-read safe. |
| `PyOS_strtol` | FAITHFUL (claimed) | low | stdlib-C/cython | C shim pyarg_variadic.c:954 wraps libc strtol; force-linked. |
| `PyOS_strtoul` | FAITHFUL (claimed) | low | stdlib-C/cython | C shim pyarg_variadic.c:965 wraps libc strtoul; force-linked. |

### 4. containers — dict / list / tuple / set / frozenset (public + Stable-ABI)

*30 entries — 27 gap, 3 faithful.*

| symbol | status | sev | needed_by | note |
|---|---|:--:|---|---|
| `PyDict_Clear` | MISSING | high | numpy/cython module dict reset & teardown; core dict correctness | Stable ABI (3.2). Absent from mapping.rs (only a u64 test helper in molt-runtime cpython_compat.rs, NOT an ABI export). Runtime clear logic exists; thin PyObject* wrapper needed. |
| `PyDict_New / GetItem / GetItemWithError / GetItemString / SetItem / SetItemString / DelItem / DelItemString / Next / Keys / Values / Items / Size / Copy / Contains / Merge / SetDefault / PyDictProxy_New` | FAITHFUL ✗→PARTIAL/DIVERGENT | high | numpy _multiarray_umath init; core | mapping.rs CPython-faithful for the core: borrowed-ref GetItem, error-propagating WithError, BadInternalCall on non-dict, KeyError on DelItem miss, O(1) PyDict_Next cursor, foreign key/value custody, non-stealing SetItem. DOWNGRADE: PyDictProxy_New is a HOLLOW STUB (no arg validation, forwards NOTHING — proxy[key]/len/in/iter all fail; PyDictProxy_Type is zeroed w/ only tp_name); PyDict_Merge override==2 raises instead of overwriting (public symbol should collapse to bool); PyDict_SetItem/GetItem skip PyDict_Check (non-dict silently accepted); unhashable-key Contains/GetItemWithError return 0/NULL with no exception. |
| `PyDict_Type / PyDictProxy_Type / PyList_Type / PyTuple_Type / PySet_Type / PyFrozenSet_Type` | FAITHFUL ✗→PARTIAL/DIVERGENT | high | numpy PyType_Ready / isinstance / tp identity; core | abi_types.rs 6 core container type objects defined, named, given tp_dealloc/tp_basicsize, registered. DOWNGRADE: std::mem::zeroed() sentinels — only tp_name+READY set. CheckExact aliased to subtype-inclusive *_Check (subclass wrongly passes); no BASETYPE / DICT/LIST/TUPLE_SUBCLASS / HAVE_GC flags; tp_basicsize=0 (C subtype inherits size 0 -> unsafe alloc); all method slots null (tp_hash/richcompare/iter/as_mapping/as_sequence/as_number null -> null-deref if read). |
| `PyDict_Update` | MISSING | high | numpy/cython namespace/__dict__ population; core | Stable ABI (3.2). Only a u64 helper (cpython_compat.rs:1933); no PyObject* ABI export. PyDict_Merge(override=1) already implements the logic — wrapper trivial. |
| `PyFrozenSet_New` | MISSING | high | numpy dtype/immutable-set construction; core set completeness | Stable ABI (3.2). Only u64 helper cpython_compat.rs:1563; no ABI export. Runtime molt_frozenset_new + set hooks exist; wrap like PySet_New. |
| `PyList_New / Append / GetItem / GetItemRef / SetItem / Insert / GetSlice / SetSlice / Sort / Reverse / AsTuple / Size / GET_ITEM / SET_ITEM / GET_SIZE` | FAITHFUL ✗→PARTIAL/DIVERGENT | high | numpy/cython list construction & iteration; core | sequences.rs faithful core: pre-sized None slots, reference-stealing SetItem w/ Py_XDECREF on error, IndexError/BadInternalCall, real in-place Sort/Reverse/SetSlice/Insert (no longer no-op stubs). DOWNGRADE: PyList_Append skips type check + DISCARDS hook result -> silent success on non-list AND on alloc-fail; GetSlice/AsTuple silent NULL (no BadInternalCall) on null/non-list; macros PyList_GET/SET_ITEM/GET_SIZE remap to CHECKED fns (SET_ITEM must be an unchecked void raw store — breaks New(n)+SET_ITEM fill idiom); New fills None not NULL; SetItem rejects v==NULL (CPython stores NULL) + leaks replaced occupant (no __del__); SetSlice requires bridge ptr not any iterable. |
| `PySet_New / Add / Contains / Discard / Size / Check / PyFrozenSet_Check` | FAITHFUL ✗→PARTIAL/DIVERGENT | high | set-using extensions; core | sequences.rs routes through runtime set hooks; ensure_set_error guarantees an exception on every -1/NULL; ABI-layer Check now subtype-aware. DOWNGRADE: PySet_Add on a shared frozenset MUTATES unconditionally (runtime accepts TYPE_ID_FROZENSET with NO Py_REFCNT==1 guard) -> silently corrupts an immutable frozenset's cached hash where CPython raises SystemError; runtime mutation/query gate on EXACT type_id so set/frozenset SUBCLASS instances are rejected (CPython accepts). |
| `PyTuple_New / GetItem / GET_ITEM / SetItem / GetSlice / Size / GET_SIZE` | FAITHFUL ✗→PARTIAL/DIVERGENT | high | numpy return-tuple building; Py_BuildValue; core | sequences.rs dual-path (native ABI-layout PyTupleObject w/ real ob_item + bridge Molt tuple); reference-stealing SetItem w/ bounds gate; tuple dealloc XDECREFs slots. DOWNGRADE: PyTuple_Size no type check + never returns -1 (non-tuple silently wrong length); SetItem omits refcnt!=1 guard (mutates shared tuple) + rejects v==NULL (CPython accepts); New(neg) clamps to 0 (CPython BadInternalCall) + fresh empty tuple not the shared singleton; GetSlice no PyTuple_Check + no same-object full-slice; tuple subclasses miss the fast path. |
| `PyTuple_Pack` | FAITHFUL ✗→PARTIAL/DIVERGENT | high | numpy/Py_BuildValue variadic paths | Variadic in C shim pyarg_variadic.c:144, exported via whole-archive anchor. DOWNGRADE: silent 64-arg cap (MOLT_VARARG_MAX_ARGS) — n>64 returns NULL with NO exception (functional wrong-answer for legit input); n==0 mints fresh heap tuple not the shared singleton; all failure paths return bare NULL WITHOUT setting an exception (CPython sets BadInternalCall for negative n). Core steal contract correct. |
| `_PyList_Extend` | MISSING | high | scipy Cython (_ni_label, _nd_image); witness Tier-A A-CYTHON (one of the 14) | cpython/listobject.h private. NOT exported (no no_mangle/export_name). Gates the entire Cython-3 class per NATIVE_DISCOVERY_FRONTIERS.md Lane A-CYTHON. |
| `PyDictKeys_Type / PyDictValues_Type / PyDictItems_Type / PyDictIterKey_Type / PyDictIterValue_Type / PyDictIterItem_Type / PyDictRevIterKey_Type / PyDictRevIterValue_Type / PyDictRevIterItem_Type` | MISSING | med | stable-ABI data symbols; isinstance/type-identity on dict views | 9 Stable-ABI (3.2/3.8) type-object DATA symbols. abi_types.rs defines PyDict_Type/PyDictProxy_Type but none of the view/iterator types. |
| `PyDict_MergeFromSeq2` | MISSING | med | dict(seq-of-pairs); some numpy/cython init | Stable ABI (3.2). Not present. dict_merge_from_mapping (mapping.rs:261) is the sibling; a Seq2 variant must be added. |
| `PySet_Clear` | MISSING | med | stable-ABI set completeness | Stable ABI (3.2). Only u64 helper cpython_compat.rs:1734; no ABI export. Trivial wrapper. |
| `PySet_Pop` | MISSING | med | set-consuming extensions; stable-ABI completeness | Stable ABI (3.2). Only u64 helper cpython_compat.rs:1720; no PyObject* export. Runtime set_pop exists. |
| `_PyDict_GetItemWithError` | MISSING | med | numpy private lookups | cpython/dictobject.h private (distinct from public PyDict_GetItemWithError). Not exported under the underscore name — ALIAS delegation suffices. |
| `_PyDict_Next` | MISSING | med | Cython fast dict iteration (5-arg form returning hash) | cpython/dictobject.h private. Public PyDict_Next (mapping.rs:622) faithful; the hash-returning private form is absent. |
| `_PyDict_SetItem_KnownHash` | MISSING | med | numpy/cython pre-hashed insert fast path | cpython/dictobject.h private. Absent. PyDict_SetItem faithful; KnownHash variant would ignore hash and delegate. |
| `_PySet_NextEntry` | MISSING | med | Cython set iteration; scipy Cython modules | cpython/setobject.h private. Not exported. (3.12 exposes _PySet_NextEntry.) Needs a set-cursor hook analogous to dict_entry. |
| `_PyTuple_Resize` | MISSING | med | numpy incremental tuple building (PyArray tuple construction) | cpython/tupleobject.h private. Not exported. PyTuple_New uses a boxed slot array (sequences.rs:379) — resize = realloc slice + fix ob_size. |
| `PyDict_AddWatcher / PyDict_ClearWatcher / PyDict_Watch / PyDict_Unwatch` | MISSING | low | 3.12 dict-watcher introspection (specializing interpreters/profilers) | New public API in 3.12. 4 symbols, none exported. Not used by numpy/scipy witness; needed for full 3.12 public surface. |
| `PyListIter_Type / PyListRevIter_Type / PyTupleIter_Type / PySetIter_Type` | MISSING | low | stable-ABI data symbols; iterator type identity | 4 Stable-ABI (3.2) type-object DATA symbols. Not defined in abi_types.rs. |
| `_PyDict_Contains_KnownHash` | MISSING | low | perf-hashed membership (cython) | cpython/dictobject.h private; delegate to PyDict_Contains. |
| `_PyDict_DelItem_KnownHash` | MISSING | low | cython | cpython/dictobject.h private; delegate to PyDict_DelItem. |
| `_PyDict_MergeEx` | MISSING | low | cython | cpython/dictobject.h private (backs PyDict_Merge/Update). Logic exists (mapping.rs:214) but not exported under this private name. |
| `_PyDict_Pop` | MISSING | low | cython | cpython/dictobject.h private (dict.pop C fast path). Absent. |
| `_PyDict_SetItemId / DelItemId / GetItemIdWithError / ContainsId / MaybeUntrack / HasOnlyStringKeys / SizeOf / DelItemIf / DebugMallocStats / _PyDictView_New / _PyDictView_Intersect` | MISSING | low | niche CPython-internal | 11 cpython/dictobject.h Identifier/GC/debug privates. None exported. Id-based use _Py_Identifier which molt does not model. |
| `_PyTuple_MaybeUntrack / _PyTuple_DebugMallocStats / _PyList_DebugMallocStats` | MISSING | low | internal-only | cpython/ header GC/debug privates; internal-only, not part of extension linking in practice. |
| `_PyDict_GetItemStringWithError` | FAITHFUL (claimed) | med | numpy (links this private error-propagating form) | mapping.rs:464 builds str key, routes through PyDict_GetItemWithError; NULL key -> BadInternalCall. Faithful, unit-tested. LANDED via DISCOVERY-FRONTIER-FIXES. |
| `_PyDict_GetItem_KnownHash` | FAITHFUL (claimed) | low | cython | mapping.rs:433 hash arg ignored, delegates to PyDict_GetItem (borrowed ref). Correct result since caller hash is only an optimization. |
| `_PyDict_NewPresized` | FAITHFUL (claimed) | low | numpy presized-dict init | mapping.rs:54 aliases PyDict_New; the minused hint is ignored (preallocation perf-only, semantics correct). Behaviorally faithful. |

### 5. strings-bytes — `PyUnicode_*` / `PyBytes_*` / `PyByteArray_*` / `PyCodec_*`

*158 entries — 108 gap, 50 faithful.*

| symbol | status | sev | needed_by | note |
|---|---|:--:|---|---|
| `PyBytes_FromObject` | MISSING | high | numpy/cython (coerce buffer/bytes-like -> bytes) | Not exported anywhere in runtime/. |
| `PyUnicode_AsUTF8` | PARTIAL (verified) | high | cython/numpy (ubiquitous C-string access) | L6 fixed the memory-safety contract: delegates to AsUTF8AndSize, returns one object-owned stable cache with `data[len] == 0`, and releases the cache with the bridge object. Mask proof reads the terminator and verifies pointer stability. Remaining divergence: Molt cannot encode lone-surrogate Unicode values faithfully yet. |
| `PyUnicode_Check` | FAITHFUL (verified) | high | cython/numpy | Uses CPython's `Py_TPFLAGS_UNICODE_SUBCLASS` fast-subclass contract, so str-subclass instances return true while CheckExact remains exact identity. Mask proof covers a synthetic subtype. |
| `PyUnicode_FromFormat` | DIVERGENT | high | cython/numpy (A-CYTHON: error messages, __repr__, exception text) | Real C impl in shims/pyarg_variadic.c but supports only %s %S %R %d %i %% (+ l/ll/z mods). Any %c %u %x %X %p %U %A %V goto-errors -> NULL. Width/precision PARSED then IGNORED (no %.200s truncation). Cython/numpy use %U/%A/%c/%x heavily -> NULL/wrong text. |
| `PyUnicode_FromFormatV` | DIVERGENT | high | cython/numpy | Same shim engine; same format-code subset gap and ignored width/precision. |
| `PyUnicode_FromString` | PARTIAL (verified) | high | cython/numpy/witness | L6 now validates strict UTF-8 before allocation and raises UnicodeDecodeError+NULL for malformed input; OOM remains fail-closed. Remaining identity divergence: runtime allocation may intern identifiers/one-character strings whereas CPython FromString does not promise interning. |
| `PyUnicode_FromStringAndSize` | FAITHFUL (verified) | high | cython/numpy/witness | Strict UTF-8 validation precedes allocation; malformed input raises UnicodeDecodeError+NULL. Negative size raises SystemError, NULL+positive size raises SystemError, and NULL+zero size constructs the empty string. Mask proof verifies invalid UTF-8 never reaches alloc_str. |
| `PyUnicode_RichCompare` | MISSING | high | cython/core-correctness (str == / < in C) | Only PyUnicode_Compare (raw -1/0/1) + CompareWithASCIIString exist; the object-returning rich compare used by Cython comparison lowering is absent. |
| `PyUnicode_Type` | FAITHFUL ✗→PARTIAL/DIVERGENT | high | numpy/cython (identity checks, PyDict registry keys) | Exported #[no_mangle] static, tp_name='str'. DOWNGRADE: std::mem::zeroed() with only tp_name+READY. CPython populates ~20 slots + 4 flags. PyUnicode_Check is exact-identity (subtypes broken); no UNICODE_SUBCLASS/BASETYPE; all behavioral slots (tp_hash/richcompare/str/nb_remainder/subscript/iter) NULL -> null-deref if dispatched; ob_refcnt/metatype left 0/NULL. Identity token, not a faithful type object. |
| `_PyBytes_Resize` | MISSING | high | numpy/cython (incremental bytes buffer growth then shrink) | Documented public resize helper; absent. Extensions that FromStringAndSize(NULL,n) then _PyBytes_Resize cannot build bytes. |
| `_PyUnicode_FastCopyCharacters` | MISSING | high | cython/numpy (str building; witness A-CYTHON adjacent) | Private-but-linked helper; numpy/cython link it. Absent. No CopyCharacters/WriteChar companion either, so allocate-then-fill str construction is unsupported. |
| `PyByteArray_Concat` | MISSING | med | cython (bytearray building) | Absent. |
| `PyByteArray_FromObject` | MISSING | med | cython (bytes-like -> bytearray) | Absent; cannot construct a bytearray from an arbitrary buffer object. |
| `PyByteArray_Resize` | MISSING | med | cython/numpy (mutable buffer grow/shrink) | Absent; native PyByteArrayObject has ob_alloc but no resize entry point, so incremental bytearray building via C API is impossible. |
| `PyBytes_CheckExact` | FAITHFUL (verified) | med | cython/numpy (exact-type guard) | Inline exact-identity macro is present in both header homes; unlike PyBytes_Check it rejects subtype instances. |
| `PyBytes_ConcatAndDel` | MISSING | med | cython (bytes building) | Absent; PyBytes_Concat present but the AndDel variant that decrefs newpart is not exported. |
| `PyBytes_FromFormat` | MISSING | med | numpy/cython (bytes error/message building) | Absent (no C shim analog to PyUnicode_FromFormat). |
| `PyBytes_FromFormatV` | MISSING | med | numpy/cython | Absent. |
| `PyBytes_Repr` | MISSING | med | numpy/cython (bytes __repr__) | Stable-ABI; absent. |
| `PyUnicode_Append` | MISSING | med | cython (in-place str concat) | Absent; only PyUnicode_Concat (new object). |
| `PyUnicode_AppendAndDel` | MISSING | med | cython | Absent. |
| `PyUnicode_AsEncodedString` | DIVERGENT | med | codecs consumers | Encodes only utf8/ascii/latin1; other codecs -> LookupError (fail-loud). errors= ignored. |
| `PyUnicode_AsUTF16String` | MISSING | med | codecs/Windows | Absent (DecodeUTF16 present, encode side missing). |
| `PyUnicode_AsUTF32String` | MISSING | med | numpy 'U' dtype / codecs | Absent. |
| `PyUnicode_AsUnicodeEscapeString` | MISSING | med | cython/repr | Absent. |
| `PyUnicode_AsWideChar` | MISSING | med | Windows/wchar interop | Absent. |
| `PyUnicode_AsWideCharString` | MISSING | med | Windows/wchar interop | Absent. |
| `PyUnicode_CheckExact` | FAITHFUL (verified) | med | cython/numpy (exact-type guard) | Inline exact-identity macro is present in both header homes and is intentionally distinct from subtype-aware PyUnicode_Check. |
| `PyUnicode_CopyCharacters` | MISSING | med | cython/numpy (str building) | Public counterpart of _PyUnicode_FastCopyCharacters; absent. |
| `PyUnicode_Count` | MISSING | med | text ext | Absent. |
| `PyUnicode_Decode` | DIVERGENT | med | codecs consumers | Dispatches only utf8/ascii/latin1/utf16; every other encoding (utf-32, cp1252, unicode_escape...) fails-loud with LookupError. Honest (no silent wrong answer) but a large codec subset unimplemented. |
| `PyUnicode_DecodeFSDefault` | MISSING | med | filesystem paths | Absent. |
| `PyUnicode_DecodeFSDefaultAndSize` | MISSING | med | filesystem paths | Absent. |
| `PyUnicode_DecodeUTF32` | MISSING | med | codecs/numpy string dtypes | Absent; UCS4 string interchange path missing. |
| `PyUnicode_DecodeUTF8Stateful` | MISSING | med | io/codecs (incremental decode) | Absent; only non-stateful DecodeUTF8. |
| `PyUnicode_DecodeUnicodeEscape` | MISSING | med | cython (source-literal/repr parsing) | Absent. |
| `PyUnicode_EncodeFSDefault` | MISSING | med | filesystem paths | Absent. |
| `PyUnicode_FSConverter` | MISSING | med | numpy/scipy (file path args via 'O&') | Absent; path-accepting ext fns (np.load/save, scipy io) cannot use the O& converter. |
| `PyUnicode_FSDecoder` | MISSING | med | numpy/scipy (path decode) | Absent. |
| `PyUnicode_Fill` | MISSING | med | cython/numpy | Absent; no way to bulk-fill a PyUnicode_New buffer. |
| `PyUnicode_Find` | MISSING | med | cython (substring index) | Only FindChar (single code point) exists; substring Find absent. |
| `PyUnicode_FromObject` | MISSING | med | cython/numpy (str/str-subclass coercion) | Absent; callers must fall back to PyObject_Str. |
| `PyUnicode_FromWideChar` | MISSING | med | Windows/wchar interop | Absent. |
| `PyUnicode_GetDefaultEncoding` | MISSING | med | extensions querying default codec | Absent; should return 'utf-8'. |
| `PyUnicode_New` | DIVERGENT | med | cython/numpy (allocate-then-fill str) | Fills buffer with ASCII spaces and IGNORES maxchar (stores UTF-8). Intended fill pattern broken because WriteChar/CopyCharacters/Fill/WRITE are all MISSING; object is effectively a spaces string, not a writable buffer. |
| `PyUnicode_RSplit` | MISSING | med | text ext | Absent. |
| `PyUnicode_ReadChar` | MISSING | med | cython/numpy (per-codepoint access) | Absent; only FindChar reads code points. |
| `PyUnicode_Resize` | MISSING | med | cython/numpy (str buffer grow/shrink) | Stable-ABI; absent. |
| `PyUnicode_Split` | MISSING | med | cython/text ext | Absent (RSplit/Splitlines/Partition/RPartition also absent). |
| `PyUnicode_Splitlines` | MISSING | med | text ext | Absent. |
| `PyUnicode_Translate` | MISSING | med | text ext | Absent. |
| `PyUnicode_WriteChar` | MISSING | med | cython (allocate-then-write str idiom) | Absent. With missing WRITE macro, PyUnicode_New is unusable for its intended fill pattern. |
| `_PyUnicode_EqualToASCIIString` | MISSING | med | cython (interned/keyword fast compare) | Absent; CompareWithASCIIString exists but the boolean equal helper does not. |
| `_PyUnicode_FromId` | MISSING | med | cython/_Py_IDENTIFIER idioms | Absent; extensions using cached identifier objects cannot resolve them. |
| `PyByteArray_AS_STRING` | MISSING | low | cython | Not exported; header-macro over ob_start. |
| `PyByteArray_CheckExact` | MISSING | low | cython (exact-type guard) | Not exported; header-macro territory. |
| `PyByteArray_GET_SIZE` | MISSING | low | cython | Not exported; header-macro over ob_size. |
| `PyBytes_DecodeEscape` | MISSING | low | parser/marshal ext | Stable-ABI; absent. |
| `PyCodec_BackslashReplaceErrors` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_Decode` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_Decoder` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_Encode` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_Encoder` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_IgnoreErrors` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_IncrementalDecoder` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_IncrementalEncoder` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_KnownEncoding` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_LookupError` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_NameReplaceErrors` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_Register` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_RegisterError` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_ReplaceErrors` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_StreamReader` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_StreamWriter` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_StrictErrors` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_Unregister` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyCodec_XMLCharRefReplaceErrors` | MISSING | low | codec-registry / custom-codec / io-codecs | Entire PyCodec_* codecs.h family absent; str/bytes encode/decode C-API bypasses the codec registry. A C ext cannot register/lookup a codec or error handler. |
| `PyUnicode_AsCharmapString` | MISSING | low | legacy codecs | Absent. |
| `PyUnicode_AsDecodedObject` | MISSING | low | legacy (deprecated) | Absent. |
| `PyUnicode_AsDecodedUnicode` | MISSING | low | legacy (deprecated) | Absent. |
| `PyUnicode_AsEncodedObject` | MISSING | low | legacy (deprecated) | Absent. |
| `PyUnicode_AsEncodedUnicode` | MISSING | low | legacy (deprecated) | Absent. |
| `PyUnicode_AsMBCSString` | MISSING | low | Windows-only | Absent. |
| `PyUnicode_AsRawUnicodeEscapeString` | MISSING | low | repr | Absent. |
| `PyUnicode_BuildEncodingMap` | MISSING | low | charmap codecs | Absent. |
| `PyUnicode_DecodeASCII` | DIVERGENT | low | core-correctness | Correct strict decode, errors= ignored (strict only). |
| `PyUnicode_DecodeCharmap` | MISSING | low | legacy codecs | Absent. |
| `PyUnicode_DecodeCodePageStateful` | MISSING | low | Windows-only | Absent. |
| `PyUnicode_DecodeLatin1` | DIVERGENT | low | core-correctness | Correct (latin1 can't fail), errors= ignored (moot). |
| `PyUnicode_DecodeLocale` | MISSING | low | locale-aware ext | Absent. |
| `PyUnicode_DecodeLocaleAndSize` | MISSING | low | locale-aware ext | Absent. |
| `PyUnicode_DecodeMBCS` | MISSING | low | Windows-only | Absent; N/A on wasm, needed for native Windows ext parity. |
| `PyUnicode_DecodeMBCSStateful` | MISSING | low | Windows-only | Absent. |
| `PyUnicode_DecodeRawUnicodeEscape` | MISSING | low | repr/parsing | Absent. |
| `PyUnicode_DecodeUTF16` | DIVERGENT | low | core-correctness | Real endianness/BOM via String::from_utf16, but errors= ignored (strict only). |
| `PyUnicode_DecodeUTF16Stateful` | MISSING | low | codecs | Absent; non-stateful DecodeUTF16 present. |
| `PyUnicode_DecodeUTF32Stateful` | MISSING | low | codecs | Absent. |
| `PyUnicode_DecodeUTF7` | MISSING | low | email/imap codecs | Absent. |
| `PyUnicode_DecodeUTF7Stateful` | MISSING | low | codecs | Absent. |
| `PyUnicode_DecodeUTF8` | DIVERGENT | low | core-correctness | Validates + strict-decodes correctly, but the errors= parameter is ignored (always strict). replace/ignore/surrogatepass/surrogateescape silently unsupported -> raises instead of substituting (breaks os.fsdecode surrogateescape). |
| `PyUnicode_EncodeCodePage` | MISSING | low | Windows-only | Absent. |
| `PyUnicode_EncodeLocale` | MISSING | low | locale-aware ext | Absent. |
| `PyUnicode_GetSize` | MISSING | low | legacy ext (deprecated) | Absent; GetLength present. |
| `PyUnicode_InternImmortal` | ALIAS_NEEDED | low | stable-ABI completeness | Absent; could alias the no-op InternInPlace path (interning already a no-op in the bridge). |
| `PyUnicode_IsIdentifier` | MISSING | low | ast/compile ext | Absent. |
| `PyUnicode_Partition` | MISSING | low | text ext | Absent. |
| `PyUnicode_RPartition` | MISSING | low | text ext | Absent. |
| `_PyUnicode_Ready` | MISSING | low | legacy PyUnicode_READY callers | Absent; PyUnicode_READY should be a no-op macro returning 0 in the header overlay. |
| `PyBytes_AS_STRING` | FAITHFUL (claimed) | high | numpy/cython (A-CYTHON data access) | Bonus function export; returns runtime bytes_data pointer. |
| `PyBytes_AsString` | FAITHFUL (claimed) | high | cython/numpy | Real; delegates to AS_STRING. |
| `PyBytes_AsStringAndSize` | FAITHFUL (claimed) | high | numpy/cython (ubiquitous buffer access) | Real; BadInternalCall on NULL buf, TypeError on non-bytes, ValueError 'embedded null byte' when length==NULL and interior NUL. |
| `PyBytes_Check` | FAITHFUL (verified) | high | cython/numpy | Uses CPython's `Py_TPFLAGS_BYTES_SUBCLASS` fast-subclass contract; mask proof covers a synthetic bytes subtype. |
| `PyBytes_FromString` | FAITHFUL (claimed) | high | cython/numpy | Real; OOM fail-closed. |
| `PyBytes_FromStringAndSize` | FAITHFUL (claimed) | high | cython/numpy/witness (ubiquitous) | Real; NULL s zero-fills; OOM fail-closed. NOTE: unlike CPython returns a bridge-managed object, not a writable inline buffer -> PyBytes_AS_STRING read-only-ish and _PyBytes_Resize absent. |
| `PyBytes_GET_SIZE` | FAITHFUL (claimed) | high | numpy/cython (A-SIZE tier) | Bonus function export aliasing Size (CPython macro). |
| `PyBytes_Size` | FAITHFUL (claimed) | high | numpy/cython (A-SIZE tier) | Real; TypeError 'expected bytes, X found' on non-bytes, -1 carries exception. |
| `PyBytes_Type` | FAITHFUL (claimed) | high | numpy/cython (identity checks) | Exported #[no_mangle] static, tp_name='bytes'. |
| `PyUnicode_AsUTF8AndSize` | PARTIAL (verified) | high | cython/numpy | Single handle resolution; non-str returns TypeError+NULL; returns a stable object-owned NUL-terminated cache and reports the payload length excluding the terminator. Remaining divergence: lone-surrogate encoding. |
| `PyUnicode_GetLength` | FAITHFUL ✓verified | high | cython/numpy (A-SIZE-adjacent) | strings.rs:677 counts CODE POINTS (chars().count(), not bytes) — astral/BMP/latin1 all correct; non-str -> PyErr_BadArgument+-1 (exact TypeError); empty '' -> 0. Verified vs 3.12. O(n) vs CPython O(1) (perf only). |
| `PyByteArray_AsString` | FAITHFUL (claimed) | med | cython/mutable-buffer ext | Returns ob_start; NULL on non-bytearray. |
| `PyByteArray_Check` | FAITHFUL (claimed) | med | cython | Address compare to PyByteArray_Type. |
| `PyByteArray_FromStringAndSize` | FAITHFUL (claimed) | med | cython/mutable-buffer ext | Real native PyByteArrayObject (ob_bytes/ob_start/ob_alloc), NUL-terminated, PyMem_Calloc-backed. |
| `PyByteArray_Size` | FAITHFUL (claimed) | med | cython | Returns ob_size; -1 on non-bytearray. |
| `PyByteArray_Type` | FAITHFUL (claimed) | med | cython (identity checks) | Exported static, tp_name='bytearray', tp_dealloc=molt_bytearray_dealloc. |
| `PyBytes_Concat` | FAITHFUL (claimed) | med | cython (bytes building) | Real join via runtime bytes_data; clears *pv + MemoryError on OOM (previously a stub that dropped newpart). |
| `PyUnicode_AsASCIIString` | FAITHFUL (claimed) | med | cython | Raises UnicodeEncodeError with CPython-shaped message on non-ASCII. |
| `PyUnicode_AsLatin1String` | FAITHFUL (claimed) | med | cython | UnicodeEncodeError for code points >0xFF. |
| `PyUnicode_AsUCS4` | FAITHFUL (claimed) | med | numpy 'U' dtype | Real; SystemError on undersized target, BadInternalCall on NULL/neg. |
| `PyUnicode_AsUCS4Copy` | FAITHFUL (claimed) | med | numpy 'U' dtype | Real; PyMem_Malloc'd NUL-terminated UCS4. |
| `PyUnicode_AsUTF8String` | FAITHFUL (claimed) | med | cython/numpy | Real; returns bytes. |
| `PyUnicode_Compare` | FAITHFUL (claimed) | med | cython (ordering) | Byte-lexicographic (UTF-8) ordering; TypeError sentinel-safe. NOTE: differs from CPython code-point ordering only for astral-vs-BMP edge. |
| `PyUnicode_CompareWithASCIIString` | FAITHFUL (claimed) | med | cython/numpy (attr/keyword matching) | Real. |
| `PyUnicode_Concat` | FAITHFUL (claimed) | med | cython | Real; OOM fail-closed. |
| `PyUnicode_Contains` | FAITHFUL (claimed) | med | cython | UTF-8 self-synchronizing substring search; TypeError on non-str element. |
| `PyUnicode_FindChar` | FAITHFUL (claimed) | med | cython | Code-point indexed forward/reverse search; -1 not-found / -2 error. |
| `PyUnicode_Format` | FAITHFUL (claimed) | med | '%'-format ext | Substantial printf engine: flags/width/precision/*, %s%r%a%d%i%u%o%x%X%c + floats, mapping keys. |
| `PyUnicode_FromEncodedObject` | FAITHFUL (claimed) | med | cython | Rejects str input, decodes bytes via requested encoding (subset), TypeError on non-bytes-like. |
| `PyUnicode_FromKindAndData` | FAITHFUL (claimed) | med | numpy 'U' dtype interchange | Real UCS1/2/4 -> UTF-8 conversion; rejects surrogates/out-of-range scalars. |
| `PyUnicode_Join` | FAITHFUL (claimed) | med | cython/numpy | tuple/list fast-path + generic sequence; TypeError 'sequence item N: expected str'. |
| `PyUnicode_Replace` | FAITHFUL (claimed) | med | text ext | Real; empty-needle inserts between code points correctly. |
| `_PyUnicode_IsWhitespace` | FAITHFUL (claimed) | med | numpy/cython (linked private helper) | Backed by generated Unicode space-range table. |
| `_Py_ascii_whitespace` | FAITHFUL (claimed) | med | numpy (links the [128] table as DATA for bytes strip/split) | Exported static matching CPython's exact whitespace byte set. LANDED via DISCOVERY-FRONTIER-FIXES. |
| `PyUnicode_FromOrdinal` | FAITHFUL (claimed) | low | chr()-style ext | Real; ValueError on out-of-range ordinal. |
| `PyUnicode_GET_LENGTH` | FAITHFUL (claimed) | low | macro-as-symbol callers | Bonus: exported as a function aliasing GetLength (CPython has it as an inline macro). |
| `PyUnicode_IS_ASCII` | FAITHFUL (claimed) | low | macro-as-symbol callers | Bonus function export; true iff UTF-8 bytes are ASCII. |
| `PyUnicode_InternFromString` | FAITHFUL (claimed) | low | cython | Delegates to FromString (no explicit intern). |
| `PyUnicode_InternInPlace` | FAITHFUL (claimed) | low | cython (interned singletons) | No-op (allocator already de-dups); *p unchanged. Acceptable per contract but does not force canonical identity. |
| `PyUnicode_Substring` | FAITHFUL (claimed) | low | text ext | Code-point-indexed slice. |
| `PyUnicode_Tailmatch` | FAITHFUL (claimed) | low | startswith/endswith ext | Code-point window mapping; direction>0 endswith. |
| `_PyUnicode_IsAlpha` | FAITHFUL (claimed) | low | str.isalpha ext | General-category L* check. |
| `_PyUnicode_IsDecimalDigit` | FAITHFUL (claimed) | low | str.isdecimal ext | Generated decimal-range table. |
| `_PyUnicode_IsDigit` | FAITHFUL (claimed) | low | str.isdigit ext | Generated digit-range table. |
| `_PyUnicode_IsLinebreak` | FAITHFUL (claimed) | low | splitlines ext | Real (matches CPython linebreak set). |
| `_PyUnicode_IsLowercase` | FAITHFUL (claimed) | low | str-method ext | Real. |
| `_PyUnicode_IsNumeric` | FAITHFUL (claimed) | low | str.isnumeric ext | Generated numeric-range table. |
| `_PyUnicode_IsPrintable` | FAITHFUL (claimed) | low | repr ext | Generated printable-range table. |
| `_PyUnicode_IsTitlecase` | FAITHFUL (claimed) | low | str-method ext | Real (Lt category). |
| `_PyUnicode_IsUppercase` | FAITHFUL (claimed) | low | str-method ext | Real. |

### 6. errors-exc — `PyErr_*` / `PyException_*` / exception-type hierarchy (PARTIAL — see caveat)

*4 entries — 4 gap, 0 faithful.*

| symbol | status | sev | needed_by | note |
|---|---|:--:|---|---|
| `PyErr_Fetch` | DIVERGENT | high | cython/numpy exception save-restore & chaining | errors.rs:264 materializes value as message-str, tb ALWAYS NULL, type->Py_None when handle unresolvable. Fetch->Restore preserves type+message only; a caller inspecting value attributes or re-raising with the original traceback gets wrong data. Symptom of the FUNDAMENTAL MODEL LIMIT below. |
| `PyErr_NewException` | MISSING | high | numpy/scipy/cython (witness Tier-A A-EXC; lapack_lite + every custom-exception ext) | Not exported; not declared in molt Python.h. Molt has a singleton hierarchy but no runtime-backed dynamic exception-class constructor. Named gap in NATIVE_DISCOVERY_FRONTIERS.md L284 (one of the 14). |
| `PyErr_NewExceptionWithDoc` | MISSING | high | numpy/cython (custom exception types carrying __doc__) | Companion to PyErr_NewException; also absent. |
| `_Py_FatalErrorFunc` | ALIAS_NEEDED | high | scipy _nd_image + _ni_label (witness Tier-A A-FATAL); 3.12 Py_FatalError macro expands to _Py_FatalErrorFunc(__func__,msg) | Py_FatalError (memory.rs:258, print+abort) exists; expose _Py_FatalErrorFunc(func,message) as a thin wrapper reusing it. Extensions recompiled against real 3.12 headers reference this symbol, not Py_FatalError. Docs L289 (one of the 14). |

> **errors-exc is intentionally short here** — the source audit was truncated mid-domain. See §1.4 for the domain aggregate (101 spec functions, 25 implemented-but-model-limited, 69 `PyExc_*` types = 49 faithful / 20 missing). The full per-symbol errors-exc rows are a follow-up enumeration (Lane 8).

---

## 3. Close-the-surface batch-fix plan (8 lanes, witness-critical first)

Every gap (MISSING / STUB_THEATER / DIVERGENT / ALIAS_NEEDED / DOWNGRADE) is folded into **8 coherent lanes**, each sized for one agent, ordered so the field_solve witness unblocks first. A few symbols legitimately appear in two lanes (the witness lane **L1** exports them first; the domain lane owns the rest of the cluster) — flagged inline.

| # | lane | severity | fixes | ~size |
|---|---|:--:|---|:--:|
| **L1** | **WITNESS TIER-A SYMBOL CLOSE (the 14)** | **P0** | The complete field_solve symbol frontier — unblocks scipy Cython (`_ni_label`,`_nd_image`) + `lapack_lite`. In this matrix: `_PyObject_CallFunction_SizeT`,`_PyObject_CallMethod_SizeT` (A-SIZE aliases), `PyErr_NewException`,`PyErr_NewExceptionWithDoc` (A-EXC), `_Py_FatalErrorFunc` (A-FATAL alias), `PyVectorcall_Function`,`_PyList_Extend`,`_PyUnicode_FastCopyCharacters` (A-CYTHON). Cross-domain (named in FRONTIERS, outside these 6 domains): `_PyArg_ParseTuple_SizeT`,`_PyArg_ParseTupleAndKeywords_SizeT`,`_PyArg_VaParseTupleAndKeywords_SizeT`,`PyCMethod_New`,`PyImport_GetModule`,`PyThread_allocate_lock`/`free_lock`. Also verify `PyUnstable_Long_IsCompact`/`CompactValue` vs the wasm-recompiled Cython. | S (aliases + 3 thin ctors) |
| **L2** | **NUMBER-PROTOCOL DISPATCH & IN-PLACE** | **P0 silent-fail** | `abstract_number.rs` `binary_op`: on bridge-miss, dispatch operand `tp_as_number` nb_* (as unary Long/Float/Index already do) instead of `SystemError`+NULL — fixes `PyNumber_Multiply(pyint, np_scalar)` and all 14 binary ops; make `InPlace*` call `nb_inplace_*` (`__iadd__`) not the non-in-place op; add `PyNumber_MatrixMultiply` consistency. | M (1 file + teeth) |
| **L3** | **CALL / VECTORCALL PROTOCOL** | **P0 correctness+crash** | `object.rs` call cluster: `PyObject_Vectorcall` honor kwnames (stop dropping them + returning bare NULL); `PyVectorcall_Call` read `tp_vectorcall_offset` (avoid tp_call recursion → stack overflow); `VectorcallDict` mask `PyVectorcall_NARGS`; `PyObject_Call` vectorcall-first; `CallObject` non-tuple TypeError. Exports `PyVectorcall_Function` *(shared L1)*. | M |
| **L4** | **TYPE-OBJECT SUBSTANCE + READY/FROMSPEC + PEP 573/384/697** | **high** | (a) Populate DIVERGENT type shells `PyLong/PyFloat/PyComplex/PyBool_Type` (basicsize, subclass flags, number slots, hash/richcompare, tp_base). (b) `PyType_Ready`: add_operators (dunders→tp_dict), tp_bases, add_subclasses, DISALLOW_INSTANTIATION, unhashable `__hash__=None`, inherit_slots pair/guard rules, ready-unready-base; fix POISON_ORPHAN #2 add_methods fail-open. (c) `FromSpecWithBases`: HEAPTYPE flag, INCREF bases/tp_base, member-offset special-casing, best_base/BASETYPE, PEP-697 basicsize, subtype_dealloc. (d) `GenericAlloc` nitems+1 + GC/managed-dict pre-header. (e) `FromSpec` alias+decl, `FromModuleAndSpec` module assoc, `FromMetaclass`, `GetSlot`, `GetModule`/`GetModuleState`/`GetModuleByDef`, `PyType_CheckExact` macro, complete typeslots.h to 81 ids. *May split 4a (substance+Ready+FromSpec+GenericAlloc) / 4b (PEP-573/384/697 + header).* | L (split-candidate) |
| **L5** | **CONTAINER C-API CONTRACT + MISSING fns** | **high** | DOWNGRADE fixes: `PyDictProxy_New` (real forwarding proxy + validation, not a zeroed sentinel), `PyDict_Merge` override==2 collapse-to-bool, non-dict/non-tuple type checks (`PyDict_SetItem`/`Size`,`PyTuple_Size`), `PyList_SET_ITEM` unchecked-void macro, `PyList_Append` type-check+honor-result, `PySet_Add` frozenset-refcnt==1 guard + subtype, `PyTuple_New`/`Pack` negative/singleton/cap/exception. MISSING: `PyDict_Clear`/`Update`/`MergeFromSeq2`, `PyFrozenSet_New`, `PySet_Pop`/`Clear`, `_PyList_Extend` *(shared L1)*, `_PyTuple_Resize`, `_PyDict_*KnownHash`/`_PyDict_Next`/`_PySet_NextEntry`, + 13 dict/list/tuple/set view+iter type objects. | M–L |
| **L6** | **STRING/BYTES CONSTRUCTION & UTF-8 CORRECTNESS** | **P0 silent-wrong + memory-safety** | (a) **Strict UTF-8 decode** in `PyUnicode_FromString`/`FromStringAndSize` (today store malformed bytes silently). (b) `PyUnicode_AsUTF8`/`AsUTF8AndSize` **NUL-termination** (heap over-read). (c) `PyUnicode_Check` subtype walk (numpy.str_). (d) allocate-then-fill: `PyUnicode_New` maxchar + `WriteChar`/`CopyCharacters`/`Fill`/`Resize`/`ReadChar` + `_PyUnicode_FastCopyCharacters` *(shared L1)*. (e) MISSING builders/ops: `PyBytes_FromObject`/`_PyBytes_Resize`/`FromFormat`/`ConcatAndDel`/`Repr`, `PyByteArray_Resize`/`Concat`/`FromObject`, `PyUnicode_RichCompare`/`FromObject`/`Find`/`Count`/`Translate`/`Split*`/`Append`, `FSConverter`/`FSDecoder`/`DecodeFSDefault*`, wide-char. (f) `PyUnicode_FromFormat*` %U/%A/%c/%x + width/precision. (g) `*_CheckExact` macros. *May split 6a (UTF-8/AsUTF8/Check/builders) / 6b (search/split/path/wide breadth).* | L (split-candidate) |
| **L7** | **NUMERIC LONG-TAIL & SERIALIZATION** | **med (2 high)** | `PyUnstable_Long_IsCompact`/`CompactValue` (Cython-3 link blocker) *(shared L1)*; `PyLong_FromString`, `AsSize_t`, `AsUnsignedLong(LongLong)Mask` (bignum mask), `_PyLong_FromByteArray`/`_AsByteArray` arbitrary-precision (fix >64-bit silent truncation), `PyLong_FromSsize_t` LLP64, Long/Complex `CheckExact` exact-identity, `_PyLong_Sign`/`_NumBits`, 5× `_PyLong_*_Converter`, `PyLong_GetInfo`; `PyFloat_Pack/Unpack 2/4/8` (float16+struct) + `PyFloat_GetInfo`/`GetMax`/`GetMin`; `_Py_c_sum/diff/neg/prod/quot/pow/abs`; `_Py_TrueStruct`/`_Py_FalseStruct` aliases. | M (mechanical breadth) |
| **L8** | **CODECS, ERROR-MODEL & LONG-TAIL** | **deep (low witness-priority)** | (a) **Error model**: materialize a real exception instance carrying args + traceback + `__cause__`/`__context__` so `PyErr_Fetch`/`Restore`/`Normalize` round-trip faithfully; finish the errors-exc per-symbol enumeration (101 fns, only 4 rowed here) + the 20 MISSING `PyExc_*` types. (b) `PyCodec_*` registry (19) + `errors=` handlers in `Decode*`/`AsEncoded*` + non-utf8 codecs. (c) buffer helpers `PyBuffer_ToContiguous`/`FromContiguous`/`GetPointer`/`CopyData`/`SizeFromFormat`. (d) `PyObject_ClearWeakRefs` real weakref model. (e) PEP-697 `GetTypeData`/`GetItemData`/`GetTypeDataSize`, dict watchers, misc low (`DelItemString`,`DelSlice`,`AIter_Check`,`LengthHint`, deprecated/Windows codecs). | L (error-model half is one agent) |

**Ordering rationale.** L1 is the *only* lane that flips the witness from red at the symbol level — everything scipy-Cython needs is in it. L2/L3/L6 are the P0 *silent-wrong-answer / memory-safety / crash* correctness lanes (a present-but-wrong symbol is worse than a missing one — it fails the M05 zero-fakes bar quietly). L4/L5 are the structural type-object + container faithfulness lanes that numpy static-type init leans on. L7 is mechanical breadth. L8 is the deep error-model rework + the low-witness-priority long tail, deliberately last.

---

## 4. What's already tracked (dedup map — do not re-file)

This matrix is an **index over** the existing ledgers, not a new tracker. The mapping below says where each gap-cluster is already owned so a lane *sources* its fix-list instead of re-discovering it.

| gap cluster | already tracked in | this-matrix lane | relationship |
|---|---|:--:|---|
| The 14 field_solve Tier-A witness symbols (A-SIZE/A-EXC/A-FATAL/A-CYTHON) | **NATIVE_DISCOVERY_FRONTIERS.md** (§Tier A, the whole-witness sweep) | L1 | **fully tracked** — L1 *is* those 4 sub-lanes; this doc does not duplicate, it sources them. |
| The 19 DOWNGRADED faithful claims + the 28 DIVERGENT rows (per-defect) | **CPYTHON_ABI_DIVERGENCE_LEDGER.md** (248 rows; the verdicts cite rows 102/160/247/275/276/289/293/300/311/344/345…) | L2,L3,L5,L6,L7 | **tracked per-defect** — lanes draw fix-lists from these rows; no new filing. |
| Type-object *layout* shells (`PyLong_Type`/`PyFloat_Type` basicsize/flags, container type objects) | **CPYTHON_ABI_BINARY_CONTRACT_MATRIX.md** (type-object-layout family; 33 gaps / 79 items) | L4,L5 | **overlap** — binary-contract owns the *layout bytes*, this matrix owns *behavioral slot population*; do them together per type. |
| `PyObject_ClearWeakRefs` (STUB_THEATER); `PyType_Ready` add_methods fail-open method-drop | **POISON_ORPHAN_LEDGER.md** (#16, #2 — landed feb4c1ef01) | L8, L4 | **tracked** — L4/L8 reconcile with the poison ledger, do not re-file. |
| SystemError/NULL fail-closed paths (`PyNumber_*` bridge-miss, vectorcall bare-NULL) | **PANIC_REACHABILITY_LEDGER.md** (panic-adjacent) | L2,L3 | **adjacent** — the fix removes the fail-closed path; coordinate wording. |
| `_PyDict_GetItemStringWithError`, `_Py_ascii_whitespace`, allocators, datetime CAPI | **CLAIMS.md** DISCOVERY-FRONTIER-FIXES (LANDED) + B.1 | — | **already landed** — marked faithful here; excluded from lanes. |
| `include/molt/Python.h` overlay-hosted divergent inlines (`PyLong_FromString`, `PyLong_AsSize_t`, `PyFloat_GetInfo` overlay routes) | orchestrator task **B1 / #73.2** (numpy-header-overlay custody) + **D1 / #10** (header-decl drift) | L4,L6,L7 | **coordinate** — the header work in these lanes must land *with* the overlay deletion, not fight it (M56). |

### 4.1 Net-new in this matrix (not previously catalogued)

- **The spec-first enumeration itself** — a systematic surface walk replacing reactive leaf-discovery. Prior ledgers are defect-driven (they record what was *hit*); this is the first *complete* object-model surface map.
- **175 MISSING symbols** catalogued whole-cloth here — the `PyCodec_*` family (19), wide-char (`FromWideChar`/`AsWideChar*`), the 5 `_PyLong_*_Converter` argument-clinic converters, the 13 container view/iter type objects, `PyFloat_Pack/Unpack`, `_Py_c_*` complex primitives, PEP-697, dict watchers — many never leaf-discovered because the witness path does not reach them yet.
- **The errors-exc 101-function domain enumeration** — begun here (4 rows + aggregate); the full per-symbol walk is Lane 8 net-new work.
- **The DOWNGRADE accounting** — quantifying that **63% of adversarially-audited FAITHFUL claims did not survive** is the headline honesty result and is not recorded elsewhere.

---

*Generated spec-first from the audited entry set. Counting unit = one matrix entry (row); a row may name >1 symbol (≈510 named symbols across 345 rows). Re-audit verdicts (CONFIRMED/DOWNGRADE) were checked against CPython 3.12 primary source per M06. errors-exc is partial (source stream truncated); treat it as the weakest domain.*
