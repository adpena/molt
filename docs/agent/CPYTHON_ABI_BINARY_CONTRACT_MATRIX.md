# CPython-ABI Binary-Contract Coverage Matrix — object model, structs & data symbols

> **Scope.** The *binary* contract: struct layout (field order, width, offset,
> alignment, `sizeof`), exported **data-symbol** names and their initial bytes,
> and flag/slot-ID constant values — everything a compiled C extension (numpy /
> scipy / Cython) reads or writes *by memory layout* rather than by calling a
> function. This is the substrate the behavioral catalogue sits on top of; a wrong
> offset or a mortal static here is silent memory corruption, not a wrong return
> value. **Complements** `CPYTHON_ABI_DIVERGENCE_LEDGER.md` (function *behavior* —
> MISSING_DISPATCH / DIVERGENT / SILENT_SENTINEL) and the C-API coverage matrix
> (workflow `wf_4ab826d5`, function-*presence*). See §4 for the overlap map.
>
> **Authorities compared, per item.** (1) the Rust `#[repr(C)]` struct or
> `#[no_mangle]` static in `runtime/molt-cpython-abi/src/` (`abi_types.rs`,
> `object.rs`, `refcount.rs`, `typeobj.rs`, `api/*.rs`); (2) the C header a witness
> extension actually compiles against (`runtime/molt-cpython-abi/include/Python.h`
> + `include/molt/Python.h` + `include/datetime.h`); (3) the machine-checked
> `include/_molt_abi_layout.generated.h` `_Static_assert` gate; (4) the **primary**
> CPython source (`v3.12.0`/`v3.13.0`/`v3.14` `Include/` — verified against
> `python/cpython`, not from memory, per M06). A claim of MATCHES is only kept when
> all four agree.
>
> **Anchor.** Line numbers are as of `origin/main` at authoring time; they are
> anchors, not identities — re-anchor by grepping the symbol when a line drifts.
> Molt is version-pinned to **3.12** (`Py_Version = 0x030c00f0`); 3.13/3.14 rows
> assess what happens when a target beyond 3.12 enters the M02 verified subset.
>
> **Doctrine.** Operationalizes M02 (≥3.12, version-gated within the verified
> subset), M05 (zero fakes), M34 (silently-degrades → fix the class or fail LOUD),
> and the drift-gate discipline of M45. Brutally honest by construction: the
> default disposition of a layout claim is DOWNGRADE unless the primary source is
> reproduced.

---

## 1. Executive summary

### 1.1 The honest verdict — does Molt's binary ABI match CPython ≥3.12?

**Not yet — it is a faithful layout *substrate* wrapped around an incomplete
object-model *contract*.** Of **79 audited binary-contract items across 6
families, 46 MATCH (58%) and 33 are gaps (42%)**.

The good news is real and load-bearing: **every struct that carries a
`_Static_assert` gate is byte-pinned on both `wasm32` (ILP32) and 64-bit
(LP64/LLP64)** — `PyObject`, `PyVarObject`, `PyTypeObject` (sizeof 416 / 208),
`Py_buffer`, `PyModuleDef(_Base/_Slot)`, `PyType_Spec/Slot`, `PyMethodDef`,
`PyMemberDef`, `PyGetSetDef` — and a drift there **fails to compile**. All 36
`PyNumberMethods`, 10 `PySequenceMethods`, 3 `PyMappingMethods`, `PyBufferProcs`,
the 15-field `PyDateTime_CAPI`, the typeslots IDs, and the vectorcall
constant/macros are field-for-field correct against the 3.12 primary source. The
wasm32 4-byte-alignment clobber class (`bridge.rs` header reads) is *fixed and
gated*. For the **primary target** — an all-source witness where numpy/scipy/
Cython are recompiled against *molt's own* `Python.h` and every refcount op is
routed through molt's **out-of-line** `Py_INCREF`/`Py_DECREF` — most of the
object model is internally consistent.

The bad news is equally real: **13 of the 33 gaps are high-severity, and ~8 of
those are live memory-corruption or crash-class vectors that bite even inside the
molt-header witness model** — mortal static objects that `tp_dealloc` themselves
to zero, `bool` singletons shaped as bare `PyObject` that numpy reads past
(`ob_digit` OOB), a `None` sentinel split across two unreconciled symbols
(foreign-`None` → `Py_TYPE(Py_None)` null-deref, `is None` → `False`),
`PyType_FromSpec` heap types under-allocated as a bare `Box<PyTypeObject>` that
`ht_name`/`ht_module` read past, and a vectorcall path that either returns
`NULL` with **no exception set** or **infinitely recurses** when wired as
`tp_call`. A second cluster is pure **build-break**: ~20 missing `PyExc_*` data
symbols, the `Py_TPFLAGS_*` fast-subclass macros, the `PyDateTime_GET_*`
accessor macros, `_Py_NotImplementedStruct`, `_Py_EllipsisObject`, and
`PyVectorcall_Function` — each an undefined-symbol / undeclared-identifier that
stops a Cython- or broad-`PyExc_*`-using extension from compiling or linking at
all.

Two structural cross-cutting failures underlie the object-model gaps:

- **No single immortality authority.** Molt uses *four* different "immortal"
  encodings — `1`, `0` (zeroed), `1<<30`, and the header macro — across static
  types, exception singletons, and the None/True/False sentinels. Its own
  `Py_DECREF` treats `ob_refcnt ≥ 1<<29` as immortal, so the objects initialised
  to `1` or `0` are **mortal**, and a benign refcount imbalance CPython absorbs
  (because *its* statics are immortal) drives molt to free static memory.

- **Almost no version-gating.** The contract is frozen at 3.12 with `Py_Version`
  hard-pinned; the 3.13/3.14 `tp_versions_used` tail and the free-threaded
  (`Py_GIL_DISABLED`) header — a *completely different* `PyObject` shape — are
  UNGATED. Nothing refuses an incompatible-ABI extension, so a cp313t/cp314t
  pairing corrupts silently instead of failing honest-early (an M02/M34 breach).

For any path that leaves the molt-header model — a **prebuilt/native `.so`
compiled against genuine CPython headers** with *inlined* refcount macros — the
immortal-*value* divergence (`1<<30` vs `UINT_MAX`) and the dual-symbol split
become latent corruption on `Py_None`/`Py_True`/exceptions. That path is out of
the current WASM witness subset, but nothing gates it either.

**Bottom line:** the layout math is world-class where it is gated; the
object-model *contract* (immortality, canonical singletons, full type-object
initialisation, real vectorcall) and the *version-gating* are not yet at the bar
a drop-in CPython-ABI compiler requires. All 33 gaps are enumerated below and
close in **6 batch-fix lanes** (§3), memory-corruption first.

### 1.2 Counts by status × severity

| status | high | med | low | total | disposition |
|--------|-----:|----:|----:|------:|-------------|
| **MATCHES** | 17 | 5 | 24 | **46** | byte-verified against primary source (gated where a `_Static_assert` exists) |
| **WRONG_SEMANTICS** | 6 | 6 | 3 | **15** | correct-ish layout, wrong runtime contract (mortal statics, slot-bypass, shells) |
| **MISSING** | 5 | 2 | 2 | **9** | symbol / macro / typedef absent → build-break or undefined reference |
| **LAYOUT_MISMATCH** | 1 | 0 | 1 | **2** | field width/shape wrong → OOB read (1 benign, 1 corruption) |
| **WRONG_NAME** | 1 | 1 | 0 | **2** | right bytes, non-canonical exported symbol name → link fail off-model |
| **VERSION_UNGATED** | 0 | 3 | 2 | **5** | correct at 3.12, no gate/refusal for 3.13/3.14/free-threaded |
| **TOTAL** | **30** | **17** | **32** | **79** | 46 match · **33 gaps** |

### 1.3 Gaps that are version-deltas (must be gated)

**12 of 79 items are flagged `version_delta`; 9 of the 33 gaps are version-deltas
that require explicit per-target-version gating or an honest-early refusal** (the
rest of the version-delta items already MATCH or are handled). These are the M02
teeth that do not yet exist:

- Free-threaded (`Py_GIL_DISABLED`, cp313t/cp314t) `PyObject` header — a different
  struct shape; **2 items** (object-header + the module-def family that embeds it).
- `PyTypeObject` 3.13/3.14 tail field `uint16_t tp_versions_used` — absent, no gate.
- 3.14 immortal-refcount representation change (`_Py_IMMORTAL_REFCNT = 3ULL<<30`,
  `_Py_STATIC_IMMORTAL_INITIAL_REFCNT`, `refcount.h` split) — ungated.
- Immortal-refcount sentinel value (`1<<30` vs `UINT_MAX`) — ungated for
  prebuilt-3.12+ inlined refcounting.
- `PyExc_*` completeness and `Py_SET_REFCNT` / `_Py_IsImmortal` / `Py_RELATIVE_OFFSET`
  — 3.12+ / 3.14-surfaced additions absent from the header.

**The verdict this section forces:** a "≥3.12 version-gated within the verified
subset" contract (M02) cannot be claimed while the *only* gate is a single
`_Static_assert sizeof==416` that silently forces every 3.13/3.14 target back to
the 3.12 shape. Version-gating is Lane 6.

### 1.4 Top memory-corruption / high gaps (ranked, worst first)

The 8 with the largest blast radius inside the molt-header witness model
(corruption/crash first, then wrong-answer):

1. **`_Py_True/FalseStruct` bool object is bare `PyObject`, not `PyLongObject`**
   (LAYOUT_MISMATCH, H) — `((PyLongObject*)Py_True)->ob_digit[0]` reads OOB into
   adjacent static memory → garbage int; the `object.rs` copies also have
   `ob_type=NULL`. *Lane 1.*
2. **Static objects not immortal-initialised** (WRONG_SEMANTICS, H) — `Py*_Type`
   at `ob_refcnt=0`, `PyExc_*` at `1`, header macro at `1`; a net-negative
   `Py_DECREF` hits 0 → `release_pyobj`/`tp_dealloc` on **statically-allocated**
   memory = the O!-header-clobber corruption class. *Lane 2.*
3. **`_Py_NoneStruct` / `_Py_TrueStruct` / `_Py_FalseStruct` unreconciled with the
   live `Py_None`/`Py_True`/`Py_False`** (WRONG_SEMANTICS, H) — separate statics,
   `ob_type=NULL`, never bridge-registered → foreign `None` (`is None` → `False`)
   and `Py_TYPE(Py_None)->tp_name` null-deref. *Lane 1.*
4. **`PyType_FromSpec`/`FromModuleAndSpec` not `HEAPTYPE`, bare `Box<PyTypeObject>`
   not `PyHeapTypeObject`** (WRONG_SEMANTICS, H) — `ht_name`/`ht_module`/
   `_spec_cache` reads run past the 416-byte allocation; module state dropped.
   *Lane 3.*
5. **`PyExc_*` are bare `PyObject` sentinels (`ob_type=NULL`), not exception TYPE
   objects; builtin `Py*_Type` are zero-init shells** (WRONG_SEMANTICS, H+M) —
   `PyErr_NewException`/`PyType_FastSubclass` read `tp_flags`/`tp_dict`/
   `tp_basicsize` OOB past an 8/16-byte sentinel; foreign subclasses inherit
   `tp_basicsize=0`. *Lane 3.*
6. **`PyVectorcall_Call` infinite-recurses when used as `tp_call`**
   (WRONG_SEMANTICS, H) — it re-enters `PyObject_Call` → `tp_call` → itself; the
   *intended, documented* `tp_call = PyVectorcall_Call` pattern → stack overflow /
   SIGSEGV. *Lane 4.*
7. **`PyObject_Vectorcall` returns `NULL` with no exception on any kwnames, and
   never reads the vectorcall slot** (WRONG_SEMANTICS, H) — the entire PEP-590
   fast path is a no-op → `SystemError: NULL result without error` or wrong-answer
   crash for Cython keyword fast-calls / numpy ufunc dispatch. *Lane 4.*
8. **Builtin static types carry `tp_flags = READY` only** (WRONG_SEMANTICS, H) —
   no `DEFAULT|BASETYPE|<TYPE>_SUBCLASS`, so `PyType_FastSubclass(&PyLong_Type,
   LONG_SUBCLASS)==0` and subclassability checks are wrong; numpy's inlined
   feature tests miss. *Lane 3.*

Immediately behind them, the **build-break MISSING cluster** (all H, compile/link
failures rather than corruption): the `Py_TPFLAGS_*` fast-subclass macros, the
~20 absent `PyExc_*` symbols, the `PyDateTime_GET_*` macros + struct typedefs,
`_Py_NotImplementedStruct`, `_Py_EllipsisObject` (WRONG_NAME), and
`PyVectorcall_Function`.

---

## 2. The full matrix, per family

Gaps are listed first within each family (high → med → low), MATCHES after. Each
family shows a scannable table (`item | status | sev | vΔ | needed_by`) followed
by the **verbatim `note`** for every row, numbered to match — the forensic
detail *is* the note column, expanded so it stays readable. `vΔ` = version-delta.

### Object header & refcount (PyObject / PyVarObject / immortality)

*11 items — 6 gap(s), 5 MATCHES. The `note` column is expanded verbatim in the numbered detail directly below the table.*

| # | item | status | sev | vΔ | needed_by |
|---|------|--------|-----|----|-----------|
| 1 | Static objects not immortal-initialized (PyObject_HEAD_INIT=1; Py*_Type zeroed to refcnt 0; PyExc_* singletons refcnt 1) | WRONG_SEMANTICS | high | yes | numpy/scipy/Cython static PyTypeObjects, dtype/scalar singletons, PyExc_* exception objects |
| 2 | Free-threaded (Py_GIL_DISABLED, cp313t/cp314t) PyObject layout — ob_tid/ob_mutex/ob_gc_bits/ob_ref_local/ob_ref_shared | VERSION_UNGATED | med | yes | free-threaded numpy/scipy wheels (cp313t/cp314t) |
| 3 | Immortal refcount VALUE/representation diverges from CPython (molt 1<<30 + threshold 1<<29 vs 0xFFFFFFFF / 0x3FFFFFFF) | WRONG_SEMANTICS | med | — | refcounting of Py_None/Py_True/Py_False/NotImplemented/Ellipsis across numpy/scipy/Cython |
| 4 | _Py_IsImmortal / _Py_IMMORTAL_REFCNT / public Py_IsImmortal absent from molt header | MISSING | med | ? | Cython 3.x-generated modules, numpy 2.x internals, 3.14-targeted code |
| 5 | 3.14 immortal-refcount representation change ungated (constants + _Py_STATIC_IMMORTAL_INITIAL_REFCNT + refcount.h split) | VERSION_UNGATED | low | yes | 3.14-targeted extensions that inline _Py_IsImmortal |
| 6 | Py_SET_REFCNT lacks the immortal guard | WRONG_SEMANTICS | low | yes | numpy/Cython code using Py_SET_REFCNT on shared objects |
| 7 | PyObject struct: ob_refcnt (Py_ssize_t) @0, ob_type (PyTypeObject*) @ptr-size; sizeof 8/16 | MATCHES | low | — | every numpy/scipy/Cython PyObject access |
| 8 | PyVarObject: ob_base (PyObject) + ob_size (Py_ssize_t) @ptr-size | MATCHES | low | — | numpy tuple/bytes/type (PyVarObject-derived) size access, Py_SIZE |
| 9 | ob_refcnt union with PY_UINT32_T ob_refcnt_split[2] (64-bit) not exposed | MATCHES | low | — | numpy prebuilt (inlined) refcounting on 64-bit native |
| 10 | _PyObject_HEAD_EXTRA (Py_TRACE_REFS _ob_next/_ob_prev doubly-linked list) omitted | MATCHES | low | — | numpy/scipy release (non-debug) builds |
| 11 | PyObject alignment on wasm32 = 4 bytes (pointer-sized) — the bridge.rs:571 alignment bug class | MATCHES | low | — | numpy static PyObjects / bridge header reads on wasm32 |

**Notes — Object header & refcount (PyObject / PyVarObject / immortality):**

1. **WRONG_SEMANTICS** — SPEC: CPython 3.12+ (PEP 683, confirmed in v3.12/v3.13 object.h) inits every static object via PyObject_HEAD_INIT to {_Py_IMMORTAL_REFCNT} so Py_DECREF is a permanent no-op. MOLT diverges three ways, all mortal: (a) include/Python.h:153 `#define PyObject_HEAD_INIT(type) 1,(type),` -> numpy's statically-declared type objects (PyVarObject_HEAD_INIT) get ob_refcnt=1; (b) abi_types.rs:724+ builtin Py*_Type statics are std::mem::zeroed (ob_refcnt=0) and init_static_types (abi_types.rs:821) sets tp_name/tp_flags but NEVER ob_refcnt, and PyType_Ready (typeobj.rs:46) does NOT bump refcnt to immortal (verified: only PyType_FromSpecWithBases sets refcnt, to 1); (c) exc_singletons macro (abi_types.rs:1030) hardcodes ob_refcnt:1 with ob_type left null. Molt's own Py_DECREF (refcount.rs:39) treats immortal iff ob_refcnt>=1<<29, so all three are MORTAL: a net-negative decref reaches 0 and calls release_pyobj/tp_dealloc on a statically-allocated object = memory corruption (the O!-header-clobber class). numpy carries benign refcount imbalances on static type/dtype/exception objects that are safe ONLY because CPython makes them immortal. Also an internal-inconsistency: molt uses 4 different 'immortal' encodings (1, 0, 1<<30, header-macro) with no single authority.

2. **VERSION_UNGATED** — SPEC: CPython 3.13+ free-threaded build (v3.13.0 object.h, #ifdef Py_GIL_DISABLED) replaces the whole header: struct _object { uintptr_t ob_tid; uint16_t _padding; PyMutex ob_mutex; uint8_t ob_gc_bits; uint32_t ob_ref_local; Py_ssize_t ob_ref_shared; PyTypeObject *ob_type; } with immortal = ob_ref_local==UINT32_MAX. MOLT hardcodes only the with-GIL layout (Python.h:144 comment 'matches CPython 3.12 non-Py_GIL_DISABLED', abi_types.rs:50) with NO #ifdef Py_GIL_DISABLED branch and no rejection/gating. A cp313t/cp314t extension compiled against a real free-threaded header would see every field at the wrong offset (ob_refcnt@0 vs ob_ref_local@>=16) = silent memory corruption. Currently out of the GIL-only verified subset (M02), but nothing refuses it. Note molt DOES define PyMutex (abi_types.rs:63) but never wires it into the object header.

3. **WRONG_SEMANTICS** — SPEC (v3.12/v3.13): immortal ob_refcnt = _Py_IMMORTAL_REFCNT = UINT_MAX (0xFFFFFFFF) on 64-bit, (UINT_MAX>>2)=0x3FFFFFFF on 32-bit; _Py_IsImmortal = (int32)ob_refcnt<0 (64-bit) / ==0x3FFFFFFF (32-bit). MOLT inits singletons ob_refcnt=1<<30=0x40000000 (abi_types.rs:687+) and its extern Py_INCREF/Py_DECREF (refcount.rs:28,45) treat ob_refcnt>=1<<29 as immortal. Self-consistent ONLY because molt's include/Python.h makes Py_INCREF/Py_DECREF EXTERN function calls (Python.h:874-892), not the CPython inline macros, so source-compiled extensions route through molt's runtime. Divergence bites any prebuilt/native extension compiled against genuine CPython 3.12 headers: its inlined Py_INCREF/Py_DECREF read ob_refcnt directly and would NOT recognize molt's 1<<30 as immortal (0x40000000 sign-bit clear; !=0x3FFFFFFF) -> Py_None etc get decremented toward _Py_Dealloc. Bounded for the all-source WASM witness; a latent corruption vector for any native/prebuilt-.so path.

4. **MISSING** — MOLT include/Python.h defines only _Py_IMMORTAL_REFCNT_LOCAL (=1<<30, Python.h:120) and _Py_IMMORTAL_INITIAL_REFCNT (Python.h:121); it does NOT define canonical _Py_IMMORTAL_REFCNT, the _Py_IsImmortal() inline (present in every CPython >=3.12 object.h), nor 3.14's public Py_IsImmortal(). Cython 3.x and numpy 2.x reference _Py_IsImmortal/Py_IsImmortal; such a source-compiled extension fails to compile (undefined symbol/macro) = build break. No runtime Rust export named _Py_IsImmortal exists either (grep clean).

5. **VERSION_UNGATED** — SPEC (v3.14 refcount.h): _Py_IMMORTAL_REFCNT changed to (3ULL<<30)=0xC0000000 (64-bit) / (5L<<28)=0x50000000 (32-bit); added _Py_IMMORTAL_INITIAL_REFCNT and _Py_STATIC_IMMORTAL_INITIAL_REFCNT (static objects tagged with _Py_STATIC_FLAG_BITS<<48 in the high word); _Py_IsImmortal 32-bit became `ob_refcnt >= _Py_IMMORTAL_MINIMUM_REFCNT` (threshold); refcount macros moved to a separate Include/refcount.h. MOLT is pinned to 3.12 (Python.h:72-78 PY_MINOR_VERSION 12, PY_VERSION "3.12.0 (Molt runtime)", Py_Version static 0x030c00f0 abi_types.rs:653) with a single fixed object-header/refcount definition and NO per-target-version gating, despite the '>=3.12 version-gated within verified subset' contract (M02). Non-GIL 3.14 layout is itself unchanged so extern-routed refcounting still works, but molt's advertised version and immortal constants are stale for 3.14 targets.

6. **WRONG_SEMANTICS** — SPEC: CPython 3.12+ Py_SET_REFCNT no-ops on immortals (`if (_Py_IsImmortal(op)) return;`) so it cannot mortalize a shared static. MOLT include/Python.h:1657 defines `Py_SET_REFCNT(ob,refcnt) (Py_REFCNT(ob)=(refcnt))` — an unconditional store that can overwrite an immortal singleton's refcount, defeating molt's own >=1<<29 immortal detection. Rare in numpy hot paths; low blast radius.

7. **MATCHES** — abi_types.rs:50-57 (ob_refcnt: isize, ob_type: *mut PyTypeObject) == include/Python.h:145-160 == CPython 3.12/3.13 non-GIL layout. Field order/type/offset correct: ob_refcnt@0, ob_type@4 (wasm32) / @8 (native); sizeof 8/16. Py_ssize_t=intptr_t (Python.h:90) = Rust isize: 4B on wasm32 (matches CPython ILP32), 8B on native — width correct on both. Gated at C compile time by _molt_abi_layout.generated.h:50-58 _Static_assert (sizeof + offsetof), regenerated from the single Rust authority; drift fails to compile.

8. **MATCHES** — abi_types.rs:72-75 == Python.h:149-164 == CPython 3.12. ob_size@8 (wasm32)/@16 (native); sizeof 12/24. Gated by _molt_abi_layout.generated.h:60-67. Py_SIZE (Python.h:1656) reads ob_size correctly.

9. **MATCHES** — SPEC (v3.12/v3.13): ob_refcnt is `union { Py_ssize_t ob_refcnt; PY_UINT32_T ob_refcnt_split[2]; }` where split exists only when SIZEOF_VOID_P>4 (used by CPython's inlined 64-bit Py_INCREF saturating path). MOLT uses a plain Py_ssize_t (abi_types.rs:53) — same size/offset, so layout is binary-identical; on wasm32 (the primary target) CPython's union has NO split member either, so it is byte-for-byte the same. Because molt makes Py_INCREF extern (not inlined), source-compiled extensions never reference ob_refcnt_split, so its absence is invisible. Layout MATCHES; only a prebuilt 64-bit .so using the inlined split path would notice (out of the WASM subset).

10. **MATCHES** — SPEC: _PyObject_HEAD_EXTRA expands to _ob_next/_ob_prev ONLY under Py_TRACE_REFS (debug); default/release builds define it empty. MOLT omits it entirely (Python.h PyObject_HEAD is just ob_refcnt+ob_type) — matches the non-TRACE_REFS builds numpy/scipy wheels ship. Molt has no Py_TRACE_REFS support, which is acceptable for the release-ABI target.

11. **MATCHES** — PyObject widest member is 4B (Py_ssize_t/pointer) on wasm32, so struct alignment is 4B (native 8B) — matching CPython. The historic clobber class is now handled: _molt_abi_layout.generated.h pins ob_type@4 (32-bit) and the bridge reads the trailing handle via core::ptr::read_unaligned (bridge.rs:602+ '# Alignment' doc) precisely because a statically-declared C PyObject on wasm32 lands only 4-aligned and its post-header trailer is likewise 4-aligned. No 8-byte-alignment assumption remains in the object-header read path.

---

### PyTypeObject struct, tp_flags & sub-table field order

*10 items — 6 gap(s), 4 MATCHES. The `note` column is expanded verbatim in the numbered detail directly below the table.*

| # | item | status | sev | vΔ | needed_by |
|---|------|--------|-----|----|-----------|
| 1 | Missing Py_TPFLAGS_* macro defines in the ABI header: *_SUBCLASS fast-check flags LONG(1<<24)/LIST(1<<25)/TUPLE(1<<26)/BYTES(1<<27)/DICT(1<<29)/TYPE(1<<31), plus DISALLOW_INSTANTIATION(1<<7), IMMUTABLETYPE(1<<8), SEQUENCE(1<<5), MAPPING(1<<6), MANAGED_WEAKREF(1<<3), VALID_VERSION_TAG(1<<19), ITEMS_AT_END(1<<23), READYING(1<<13) | MISSING | high | — | numpy/scipy/Cython C-extension compilation (PyType_Spec flag fields, PyType_HasFeature/PyType_FastSubclass call sites) |
| 2 | Builtin static type objects (PyLong_Type, PyUnicode_Type, PyTuple_Type, ...) initialised with tp_flags = Py_TPFLAGS_READY only | WRONG_SEMANTICS | high | — | numpy _multiarray_umath scalar-type registry (fast PyLong/PyUnicode/PyTuple checks, subtype creation) |
| 3 | PyType_FromSpec/FromModuleAndSpec results are not flagged Py_TPFLAGS_HEAPTYPE and are allocated as a bare Box<PyTypeObject>, never a PyHeapTypeObject | WRONG_SEMANTICS | high | — | numpy/scipy heap types via PyType_FromModuleAndSpec / PyType_FromMetaclass |
| 4 | PyTypeObject frozen at 3.12 layout; 3.13+ tail field uint16_t tp_versions_used (after tp_watched) absent and layout is not version-gated | VERSION_UNGATED | med | yes | extensions/tests targeting CPython 3.13/3.14 within the M02 verified subset |
| 5 | Rust constant Py_TPFLAGS_DEFAULT = Py_TPFLAGS_BASETYPE (1<<10) | WRONG_SEMANTICS | low | — | — |
| 6 | Immortal refcount sentinel = 1<<30 (_Py_IMMORTAL_REFCNT_LOCAL and Py_None/Py_True/Py_False/sentinel statics) vs CPython 3.12 _Py_IMMORTAL_REFCNT (UINT_MAX on 64-bit) | WRONG_SEMANTICS | low | — | — |
| 7 | PyTypeObject full field order + offsets (ob_base..tp_name..tp_as_number/sequence/mapping..tp_richcompare..tp_getset/members/methods..tp_dictoffset/weaklistoffset..tp_new/init/alloc/free/dealloc..tp_version_tag..tp_vectorcall..tp_watched) | MATCHES | low | — | — |
| 8 | tp_as_number/tp_as_sequence/tp_as_mapping/tp_as_async/tp_as_buffer protocol sub-table field order (PyNumberMethods 35 fields incl nb_reserved slot, PySequenceMethods 10 incl was_sq_slice/was_sq_ass_slice placeholders, PyMappingMethods 3, PyAsyncMethods am_await/aiter/anext/send, PyBufferProcs bf_getbuffer/releasebuffer) | MATCHES | low | — | — |
| 9 | Defined tp_flags bit values (HEAPTYPE 1<<9, BASETYPE 1<<10, HAVE_VECTORCALL 1<<11, READY 1<<12, HAVE_GC 1<<14, METHOD_DESCRIPTOR 1<<17, HAVE_VERSION_TAG 1<<18, IS_ABSTRACT 1<<20, MANAGED_DICT 1<<4, UNICODE_SUBCLASS 1<<28, BASE_EXC_SUBCLASS 1<<30, DEFAULT 0) | MATCHES | low | — | — |
| 10 | tp_version_tag typed c_ulong (Rust) vs unsigned int (CPython spec / molt C header) | MATCHES | low | — | — |

**Notes — PyTypeObject struct, tp_flags & sub-table field order:**

1. **MISSING** — grep across the whole include/ tree returns ZERO occurrences of any of these. molt/include/Python.h is the sole Python.h the witness compiles numpy/scipy/Cython against (no CPython header fallback), so any extension/Cython-generated source that references one of these public CPython 3.12 flag macros fails to compile with 'undeclared identifier'. The 3.12 IMMUTABLETYPE/DISALLOW_INSTANTIATION and the SEQUENCE/MAPPING match-protocol flags are commonly emitted by Cython 3.x and numpy 2.x type tables. _(verify: PLAUSIBLE)_

2. **WRONG_SEMANTICS** — abi_types.rs:822-826 set_name! macro sets tp_flags = Py_TPFLAGS_READY and nothing else; PyType_Ready only ORs READY back in. CPython's PyLong_Type etc. carry DEFAULT|BASETYPE|<TYPE>_SUBCLASS|READY. Consequence: PyType_FastSubclass(&PyLong_Type, Py_TPFLAGS_LONG_SUBCLASS)==0, PyType_HasFeature(&PyLong_Type, Py_TPFLAGS_BASETYPE)==0, so any extension using the flag-based fast checks or testing subclassability of a builtin gets the wrong answer (int_check fast-path miss, refuse-to-subclass). molt's own PyLong_Check etc. are extern runtime functions so they dodge this, but numpy's inlined feature tests do not. _(verify: CONFIRMED)_

3. **WRONG_SEMANTICS** — PyType_FromSpecWithBases (typeobj.rs:1081-1134) does ty.tp_flags = spec.flags & !READY and Box::new(PyTypeObject) — it never ORs Py_TPFLAGS_HEAPTYPE (CPython's type_from_spec always does) and never allocates the larger PyHeapTypeObject (as_number/as_mapping/as_sequence/as_buffer/ht_name/ht_qualname/ht_module/_spec_cache inline; declared in include/Python.h:484-498 but with NO Rust repr(C) mirror and NO _Static_assert gate). Any consumer that treats a Py_TPFLAGS_HEAPTYPE type as PyHeapTypeObject (ht_name for __name__, ht_module for per-module state) reads past the 416-byte allocation. PyType_FromModuleAndSpec additionally ignores its module argument (typeobj.rs:1139-1145), dropping module state PyType_GetModuleState relies on. _(verify: CONFIRMED)_

4. **VERSION_UNGATED** — 3.13 Include/cpython/object.h adds `uint16_t tp_versions_used` after tp_watched (attribute-cache accounting); 3.14 keeps it. molt's PyTypeObject (abi_types.rs) and include/Python.h:421-476 end at tp_watched with no cfg/version gate, and Py_Version is hard-pinned 0x030c00f0 (3.12.0). Under M02 (target-python >=3.12 is the gating authority) a 3.13/3.14 target still gets the 3.12-sized struct; the generated _Static_assert sizeof==416 would reject a genuine 3.13 header, so extensions built against 3.13+ headers are silently forced to 3.12 shape. Safe only because the witness compiles everything against this 3.12 header; a precompiled 3.13 wheel would mismatch. _(verify: CONFIRMED)_

5. **WRONG_SEMANTICS** — abi_types.rs:649 defines the Rust-side Py_TPFLAGS_DEFAULT as BASETYPE (1<<10), disagreeing with molt's own C header (include/Python.h:502 Py_TPFLAGS_DEFAULT==0) and with CPython 3.12 (0). The C header — the ABI authority extensions compile against — is correct; the Rust const is a latent duplicate-authority drift currently referenced only in a doc comment (typeobj.rs:115), not in any live flag computation, and is NOT covered by the _molt_abi_layout drift gate (which only pins struct layout, not flag constants). Would silently miscompile any future Rust path that sets a type's flags from this constant. _(verify: CONFIRMED)_

6. **WRONG_SEMANTICS** — include/Python.h:120 and the Rust statics (abi_types.rs:686-718) both use 1<<30; refcount.rs treats ob_refcnt>=1<<29 as immortal in Py_INCREF/Py_DECREF. Because the header declares Py_INCREF/Py_DECREF as extern out-of-line functions (Python.h:874-892, not inlined macros), immortality is decided entirely inside molt's runtime and is self-consistent for extensions compiled against molt's header. Divergence from CPython's sentinel only bites objects shared with code that inlines CPython's own _Py_IsImmortal (int32(ob_refcnt)<0) test — i.e. precompiled-against-real-CPython wheels — which is outside molt's compile-from-source witness model. _(verify: CONFIRMED)_

7. **MATCHES** — Field-by-field verified vs CPython 3.12 Include/cpython/object.h struct _typeobject. All 48 tp_* fields present in exact spec order (abi_types.rs:271-329). Layout is machine-pinned: include/_molt_abi_layout.generated.h _Static_asserts sizeof(PyTypeObject)==416 (LP64/LLP64) / 208 (wasm32) and every field offsetof (tp_flags@168, tp_dictoffset@288, tp_version_tag@384, tp_vectorcall@400, tp_watched@408 on 64-bit) against the Rust authority, so a reorder/offset drift fails to compile. _(verify: CONFIRMED)_

8. **MATCHES** — abi_types.rs:552-625 field order is 1:1 with include/Python.h:226-296 and CPython 3.12 Include/cpython/object.h. Order is load-bearing for PyType_FromSpec slot placement (Py_nb_*/sq_*/mp_*/am_*/bf_*). nb_reserved (ex-nb_long) kept at slot 17. _(verify: CONFIRMED)_

9. **MATCHES** — Every tp_flags macro that IS defined in include/Python.h:500-516 matches CPython 3.12 Include/object.h exactly, including the 3.12-correct Py_TPFLAGS_DEFAULT==0 (3.12 dropped HAVE_VERSION_TAG from DEFAULT; verified against python/cpython v3.12.0) and Py_TPFLAGS_MANAGED_DICT==1<<4. _(verify: CONFIRMED)_

10. **MATCHES** — abi_types.rs:325 declares tp_version_tag: c_ulong; CPython 3.12 and molt's own include/Python.h:472 use `unsigned int`. Layout is nonetheless byte-identical on every target: on LP64 the 8-byte c_ulong overlaps the spec's 4-byte int + 4-byte tail padding, so tp_finalize stays at offset 392 and the generated sizeof==416/offset _Static_asserts pass; on LLP64/wasm32 both are 4 bytes. Benign because molt keeps no attribute-version cache (never reads/writes a meaningful value), and low 4 bytes coincide on little-endian. Should still be c_uint for type-correctness. _(verify: CONFIRMED)_

---

### Protocol method tables & descriptor structs

*13 items — 3 gap(s), 10 MATCHES. The `note` column is expanded verbatim in the numbered detail directly below the table.*

| # | item | status | sev | vΔ | needed_by |
|---|------|--------|-----|----|-----------|
| 1 | PyAsyncMethods.am_send — sendfunc signature | WRONG_SEMANTICS | med | — | native async-generator/coroutine extensions (e.g. Cython async def); numpy/scipy do NOT exercise it |
| 2 | PyNumberMethods/PySequenceMethods/PyMappingMethods/PyAsyncMethods/PyBufferProcs — missing static_assert layout gate | MISSING | low | — | numpy/scipy/Cython PyType_FromSpec + PyType_Ready slot inheritance |
| 3 | Py_RELATIVE_OFFSET (PyMemberDef flag, value 8) | MISSING | low | yes | stable-ABI heap types declaring relative-offset PyMemberDef via PyType_FromSpec; not used by numpy/scipy |
| 4 | PyNumberMethods (nb_* × 36) | MATCHES | high | — | numpy scalar/dtype arithmetic, ufunc operand coercion |
| 5 | PySequenceMethods (sq_* × 10) | MATCHES | high | — | numpy nditer / sequence-protocol paths, PySequence_* on array types |
| 6 | PyMappingMethods (mp_* × 3) | MATCHES | high | — | numpy __getitem__/__setitem__ (mp_subscript/mp_ass_subscript on ndarray) |
| 7 | PyBufferProcs (bf_getbuffer, bf_releasebuffer) | MATCHES | high | — | numpy PEP-3118 buffer export to scipy / memoryview / Cython typed memoryviews |
| 8 | Py_buffer (buffer descriptor, 11 fields) | MATCHES | high | — | numpy/scipy buffer protocol — the descriptor bf_getbuffer fills; task-flagged nasty-bug struct |
| 9 | PyMethodDef (ml_name, ml_meth, ml_flags, ml_doc) + METH_* flags | MATCHES | high | — | every extension's tp_methods and module-level method tables (numpy _multiarray_umath, Cython) |
| 10 | PyMemberDef (name, type, offset, flags, doc) | MATCHES | high | — | numpy tp_members struct-member descriptors (PyType_Ready → member_descriptor) |
| 11 | PyGetSetDef (name, get, set, doc, closure) + getter/setter | MATCHES | high | — | numpy tp_getset computed-attribute descriptors (every dtype/generic-alias type) |
| 12 | Protocol slot IDs (typeslots.h Py_bf_/mp_/nb_/sq_/am_) | MATCHES | high | — | numpy PyType_FromSpec — every dtype/ufunc/scalar heap type is built from a Py_* slot array |
| 13 | PyMemberDef T_* member-type codes + member flags | MATCHES | med | — | member_descriptor get/set on numpy types that expose C struct fields |

**Notes — Protocol method tables & descriptor structs:**

1. **WRONG_SEMANTICS** — runtime/molt-cpython-abi/include/Python.h:221 declares `typedef PyObject *(*sendfunc)(PyObject*, PyObject*, int*)`. CPython 3.12 (Include/cpython/object.h) is `PySendResult (*sendfunc)(PyObject*, PyObject*, PyObject**)`. Two divergences: return type (PyObject* = 8B ptr on LP64 vs PySendResult enum = 4B int) and 3rd param (int* vs PyObject** out-ptr). Struct LAYOUT is unaffected (am_send is still one pointer-sized slot at field index 3, offset matches); the bug is calling-convention: if molt's runtime ever calls am_send it reads a 4-byte enum return as an 8-byte pointer and writes a PyObject* through a slot the header typed int*. Field-order/offset of the PyAsyncMethods table itself MATCHES (abi_types.rs:614-619, Python.h:358-363; slot IDs Py_am_await/aiter/anext/send = 77/78/79/81 correct).

2. **MISSING** — The five protocol method tables have NO entries in include/_molt_abi_layout.generated.h and are not emitted by tools/gen_cpython_abi_layout.py — unlike Py_buffer, PyMethodDef, PyMemberDef, PyGetSetDef which are all gated. Because every field is pointer-sized, a field REORDER (the exact O!-header-clobber / wrong-offset bug class the audit warns about) leaves sizeof identical, so it would NOT fail the C compile-time _Static_assert. Only the hand-written 'field order is load-bearing' comment (abi_types.rs:549-551) guards it. Current order MATCHES spec, but the drift gate the rest of the ABI relies on is absent here.

3. **MISSING** — runtime/molt-cpython-abi/include/Python.h:540-542 defines Py_READONLY=1, Py_AUDIT_READ=2, _Py_WRITE_RESTRICTED=4 but omits Py_RELATIVE_OFFSET=8, which CPython 3.12 added in Include/descrobject.h. A source-recompiled extension using relative member offsets would fail to compile (undefined macro). Marginal for the verified numpy/scipy/Cython subset (they use absolute offsets).

4. **MATCHES** — abi_types.rs:553-590 and include/Python.h:226-263 are 1:1 with CPython 3.12 Include/cpython/object.h: 36 fields nb_add..nb_inplace_matrix_multiply, nb_reserved placeholder at index 17. C-header field types (binaryfunc/ternaryfunc/unaryfunc/inquiry) correct. Slot IDs Py_nb_* = 6-38 + 75/76 (typeobj.rs:733-806) and dispatch arms (typeobj.rs:1000-1031) place each into the correct field. Layout stable across 3.12-3.14. _(verify: CONFIRMED)_

5. **MATCHES** — abi_types.rs:593-604 / Python.h:265-276 match CPython 3.12 exactly incl. was_sq_slice and was_sq_ass_slice void* placeholders at indices 4 and 6. Types: sq_length=lenfunc, sq_item/sq_repeat=ssizeargfunc, sq_ass_item=ssizeobjargproc, sq_contains=objobjproc. Slot IDs 39-46 + dispatch (typeobj.rs:1033-1040) correct. _(verify: CONFIRMED)_

6. **MATCHES** — abi_types.rs:607-611 / Python.h:278-282 = {mp_length:lenfunc, mp_subscript:binaryfunc, mp_ass_subscript:objobjargproc}, exact. Slot IDs Py_mp_length/subscript/ass_subscript = 4/5/3 (typeobj.rs:729-731) and dispatch (typeobj.rs:1042-1044) correct. _(verify: CONFIRMED)_

7. **MATCHES** — abi_types.rs:621-625 / Python.h:307-310 correct 2-field layout. Signatures exact: getbufferproc = int(*)(PyObject*, Py_buffer*, int), releasebufferproc = void(*)(PyObject*, Py_buffer*) (Python.h:304-305). Slot IDs Py_bf_getbuffer=1/Py_bf_releasebuffer=2 (typeobj.rs:726-727) and dispatch (typeobj.rs:1051-1052) correct. PyBUF_* request flags (abi_types.rs:1250-1267, Python.h:312-329) match CPython pybuffer.h. _(verify: CONFIRMED)_

8. **MATCHES** — abi_types.rs:1273-1286 = CPython 3.12 exactly: buf, obj, len, itemsize, readonly, ndim, format, shape, strides, suboffsets, internal. GATED by static_assert for BOTH pointer widths in _molt_abi_layout.generated.h:270-297 (sizeof 80/44, every field offset pinned — e.g. readonly@32, ndim@36, format@40 on LP64). A wrong offset here (the exact O!-clobber class) fails the C compile. Correct and machine-checked. _(verify: CONFIRMED)_

9. **MATCHES** — abi_types.rs:336-341 / Python.h:562-567 exact; GATED (generated.h:174-187: sizeof 32/16, offsets ml_meth@8, ml_flags@16, ml_doc@24 on LP64). METH_VARARGS=0x1, KEYWORDS=0x2, NOARGS=0x4, O=0x8, CLASS=0x10, STATIC=0x20, COEXIST=0x40, FASTCALL=0x80, METHOD=0x200 (abi_types.rs:628-636, Python.h:552-560) all match CPython methodobject.h. _(verify: CONFIRMED)_

10. **MATCHES** — abi_types.rs:365-371 / Python.h layout exact; GATED (generated.h:206-221: sizeof 40/20, offsets type@8, offset@16, flags@24, doc@32 on LP64 — padding after the two int fields matches C). Field order {name, int type, Py_ssize_t offset, int flags, doc} identical to CPython 3.12.

11. **MATCHES** — abi_types.rs:350-356 / Python.h struct exact; GATED (generated.h:189-204: sizeof 40/20, offsets get@8, set@16, doc@24, closure@32 on LP64). Function ptr typedefs getter=PyObject*(*)(PyObject*, void*), setter=int(*)(PyObject*, PyObject*, void*) (abi_types.rs:37-39, Python.h:219-220) match CPython descrobject.h.

12. **MATCHES** — typeobj.rs:724-813 reproduces CPython 3.12 Include/typeslots.h exactly: bf 1-2, mp 3-5, nb 6-38, sq 39-46, tp 47-74, nb_matrix_multiply/inplace 75-76, am_await/aiter/anext 77-79, tp_finalize 80, am_send 81. The slot-dispatch match (typeobj.rs:993-1052) routes each ID to the correct sub-table field, and the unrecognised-id arm fails closed (RuntimeError) rather than silently dropping. A wrong ID here would land a function pointer in the wrong protocol field — none do.

13. **MATCHES** — include/Python.h:518-542 defines Py_T_SHORT=0, Py_T_INT=1, Py_T_LONG=2, Py_T_FLOAT=3, Py_T_DOUBLE=4, Py_T_STRING=5, _Py_T_OBJECT=6, Py_T_CHAR=7, Py_T_BYTE=8, Py_T_UBYTE=9, Py_T_USHORT=10, Py_T_UINT=11, Py_T_ULONG=12, Py_T_STRING_INPLACE=13, Py_T_BOOL=14, Py_T_OBJECT_EX=16 (15 deliberately skipped), Py_T_LONGLONG=17, Py_T_ULONGLONG=18, Py_T_PYSSIZET=19, _Py_T_NONE=20 — every value matches CPython 3.12 Include/descrobject.h. Legacy T_* names aliased via include/structmember.h:30-55; Py_READONLY=1, Py_AUDIT_READ=2, _Py_WRITE_RESTRICTED=4 correct.

---

### Buffer / module-def / capsule / datetime structs

*15 items — 2 gap(s), 13 MATCHES. The `note` column is expanded verbatim in the numbered detail directly below the table.*

| # | item | status | sev | vΔ | needed_by |
|---|------|--------|-----|----|-----------|
| 1 | PyDateTime_GET_YEAR/MONTH/DAY, PyDateTime_DATE_GET_HOUR/MINUTE/SECOND/MICROSECOND/FOLD/TZINFO, PyDateTime_TIME_GET_*, PyDateTime_DELTA_GET_* accessor macros + PyDateTime_Date/Time/DateTime/Delta/TZInfo C struct typedefs + PyDateTime_CAPSULE_NAME, in the public header surface | MISSING | high | — | numpy _multiarray_umath (datetime.c / convert_datetime.c: convert_pydatetime_to_datetimestruct), pandas datetime C code, Cython datetime cimports |
| 2 | Free-threaded build (Py_GIL_DISABLED) PyObject_HEAD layout underlying PyModuleDef_Base / all family headers on 3.13t/3.14t | VERSION_UNGATED | low | yes | numpy/scipy free-threaded (cp313t/cp314t) wheels, if ever targeted |
| 3 | Py_buffer FULL layout (buf/obj/len/itemsize/readonly/ndim/format/shape/strides/suboffsets/internal) | MATCHES | high | — | numpy PyArray bf_getbuffer, memoryview, Cython buffer protocol |
| 4 | PyModuleDef (m_base/m_name/m_doc/m_size/m_methods/m_slots/m_traverse/m_clear/m_free) | MATCHES | high | — | every C extension's static PyModuleDef (numpy, Cython multi-phase init) |
| 5 | PyModuleDef_Base (PyObject_HEAD/m_init/m_index/m_copy) | MATCHES | high | — | PyModuleDef_HEAD_INIT of every extension module def |
| 6 | PyModuleDef_Slot {slot:int, value:void*} | MATCHES | high | — | numpy/Cython multi-phase init slot arrays |
| 7 | Py_mod_* slot ids (Py_mod_create=1, Py_mod_exec=2, Py_mod_multiple_interpreters=3, Py_mod_gil=4) + runtime slot dispatch | MATCHES | high | yes | Cython 3.x / numpy 2.x multi-phase modules that emit multiple_interpreters + gil slots |
| 8 | PyDateTime_CAPI (5 type ptrs + TimeZone_UTC + 9 constructor fn ptrs = 15 fields) | MATCHES | high | — | numpy PyDateTime_IMPORT -> PyDateTimeAPI->DateType/Date_FromDate/etc by fixed offset; pandas |
| 9 | PyDateTime_Date/Time/DateTime/Delta/TZInfo struct layouts + data[] field packing | MATCHES | high | — | direct PyDateTime_GET_* macro reads from numpy/pandas; molt runtime allocation/dealloc |
| 10 | PyType_Spec {name/basicsize/itemsize/flags/slots} + PyType_Slot {slot/pfunc} for PyType_FromSpec | MATCHES | high | — | numpy/Cython PyType_FromSpecWithBases heap-type creation |
| 11 | PyBUF_* request-flag constants (SIMPLE/WRITABLE/FORMAT/ND/STRIDES/C_CONTIGUOUS/F_CONTIGUOUS/ANY_CONTIGUOUS/INDIRECT + composite CONTIG/RECORDS/FULL + READ/WRITE) | MATCHES | med | — | numpy/Cython PyObject_GetBuffer flag negotiation |
| 12 | PyBufferProcs {bf_getbuffer, bf_releasebuffer} (buffer.rs dispatch) | MATCHES | med | — | buffer.rs foreign_bf_getbuffer/releasebuffer dispatch to numpy PyArray_Type buffer slots |
| 13 | PyCapsuleObject layout {ob_base/pointer/name/context/destructor} | MATCHES | med | — | numpy _ARRAY_API / DATETIMEUNITS capsules, datetime.datetime_CAPI capsule, scipy cython_blas capsules |
| 14 | PyCapsule_Import contract (registry fast-path + import-walk + IsValid + pointer return) | MATCHES | med | — | numpy import_array (PyCapsule_Import _ARRAY_API), PyDateTime_IMPORT capsule path, scipy |
| 15 | datetime.h PyDateTime_IMPORT redefined to bypass PyCapsule_Import (uses a static per-TU _molt_datetime_capi_singleton) | MATCHES | low | — | source-compiled extensions calling PyDateTime_IMPORT |

**Notes — Buffer / module-def / capsule / datetime structs:**

1. **MISSING** — CPython Include/datetime.h defines these GET macros UNCONDITIONALLY as direct struct field reads (e.g. PyDateTime_GET_YEAR(o) = ((PyDateTime_Date*)o)->data[0]<<8 | data[1]) and requires the PyDateTime_Date/Time/DateTime/Delta typedefs to be visible. molt's runtime/molt-cpython-abi/include/datetime.h (whole file, 179 lines) defines the CAPI struct, the constructor macros (PyDate_FromDate etc.), and the Check inlines, but NOT the GET accessor macros, NOT the C struct typedefs, and NOT PyDateTime_CAPSULE_NAME. grep across all of molt (include/molt/Python.h + runtime include tree) finds zero definitions. A source-compiled extension (Molt's primary model: Cython/C -> WASM) that reads a datetime object's fields via these macros fails to COMPILE (undeclared identifier). The UNDERLYING layout is correct — the Rust structs (abi_types.rs:190-229) and the data[] byte-packing (datetime.rs:86-107 write_date_data/write_time_data: [year>>8, year&0xff, month, day] / [h,m,s,us>>16,us>>8,us]) exactly match CPython, and data[] lands at offset 25 for date in both — so adding the macros+typedefs is a header-only fix that will read the right bytes. A precompiled real-CPython .so is unaffected (its baked-in macros read the runtime-owned structs at matching offsets via the runtime capsule).

2. **VERSION_UNGATED** — molt hardcodes Py_GIL_DISABLED 0 (include/molt/Python.h:561) and the standard-build PyObject {ob_refcnt, ob_type}. On 3.13+ free-threaded builds PyObject_HEAD is instead {ob_tid, _padding, ob_mutex, ob_gc_bits, ob_ref_local, ob_ref_shared, ob_type} — a different size/offsets, which shifts m_init/m_index/m_copy in PyModuleDef_Base and every embedded header in this family. This is a deliberate scoping choice (standard build only) per M02, not a live bug, but it is UNGATED: nothing rejects a free-threaded-ABI extension at build time, so pairing a cp313t extension with molt would silently corrupt. Out of the strict family scope (PyObject header) but flagged because the whole family embeds PyObject_HEAD. Recommend an explicit honest-early error if the free-threaded ABI is detected.

3. **MATCHES** — abi_types.rs:1274 — 11 fields, exact CPython 3.12 order/types (void*, PyObject*, 2x Py_ssize_t, 2x int, char*, 3x Py_ssize_t*, void*). Byte-for-byte pinned by _molt_abi_layout.generated.h on BOTH widths: PTR64 sizeof=80 with buf@0/obj@8/len@16/itemsize@24/readonly@32/ndim@36/format@40/shape@48/strides@56/suboffsets@64/internal@72 (lines 285-296); PTR32/wasm32 sizeof=44 buf@0..internal@40 (272-283). Py_buffer is ABI-stable 3.9-3.14 (unchanged), so no version gating needed. This is the descriptor the Miri-C offset fix touched — the static-assert gate now guards it against silent drift. _(verify: CONFIRMED)_

4. **MATCHES** — abi_types.rs:414 — 9 fields, exact CPython order. m_traverse/m_clear/m_free typed *mut c_void vs CPython's traverseproc/inquiry/freefunc, but all pointer-sized so layout is identical on x86-64/aarch64/wasm32. Statically asserted by generated.h: PTR64 sizeof=104, m_name@40..m_free@96 (235-243); PTR32 sizeof=52, m_name@20..m_free@48 (225-233). Unchanged 3.12->3.14. _(verify: CONFIRMED)_

5. **MATCHES** — abi_types.rs:433 — {PyObject ob_base, Option<fn()->*mut PyObject> m_init, Py_ssize_t m_index, *mut PyObject m_copy}. PyObject_HEAD in the standard (non-free-threaded, non-TRACE_REFS) build is {ob_refcnt, ob_type} = 16B/8B, matching molt's PyObject. Statically asserted: PTR64 sizeof=40, m_init@16/m_index@24/m_copy@32 (264-267); PTR32 sizeof=20 (259-262). Unchanged 3.12->3.14. _(verify: CONFIRMED)_

6. **MATCHES** — abi_types.rs:427. Statically asserted PTR64 sizeof=16 slot@0/value@8 (252-254), PTR32 sizeof=8 slot@0/value@4 (249-250). Unchanged across versions. _(verify: CONFIRMED)_

7. **MATCHES** — include/molt/Python.h:567-570 defines all four ids + the Py_MOD_GIL_USED/NOT_USED and Py_MOD_MULTIPLE_INTERPRETERS_* value constants (562-566), matching CPython 3.13. Rust PY_MOD_* consts modules.rs:11-14. CRITICALLY, the slot loop (modules.rs:397 and :461) treats PY_MOD_MULTIPLE_INTERPRETERS|PY_MOD_GIL as accepted no-ops rather than falling into the `unsupported PyModuleDef slot` error arm — so a 3.13-compiled extension carrying slot 3/4 initializes instead of erroring. version_delta=true (Py_mod_gil is a 3.13 addition) but molt handles it correctly on the >=3.12 subset. Slot 3 (multiple_interpreters) predates gil; also handled. _(verify: DOWNGRADE)_

8. **MATCHES** — abi_types.rs:650 (Rust) and runtime/molt-cpython-abi/include/datetime.h:75-93 (C) both declare all 15 fields in exact CPython 3.12 order with matching signatures: Date_FromDate(i,i,i,PyTypeObject*), DateTime_FromDateAndTime(7xint,PyObject*,PyTypeObject*), Time_FromTime, Delta_FromDelta, TimeZone_FromTimeZone(PyObject*,PyObject*), DateTime_FromTimestamp/Date_FromTimestamp (DB-API), DateTime_FromDateAndTimeAndFold/Time_FromTimeAndFold (PEP 495). datetime_capi_has_exact_field_count test pins 15*ptr size. PyDateTime_CAPI has been stable since 3.6 (no field added 3.12->3.14), so no version delta. NOT in the offset static-assert gate but the field-count test + all-pointer-field homogeneity make drift low-risk. _(verify: CONFIRMED)_

9. **MATCHES** — abi_types.rs:190-229. Date {ob_base, Py_hash_t hashcode, char hastzinfo, u8 data[4]} -> data@25 (16+8+1); Time adds data[6]+u8 fold+PyObject* tzinfo -> fold@31, tzinfo@32; DateTime data[10]+fold+tzinfo; Delta {hashcode,days,seconds,microseconds}; TZInfo {ob_base}. All match CPython 3.12 datetime.h struct macros (_PyTZINFO_HEAD + data[_PyDateTime_*_DATASIZE], DATASIZE 4/6/10). Byte packing verified matches CPython macro reads: write_date_data (datetime.rs:86) = [year>>8, year&0xff, month, day]; write_time_data (:93) = [hour, min, sec, us>>16, us>>8, us&0xff]. So PyDateTime_GET_YEAR etc. read the right bytes IF the macros existed (see MISSING entry). Runtime-owned + not in static gate; unchanged 3.12->3.14.

10. **MATCHES** — abi_types.rs:516 (PyType_Slot {int slot, void* pfunc}) and :522 (PyType_Spec {const char* name, int basicsize, int itemsize, unsigned int flags, PyType_Slot* slots}). flags correctly c_uint. Statically asserted: PyType_Spec PTR64 sizeof=32 name@0/basicsize@8/itemsize@12/flags@16/slots@24 (449-454), PTR32 sizeof=20 (442-447); PyType_Slot PTR64 sizeof=16 (435-437), PTR32 sizeof=8 (431-433). Unchanged 3.12->3.14.

11. **MATCHES** — abi_types.rs:1250-1267 uses the IDENTICAL macro expressions as CPython pybuffer.h: STRIDES=0x10|ND (=0x18), C_CONTIG=0x20|STRIDES (=0x38), F_CONTIG=0x40|STRIDES (=0x58), ANY_CONTIG=0x80|STRIDES (=0x98), INDIRECT=0x100|STRIDES, RECORDS=STRIDES|FORMAT|WRITABLE (=0x1D), etc. All correct. Minor: PyBUF_STRIDED (0x19) and PyBUF_STRIDED_RO (0x18) are not defined on the Rust side, but they are unreferenced by the Rust buffer logic (flags originate in the C extension), so this is harmless — verify include/molt/Python.h exposes them for C consumers if any use them.

12. **MATCHES** — abi_types.rs:622 — 2 pointer-sized fields in CPython order (bf_getbuffer@0, bf_releasebuffer@8/4). buffer.rs:415-446 reads tp_as_buffer->bf_getbuffer at offset 0 and bf_releasebuffer at offset 8, matching CPython. Not in the static-assert gate (2-pointer struct, low drift risk), but layout is trivially correct.

13. **MATCHES** — abi_types.rs:146 — {PyObject ob_base, void* pointer, const char* name, void* context, PyCapsule_Destructor destructor}, exact match to CPython's private Objects/capsule.c PyCapsuleObject. destructor = Option<extern fn(*mut PyObject)> matches typedef void (*PyCapsule_Destructor)(PyObject*). NOT covered by the static-assert gate (capsule.rs reads fields directly at capsule.rs:281 (*obj.cast::<PyCapsuleObject>()).pointer). Layout is correct by manual offset (pointer@16/24-align chain), but flag: adding PyCapsuleObject to tools/gen_cpython_abi_layout.py would close the last direct-field-access struct not guarded against drift. Capsule struct is private/opaque in CPython so extensions use the getter functions, reducing real-world risk.

14. **MATCHES** — capsule.rs:238 — checks the process-local capsule registry first, then on miss runs CPython's real protocol: import first dotted component, getattr the rest, PyCapsule_IsValid, return (*capsule).pointer; honest ImportError/AttributeError on failure. PyCapsule_IsValid (capsule.rs:159) correctly gates on non-NULL pointer + name match like CPython. The datetime CAPI is pre-registered at ABI init (datetime.rs:718 register_datetime_capi -> PyCapsule_New) so the fast path resolves 'datetime.datetime_CAPI' — round-trip covered by datetime_capi_capsule_roundtrips test.

15. **MATCHES** — datetime.h:120-126 makes PyDateTime_IMPORT set PyDateTimeAPI = &_molt_datetime_capi_singleton (a file-static CAPI wired to molt's real symbols) instead of CPython's PyCapsule_Import(PyDateTime_CAPSULE_NAME,0). Functionally correct (the singleton points at the same runtime constructors/types the capsule would yield), and the runtime ALSO registers the real capsule for precompiled .so paths. Divergence is benign but means the header path and capsule path are two authorities for the same CAPI — keep them in lockstep. Not a layout/ABI defect.

---

### Exported data symbols (singletons, type objects, exceptions)

*16 items — 12 gap(s), 4 MATCHES. The `note` column is expanded verbatim in the numbered detail directly below the table.*

| # | item | status | sev | vΔ | needed_by |
|---|------|--------|-----|----|-----------|
| 1 | PyExc_* hierarchy completeness (20 canonical 3.12 symbols absent: PyExc_ReferenceError, PyExc_StopAsyncIteration, PyExc_BlockingIOError, PyExc_ChildProcessError, PyExc_ConnectionAbortedError, PyExc_ConnectionRefusedError, PyExc_InterruptedError, PyExc_ProcessLookupError, PyExc_TabError, PyExc_IndentationError, PyExc_UnicodeTranslateError, PyExc_BytesWarning, PyExc_EncodingWarning, PyExc_PendingDeprecationWarning, PyExc_ResourceWarning, PyExc_SyntaxWarning, PyExc_UnicodeWarning, PyExc_BaseExceptionGroup, PyExc_EnvironmentError, PyExc_WindowsError) | MISSING | high | yes | numpy/scipy/Cython (weakref->ReferenceError, async->StopAsyncIteration, io->BlockingIOError, compile->IndentationError/TabError, warnings->ResourceWarning/SyntaxWarning) |
| 2 | _Py_NotImplementedStruct (canonical NotImplemented singleton) | MISSING | high | — | numpy/scipy rich-compare (Py_RETURN_NOTIMPLEMENTED), Cython, any ext compiled against real CPython headers |
| 3 | _Py_EllipsisObject (canonical Ellipsis singleton) | WRONG_NAME | high | — | numpy advanced indexing (arr[...]), Cython, real-CPython-header extensions |
| 4 | _Py_NoneStruct / _Py_TrueStruct / _Py_FalseStruct (canonical singletons) unreconciled with runtime's live Py_None/Py_True/Py_False | WRONG_SEMANTICS | high | — | numpy/scipy/Cython object identity (is None / is True) across header boundary; real-CPython-header TUs |
| 5 | _Py_TrueStruct / _Py_FalseStruct (and molt Py_True/Py_False) object layout | LAYOUT_MISMATCH | high | — | numpy/scipy integer coercion of Python bool (PyLong_AsLong / direct ob_digit read) |
| 6 | Exception singleton refcount not immortal (ob_refcnt: 1) | WRONG_SEMANTICS | med | — | any extension that decrefs a borrowed exception ref (numpy/scipy teardown, error-path cleanup) |
| 7 | PyExc_* are bare PyObject sentinels, not PyTypeObject (ob_type=NULL) | WRONG_SEMANTICS | med | — | numpy defining custom exception subclasses (PyErr_NewException base), PyType_FastSubclass(PyExc_Exception, ...) |
| 8 | Builtin type-object data symbols are zero-initialized shells (PyLong_Type, PyFloat_Type, PyUnicode_Type, PyList_Type, PyDict_Type, PyTuple_Type, PyBool_Type, PySet_Type, ...) | WRONG_SEMANTICS | med | — | numpy scalar type hierarchy (tp_base=&PyLong_Type/&PyFloat_Type), PyType_IsSubtype, direct tp_flags reads |
| 9 | Immortal refcount sentinel value (1<<30) vs CPython _Py_IMMORTAL_REFCNT (data-symbol singletons) | VERSION_UNGATED | med | yes | prebuilt CPython 3.12+ wheels (inline Py_INCREF/Py_DECREF) |
| 10 | PySuper_Type | MISSING | med | — | extensions calling super() from C / referencing &PySuper_Type |
| 11 | Py_None / Py_True / Py_False exported as primary (non-canonical) data symbols with inverted macro | WRONG_NAME | med | — | cross-toolchain object identity; CPython-ABI contract fidelity |
| 12 | PyTypeObject.tp_version_tag field C type (c_ulong vs unsigned int) (data-symbol path) | LAYOUT_MISMATCH | low | — | numpy/scipy type attribute cache reads (rare) |
| 13 | PyTypeObject full struct layout (50 fields through tp_watched) (data-symbol path) | MATCHES | low | yes | numpy/scipy/Cython PyType_Ready, static type definitions, slot access |
| 14 | PyObject / PyVarObject header layout (data-symbol path) | MATCHES | low | — | every extension (Py_REFCNT, Py_TYPE, Py_SIZE) |
| 15 | Py_NotImplementedSentinel / Py_EllipsisObject / PyDateTime_TimeZone_UTC_Object (molt-name live sentinels) | MATCHES | low | — | molt-header-compiled numpy/scipy |
| 16 | Present PyExc_* set (~48, e.g. PyExc_Exception, ValueError, TypeError, OSError family, LookupError chain) — hierarchy edges | MATCHES | low | — | numpy/scipy PyErr_SetString / PyErr_GivenExceptionMatches |

**Notes — Exported data symbols (singletons, type objects, exceptions):**

1. **MISSING** — abi_types.rs:1074 exc_singletons! macro lists only ~48 of CPython 3.12's ~68 PyExc_ data symbols. Verified absent from src/ AND src/molt/_wasm_abi_generated.py. Any extension referencing one gets an undefined data symbol (wasm-ld link fail) or undeclared-identifier compile error. Import-gated: only trips builds that reference the missing name, but Cython-generated code references a broad PyExc_ set.

2. **MISSING** — CPython: `#define Py_NotImplemented (&_Py_NotImplementedStruct)`. molt only provides non-canonical `Py_NotImplementedSentinel` (abi_types.rs:708) and its header maps `#define Py_NotImplemented (&Py_NotImplementedSentinel)` (include/Python.h:869). The canonical symbol `_Py_NotImplementedStruct` is defined nowhere and is absent from the wasm registry (contrast: molt DID add _Py_NoneStruct/_Py_TrueStruct/_Py_FalseStruct). A real-CPython-header TU referencing Py_NotImplemented -> undefined `_Py_NotImplementedStruct` -> link fail.

3. **WRONG_NAME** — CPython symbol is `_Py_EllipsisObject`; molt exports non-canonical `Py_EllipsisObject` (abi_types.rs:715) and header maps `#define Py_Ellipsis (&Py_EllipsisObject)` (include/Python.h:870). Missing leading underscore -> a TU compiled against real CPython headers references `_Py_EllipsisObject` -> undefined -> link fail. Not in the generated registry.

4. **WRONG_SEMANTICS** — object.rs:3170-3187 defines _Py_NoneStruct/_Py_TrueStruct/_Py_FalseStruct as SEPARATE statics (each ob_type=NULL) from the runtime's canonical Py_None/Py_True/Py_False (abi_types.rs:686-701). Verified: they are never patched by init_static_types (only Py_None/True/False are, abi_types.rs:901-903), never registered in the bridge (register_static_abi_objects/exc_singleton_ptrs/type_static_ptrs exclude them; pyobj_to_handle_static bridge.rs:586-599 recognizes only the abi_types trio), and no loader sync exists. Runtime hands out &Py_None (abi_types); an extension resolving None via _Py_NoneStruct gets a different address the runtime treats as unknown foreign (`is None` returns False) AND whose Py_TYPE() is NULL -> null-deref on Py_TYPE(Py_None)->tp_name. describe_unresolved_pyobject (abi_types.rs:1216) documents this exact 'duplicated cpython-abi data sentinel' failure. Header also inverts the canonical relationship: exports `Py_None` as a real data symbol and defines `#define Py_None (&Py_None)` (include/Python.h:800,866) where CPython has no Py_None symbol.

5. **LAYOUT_MISMATCH** — CPython (Objects/boolobject.c): `struct _longobject _Py_TrueStruct = { PyObject_HEAD_INIT(&PyBool_Type) {TRUE_TAG,{1}} }` — a value-carrying PyLongObject (16B wasm32 / 24-32B native, ob_type=&PyBool_Type). molt lays them out as bare PyObject (8B wasm32 / 16B native) with no lv_tag/ob_digit (object.rs:3177-3187, abi_types.rs:692-701). An extension reading `((PyLongObject*)Py_True)->long_value.ob_digit[0]` reads past the object into adjacent static memory (OOB) -> garbage int value; the object.rs canonical copies additionally have ob_type=NULL. molt-name Py_True/Py_False do get ob_type=&PyBool_Type but are still not PyLongObject-shaped.

6. **WRONG_SEMANTICS** — abi_types.rs:1029-1032 inits every PyExc_* singleton with ob_refcnt=1. molt's own Py_DECREF (refcount.rs:44-51) treats rc>=1<<29 as immortal but rc=1 is NOT immortal -> a single Py_DECREF(PyExc_X) drops 1->0 -> release_pyobj() unregisters the singleton from the bridge (they ARE registered via register_static_abi_objects, abi_types.rs:940) so subsequent PyErr_SetString(PyExc_X) identity resolution fails. CPython guarantees these are immortal (huge/immortal refcnt). Inconsistent with the other singletons (1<<30).

7. **WRONG_SEMANTICS** — abi_types.rs:1029 makes each PyExc_* a `PyObject{ob_refcnt, ob_type:NULL}`. In CPython PyExc_* are PyObject* pointing to real exception TYPE objects. Any extension dereferencing them as a type — PyType_FastSubclass macro reads ((PyTypeObject*)PyExc_Exception)->tp_flags at offset 84/168, or PyErr_NewException reads base->tp_dict/tp_basicsize — reads OOB past the 8/16-byte sentinel and hits the NULL ob_type. Works only for pure pointer-identity paths (PyErr_SetString, GivenExceptionMatches via exc_singleton_parent walk).

8. **WRONG_SEMANTICS** — abi_types.rs:722-808 declare them `std::mem::zeroed()`; init_static_types (abi_types.rs:825 set_name! macro) sets ONLY tp_name and tp_flags=Py_TPFLAGS_READY (1<<12). Missing tp_basicsize, tp_itemsize, tp_base, tp_dict, Py_TPFLAGS_BASETYPE/DEFAULT, and every *_SUBCLASS flag (Py_TPFLAGS_LONG_SUBCLASS/LIST_SUBCLASS/etc are not even defined). PyLong_Check etc. are bridge-backed functions (numbers.rs:1559) so molt objects are shielded, and *_CheckExact use pointer-identity, but a foreign C type that inherits from these via PyType_Ready inherits null slots + tp_basicsize=0 (broken instances), and any tp_flags-bit read returns wrong. Symbol addresses are correct (register/identity paths fine).

9. **VERSION_UNGATED** — All singletons use ob_refcnt=1<<30 (0x40000000) with molt immortal threshold 1<<29 (refcount.rs:28,45), matching molt's own header macro _Py_IMMORTAL_REFCNT_LOCAL (include/Python.h:120). CPython 3.12 _Py_IMMORTAL_REFCNT = UINT_MAX (0xFFFFFFFF, 64-bit) / UINT_MAX>>2 (32-bit); its inline _Py_IsImmortal treats 0x40000000 as NOT immortal. Self-consistent only because molt ships OUT-OF-LINE Py_INCREF/Py_DECREF (include/Python.h:874-875). A TU compiled against real CPython headers uses inline refcount ops that would mutate/eventually free molt's singletons. Detection semantics also changed 3.12->3.13 (immortality) ->3.14 (free-threading ob_ref_local/shared); not version-gated.

10. **MISSING** — CPython exports `PyAPI_DATA(PyTypeObject) PySuper_Type`. Absent from molt runtime, include/Python.h, and generated registry (verified by grep). Undefined data symbol if referenced.

11. **WRONG_NAME** — molt makes Py_None/Py_True/Py_False the real exported data symbols and defines self-referential `#define Py_None (&Py_None)` (include/Python.h:800-802,866-868). CPython exports NO such symbols — the primary symbols are _Py_NoneStruct/_Py_TrueStruct/_Py_FalseStruct and Py_None is macro-only. Internally consistent for molt-header-compiled extensions (they work), but deviates from the binary contract and is the root of the dual-symbol split flagged above. Correct fix: make _Py_*Struct the single primary and `#define Py_None (&_Py_NoneStruct)`.

12. **LAYOUT_MISMATCH** — abi_types.rs:325 declares tp_version_tag: c_ulong; CPython 3.12 is `unsigned int`. BENIGN on all three target ABIs: on LP64 the 8-aligned tp_finalize absorbs the extra 4 bytes so tp_finalize@392/tp_vectorcall@400/tp_watched@408/sizeof=416 all coincide with real CPython x86-64 (verified in _molt_abi_layout.generated.h:168-171); on wasm32/LLP64 c_ulong=4=unsigned int. molt's own C header correctly declares `unsigned int tp_version_tag` (include/Python.h:472). Only effect: runtime reads 4 bytes of adjacent zero padding as high bits. Note tp_flags IS correctly c_ulong (CPython `unsigned long`).

13. **MATCHES** — Field order and offsets byte-exact vs CPython 3.12 struct _typeobject on both wasm32 (sizeof 208, tp_watched@204) and x86-64 (sizeof 416, tp_watched@408), independently recomputed and matching _molt_abi_layout.generated.h. tp_subclasses correctly void*. Pinned to 3.12; not re-verified for 3.13 (static-type immortality) / 3.14 (free-threading header changes, possible trailing fields) — molt is not version-gated on the type struct, so treat 3.13/3.14 as UNVERIFIED.

14. **MATCHES** — abi_types.rs:49-75: {ob_refcnt: Py_ssize_t, ob_type: *mut}, VarObject adds ob_size. Matches CPython (non-Py_TRACE_REFS, non-free-threaded): 16B/24B native, 8B/12B wasm32; ob_refcnt@0. wasm32 4-align preserved (widest member 4B) — the bridge.rs:571 read_unaligned trailer fix accounts for this. Molt uses signed Py_ssize_t ob_refcnt (not the ob_refcnt_split union) which is layout-identical for storage.

15. **MATCHES** — For the molt-header source-compile model these are correct: ob_type patched (init_static_types abi_types.rs:904-906) and registered in the bridge (register_static_abi_objects abi_types.rs:943-950). The defect is purely canonical-name coverage (see the _Py_NotImplementedStruct / _Py_EllipsisObject MISSING entries).

16. **MATCHES** — abi_types.rs:1074-1127 parent edges match the documented 3.12 hierarchy, gated by exception_hierarchy_matches_python_3_12 (abi_types.rs:1137). As identity sentinels for set/match paths they are correct; the shell-type and refcnt=1 issues (separate entries) are the residual risks.

---

### Calling convention (vectorcall / METH_* / call helpers)

*14 items — 4 gap(s), 10 MATCHES. The `note` column is expanded verbatim in the numbered detail directly below the table.*

| # | item | status | sev | vΔ | needed_by |
|---|------|--------|-----|----|-----------|
| 1 | PyObject_Vectorcall — kwnames path returns NULL + never reads the vectorcall slot | WRONG_SEMANTICS | high | — | Cython keyword fast-calls, numpy ufunc/vectorcall dispatch |
| 2 | PyVectorcall_Call — routes through PyObject_Call, infinite-recurses when used as tp_call | WRONG_SEMANTICS | high | — | numpy/Cython vectorcall types wiring tp_call=PyVectorcall_Call |
| 3 | PyVectorcall_Function — not implemented or exported anywhere | MISSING | high | — | Cython-generated extensions (scipy, pandas) |
| 4 | METH_METHOD / PyCMethod (defining-class) dispatch | WRONG_SEMANTICS | med | — | extensions declaring methods with METH_METHOD (PyCMethod) |
| 5 | vectorcallfunc typedef signature | MATCHES | low | — | numpy/Cython vectorcall-enabled types |
| 6 | PY_VECTORCALL_ARGUMENTS_OFFSET | MATCHES | low | — | Cython __Pyx_PyObject_FastCall |
| 7 | PyVectorcall_NARGS macro | MATCHES | low | — | any vectorcall callee |
| 8 | METH_* flag values (VARARGS/KEYWORDS/NOARGS/O/CLASS/STATIC/COEXIST/FASTCALL/METH_METHOD) (calling-convention path) | MATCHES | low | — | every C extension method table |
| 9 | PyCFunctionObject struct layout | MATCHES | low | — | PyCFunction_NewEx consumers, numpy method objects |
| 10 | PyCMethodObject struct layout | MATCHES | low | — | METH_METHOD extensions reading defining class |
| 11 | tp_vectorcall_offset field position in PyTypeObject | MATCHES | low | — | vectorcall dispatch on heap types |
| 12 | tp_call field position in PyTypeObject | MATCHES | low | — | PyObject_Call fallback path |
| 13 | PyTypeObject vectorcall tail (tp_vectorcall, tp_watched) and 3.13 tp_versions_used | MATCHES | low | yes | 3.13+ compiled static PyTypeObject declarations |
| 14 | Fast-call helper family (PyObject_CallNoArgs/CallOneArg/CallMethodOneArg/CallMethodNoArgs) | MATCHES | low | — | numpy/Cython convenience call sites |

**Notes — Calling convention (vectorcall / METH_* / call helpers):**

1. **WRONG_SEMANTICS** — object.rs:2457-2474: PyObject_Vectorcall returns NULL immediately when kwnames != NULL (2463-2465) WITHOUT setting an exception, then otherwise materializes a tuple and calls PyObject_Call. It never consults tp_vectorcall_offset or the object's vectorcall pointer — the entire PEP-590 fast path is a no-op that degrades to tp_call. Failure: a Cython/numpy caller doing PyObject_Vectorcall(callable, args, nargsf, kwnames) with any keyword names gets a bare NULL with no pending exception -> caller's error check trips into 'SystemError: NULL result without error' or a wrong-answer crash. Even keyword-less vectorcalls silently bypass the object's own vectorcallfunc. Same slot-bypass applies to _PyObject_Vectorcall (2477) and PyObject_VectorcallMethod (2512).

2. **WRONG_SEMANTICS** — object.rs:2503-2509: PyVectorcall_Call(callable, args, kwargs) just calls PyObject_Call(callable, args, kwargs). But CPython documents PyVectorcall_Call as the canonical value for a vectorcall type's tp_call slot ('intended to be put in the tp_call slot'). molt's PyObject_Call (object.rs:2057-2062) dispatches to tp_call; if an extension set tp_call = PyVectorcall_Call (the intended, common pattern), then PyObject_Call -> tp_call(=PyVectorcall_Call) -> PyObject_Call -> ... unbounded recursion -> stack overflow / SIGSEGV. Correct behavior is to read the vectorcallfunc via tp_vectorcall_offset and call it directly (never re-enter PyObject_Call). needed_by names the affected consumers.

3. **MISSING** — No `fn PyVectorcall_Function` in the runtime (grep of runtime/ empty), not declared in include/Python.h (only PyVectorcall_NARGS/Call are), and absent from both symbol authorities (src/molt/c_api_symbols.py and src/molt/_wasm_abi_generated.py). CPython 3.12 provides `vectorcallfunc PyVectorcall_Function(PyObject*)` (Include/cpython/abstract.h) which reads Py_TPFLAGS_HAVE_VECTORCALL + tp_vectorcall_offset. Cython's CYTHON_VECTORCALL fast path (default on 3.12 full API) expands `__Pyx_PyVectorcall_Function` to `PyVectorcall_Function`; without molt's header declaration this is an implicit-declaration/undefined-symbol failure at compile/link. Blocks Cython-generated modules.

4. **WRONG_SEMANTICS** — molt_cfunction_call (object.rs:2679-2691) explicitly rejects METH_METHOD with a SystemError, deferring to 'the vectorcall path' — but molt's vectorcall path (PyObject_Vectorcall) routes back through PyObject_Call -> tp_call -> molt_cfunction_call, so a METH_METHOD|METH_FASTCALL|METH_KEYWORDS method (PyCMethod signature: self, defining_cls, args, nargsf, kwnames) is never invokable through the ABI's own dispatch — every call raises SystemError. The mm_class needed to satisfy the PyCMethod convention is available (PyCMethodObject.mm_class, abi_types.rs:498) but unused on this path. Runtime-registered methods (register_c_function hook) may handle it separately, but the ABI-visible tp_call/vectorcall entry cannot.

5. **MATCHES** — Rust PyVectorcallFunc (abi_types.rs:27-28) = fn(*mut PyObject, *mut *mut PyObject, usize, *mut PyObject)->*mut PyObject and C header (include/Python.h:222) `typedef PyObject*(*vectorcallfunc)(PyObject*, PyObject*const*, size_t, PyObject*)` both match CPython 3.12 Include/cpython/object.h verbatim (usize==size_t; kwnames as PyObject*). Signature unchanged in 3.13/3.14.

6. **MATCHES** — C macro include/Python.h:922 `((size_t)1 << (8*sizeof(size_t)-1))` and Rust const object.rs:2423 `1usize << (8*size_of::<usize>()-1)` both equal CPython's high-bit-of-size_t definition. Value-identical across 3.12-3.14.

7. **MATCHES** — include/Python.h:923 `((Py_ssize_t)((n) & ~PY_VECTORCALL_ARGUMENTS_OFFSET))` and Rust vectorcall_nargs() object.rs:2425-2427 both match CPython 3.12 _PyVectorcall_NARGS masking. CPython ships it as a static-inline; molt as a macro; value-identical.

8. **MATCHES** — abi_types.rs:628-636 and include/Python.h:552-560 give VARARGS=0x1,KEYWORDS=0x2,NOARGS=0x4,O=0x8,CLASS=0x10,STATIC=0x20,COEXIST=0x40,FASTCALL=0x80,METH_METHOD=0x200 — byte-identical to CPython 3.12 Include/methodobject.h. Values unchanged 3.12-3.14. METH_STACKLESS (0x0100 only under Stackless, else 0) is not defined by molt, which is harmless (no mainstream extension sets it).

9. **MATCHES** — abi_types.rs:486-493 {ob_base, m_ml, m_self, m_module, m_weakreflist, vectorcall} exactly mirrors CPython 3.12 Include/cpython/methodobject.h field order and count. The vectorcall field is the last member at the correct offset (5 pointers past the header). Unchanged 3.13/3.14.

10. **MATCHES** — abi_types.rs:496-499 {func: PyCFunctionObject, mm_class: *mut PyTypeObject} matches CPython 3.12 exactly — func embedded by value first, mm_class second. mm_class offset is correct (right after the full PyCFunctionObject). Unchanged 3.13/3.14.

11. **MATCHES** — abi_types.rs:278 places tp_vectorcall_offset (Py_ssize_t) as the 6th slot after the VAR_HEAD (tp_name, tp_basicsize, tp_itemsize, tp_dealloc, then tp_vectorcall_offset) — byte-exact with CPython 3.12 Include/cpython/object.h. This is the offset numpy/Cython read to locate an instance's per-object vectorcall pointer. Position stable 3.12-3.14.

12. **MATCHES** — abi_types.rs:288-289 places tp_call (ternaryfunc) immediately after tp_hash and before tp_str, matching CPython 3.12 order exactly. PyObject_Call (object.rs:2057-2062) dispatches through it correctly.

13. **MATCHES** — abi_types.rs:327-328 ends the struct with `tp_vectorcall: *mut c_void` then `tp_watched: u8`, matching CPython 3.12 (tp_finalize, tp_vectorcall, tp_watched). VERSION DELTA: CPython 3.13/3.14 append `uint16_t tp_versions_used` after tp_watched (verified against 3.13 Include/cpython/object.h). It sits AFTER the calling-convention fields so it does NOT shift tp_vectorcall_offset/tp_call/tp_vectorcall — no memory-corruption risk for this family; the short tail is a type-object-layout concern (molt also pins Py_Version=0x030c00f0, i.e. presents as 3.12). Flagged for the object-header/type-layout audit, not high here.

14. **MATCHES** — object.rs:2254-2337 implement these by building a tuple and funneling to PyObject_Call -> tp_call. Functionally correct for objects with a real tp_call and refcount-clean (INCREF-before-SetItem, DECREF tuple after). They share the same slot-bypass characteristic and the same tp_call=PyVectorcall_Call recursion hazard as PyObject_Vectorcall/PyVectorcall_Call (see those entries); no independent correctness defect. Symbols present in include/Python.h:995-998 and exported in _wasm_abi_generated.py.

---
## 3. Close-the-contract batch-fix plan (6 lanes)

The 33 gaps close in **6 coherent lanes**, ordered by blast radius:
memory-corruption (Lanes 1–3) → crash/wrong-answer (Lane 4) → build-break
(Lane 5) → regression teeth + honest-early version-gating (Lane 6). Each lane is
a mostly-single-cluster edit with its own gate so it can land independently and
prove itself. **Coupling:** Lanes 1–3 are the *static-object trifecta* — they all
touch the singletons, the immortal encoding, and type-object initialisation — so
they must reconcile the **one** immortality authority Lane 2 establishes (do not
re-introduce a second encoding). Where a `PyExc_*` facet appears in three lanes
(symbol presence L5, type-ness L3, refcount L2) it is the *same* objects seen
from three contracts; sequence L2 → L3 → L5 or land as one owner.

Every lane's exit criteria: (a) fix at the **root** / single authority, no second
encoding; (b) a **machine-checkable gate** (extend `gen_cpython_abi_layout.py`
`_Static_assert`, or a runtime `#[test]`/`check_table_drift` entry) that FAILS
before and PASSES after — proven load-bearing; (c) re-verify each touched
constant/offset against the **primary** CPython source (M06); (d)
`cargo test -p molt-lang-cpython-abi` green + `wasm32-wasip1` check + drift gate.

### Lane 1 — CANONICAL SINGLETON SYMBOLS (memory-corruption P0)

*Collapse the dual-symbol split and give the value-carrying singletons their real
shape.* **6 items:** `_Py_NoneStruct/_Py_TrueStruct/_Py_FalseStruct` unreconciled
(H, WRONG_SEMANTICS); `_Py_True/FalseStruct` bare-`PyObject` layout (H,
LAYOUT_MISMATCH); `_Py_EllipsisObject` wrong name (H, WRONG_NAME);
`_Py_NotImplementedStruct` absent (H, MISSING); `Py_None/True/False` inverted
primary-symbol + self-referential macro (M, WRONG_NAME); `PySuper_Type` absent
(M, MISSING).

- **Root fix.** Make `_Py_NoneStruct`/`_Py_TrueStruct`/`_Py_FalseStruct`/
  `_Py_NotImplementedStruct`/`_Py_EllipsisObject` the **single primary storage**;
  `#define Py_None (&_Py_NoneStruct)` etc. (CPython exports no `Py_None` symbol —
  reverse molt's inversion). Shape `_Py_True/FalseStruct` as a real
  `PyLongObject` (`ob_type=&PyBool_Type`, `lv_tag`+`ob_digit[0]={1,0}`). Add
  `PySuper_Type`. **Bridge-register all** (they must resolve through
  `pyobj_to_handle_static` / `register_static_abi_objects`, not be treated as
  unknown-foreign). Retire the `Py_EllipsisObject`/`Py_NotImplementedSentinel`
  molt-names to aliases.
- **Files.** `object.rs` (statics), `abi_types.rs` (sentinels/registration),
  `bridge.rs` (identity), `include/Python.h` + `include/molt/Python.h` (macros),
  `src/molt/_wasm_abi_generated.py` + `c_api_symbols.py` (symbol registry).
- **Gate.** `_Static_assert(sizeof(_Py_TrueStruct-shaped PyLongObject)==...)`; a
  runtime test that `Py_TYPE(Py_None)==&_PyNone_Type` (non-NULL) and
  `PyBool_Check(Py_True)` / `((PyLongObject*)Py_True)->ob_digit[0]==1`; identity
  test `is None` across the header/runtime boundary.

### Lane 2 — IMMORTALITY & REFCOUNT AUTHORITY (memory-corruption P0)

*One immortal encoding; immortal-init every static.* **7 items:** static objects
not immortal-init (H); immortal *value* diverges `1<<30` vs `UINT_MAX` (M);
exception singleton `ob_refcnt=1` not immortal (M); immortal-sentinel value
ungated (M, VERSION_UNGATED); immortal-sentinel `1<<30` vs `_Py_IMMORTAL_REFCNT`
(L); `Py_SET_REFCNT` lacks the immortal guard (L); `_Py_IsImmortal` /
`_Py_IMMORTAL_REFCNT` / public `Py_IsImmortal` absent (M, MISSING).

- **Root fix.** Establish **one** immortal authority. Adopt CPython's
  `_Py_IMMORTAL_REFCNT` value (`UINT_MAX` on 64-bit, `UINT_MAX>>2` on 32-bit) —
  or keep `1<<30` but make it the *sole* encoding *and* accept that off-model
  prebuilt `.so`s stay unsupported — and drive **every** static (`Py*_Type`,
  `PyExc_*`, all sentinels, `PyObject_HEAD_INIT`) through it at init so none is
  mortal. Add the `_Py_IsImmortal()` inline + `_Py_IMMORTAL_REFCNT` macro +
  public `Py_IsImmortal()`; make `Py_SET_REFCNT` no-op on immortals.
- **Files.** `abi_types.rs` (`init_static_types`, `exc_singletons!`,
  `PyObject_HEAD_INIT`), `refcount.rs` (threshold + guard), `include/Python.h`
  (macros/inlines).
- **Gate.** `check_table_drift`-style single-authority assertion (one immortal
  constant referenced everywhere); runtime test: `Py_DECREF` any static N times
  then assert refcount unchanged and object still bridge-resolvable; a teeth-test
  that a mortal-init regression makes `PyErr_SetString(PyExc_ValueError)` fail
  identity after one decref.

### Lane 3 — TYPEOBJECT COMPLETENESS & FLAGS (OOB + wrong-answer P0)

*Fully initialise type objects; define the flag surface.* **6 items:** builtin
`Py*_Type` `tp_flags = READY` only (H); builtin type-object zero-init shells (M) —
same class; `PyType_FromSpec` not `HEAPTYPE` + bare `Box` not `PyHeapTypeObject`
(H); `Py_TPFLAGS_*` fast-subclass macros absent (H, MISSING); Rust
`Py_TPFLAGS_DEFAULT = BASETYPE` wrong (L); `PyExc_*` bare sentinels not
`PyTypeObject` (M).

- **Root fix.** Initialise every builtin static type with real
  `tp_basicsize`/`tp_itemsize`/`tp_base`/`tp_dict` and
  `DEFAULT|BASETYPE|<TYPE>_SUBCLASS|READY`. **Define all `Py_TPFLAGS_*` macros**
  in the header (the `*_SUBCLASS` fast-check bits, `DISALLOW_INSTANTIATION`,
  `IMMUTABLETYPE`, `SEQUENCE`, `MAPPING`, `MANAGED_WEAKREF`, `ITEMS_AT_END`,
  `VALID_VERSION_TAG`, `READYING`). In `PyType_FromSpecWithBases` OR in
  `HEAPTYPE` and allocate a real `PyHeapTypeObject` (add the `#[repr(C)]` mirror
  + `_Static_assert` gate); honor the module argument in
  `PyType_FromModuleAndSpec` so `PyType_GetModuleState` works. Make `PyExc_*`
  real exception TYPE objects (couples to Lanes 1/2 — reuse the immortal init).
  Fix the Rust `Py_TPFLAGS_DEFAULT` const to `0`.
- **Files.** `abi_types.rs` (`init_static_types`, exc), `typeobj.rs`
  (`PyType_FromSpec*`), `include/Python.h` (flag macros + `PyHeapTypeObject`),
  `gen_cpython_abi_layout.py` (heap-type gate).
- **Gate.** `_Static_assert(sizeof(PyHeapTypeObject)==...)`; runtime tests:
  `PyType_FastSubclass(&PyLong_Type, Py_TPFLAGS_LONG_SUBCLASS)==1`,
  `PyType_HasFeature(&PyLong_Type, BASETYPE)==1`, a `PyType_FromSpec` type is
  `HEAPTYPE` and its `ht_name`/`ht_module` are in-bounds.

### Lane 4 — CALL / VECTORCALL PROTOCOL (crash + wrong-answer P0)

*Implement the real PEP-590 fast path; stop the recursion.* **5 items:**
`PyObject_Vectorcall` NULL-without-exception + slot-bypass (H);
`PyVectorcall_Call` infinite recursion as `tp_call` (H); `PyVectorcall_Function`
absent (H, MISSING); `METH_METHOD`/`PyCMethod` dispatch rejected with SystemError
(M); `PyAsyncMethods.am_send` sendfunc signature wrong (M).

- **Root fix.** `PyObject_Vectorcall`/`_PyObject_Vectorcall`/
  `PyObject_VectorcallMethod` must read `Py_TPFLAGS_HAVE_VECTORCALL` +
  `tp_vectorcall_offset`, fetch the object's `vectorcallfunc`, and **call it
  directly** (handling kwnames), falling back to `tp_call` only when no slot —
  never returning NULL without a pending exception. `PyVectorcall_Call` must call
  the slot directly and **never re-enter `PyObject_Call`** (so `tp_call =
  PyVectorcall_Call` terminates). Implement + export + declare
  `PyVectorcall_Function`. Route `METH_METHOD|FASTCALL|KEYWORDS` through
  `PyCMethodObject.mm_class` (the field already exists). Fix the `sendfunc`
  typedef to `PySendResult (*)(PyObject*, PyObject*, PyObject**)`.
- **Files.** `object.rs` (vectorcall family, `molt_cfunction_call`),
  `include/Python.h` (`PyVectorcall_Function` decl + `sendfunc`), `abi_types.rs`
  (sendfunc/PySendResult).
- **Gate.** Runtime tests: a vectorcall type with a real slot is invoked through
  the slot (assert slot ran, not the tuple fallback); `tp_call =
  PyVectorcall_Call` completes without recursion; a keyword vectorcall returns a
  value or sets an exception (never bare NULL); a `PyCMethod` is callable.

### Lane 5 — MISSING HEADER / SYMBOL SURFACE (build-break, import-gated)

*Populate the compile/link surface that is not a corruption vector.* **3 items:**
~20 absent `PyExc_*` data symbols (H, MISSING); `PyDateTime_GET_*` accessor
macros + `PyDateTime_Date/Time/DateTime/Delta/TZInfo` typedefs +
`PyDateTime_CAPSULE_NAME` (H, MISSING); `Py_RELATIVE_OFFSET` flag (L, MISSING).

- **Root fix.** Extend the `exc_singletons!` list to CPython 3.12's full ~68
  `PyExc_*` set (`ReferenceError`, `StopAsyncIteration`, the `BlockingIOError`/
  connection/process family, `IndentationError`/`TabError`,
  `UnicodeTranslateError`, the `*Warning` family, `BaseExceptionGroup`,
  `EnvironmentError`/`WindowsError` aliases) — and register/immortal-init them via
  Lanes 1–3. Add the `PyDateTime_GET_*` / `*_DATE_GET_*` / `*_TIME_GET_*` /
  `*_DELTA_GET_*` macros + the C struct typedefs + `PyDateTime_CAPSULE_NAME` to
  `include/datetime.h` (**header-only** — the Rust structs + `data[]` packing
  already match, so the macros read the right bytes). Add `Py_RELATIVE_OFFSET=8`.
- **Files.** `abi_types.rs` (exc list), `include/datetime.h`,
  `include/Python.h` (`Py_RELATIVE_OFFSET`),
  `src/molt/_wasm_abi_generated.py` (registry).
- **Gate.** A registry-completeness test enumerating the canonical 3.12 `PyExc_*`
  set and asserting each is exported; a compile-smoke that `#include`s a TU using
  `PyDateTime_GET_YEAR` + a missing `PyExc_ReferenceError`.

### Lane 6 — DRIFT-GATE TEETH & VERSION-GATING (M02 honest-early)

*Close the gate gaps and the version-delta cliff.* **6 items:** the 5 protocol
method tables (`PyNumber/Sequence/Mapping/Async/BufferProcs`) + `PyCapsuleObject`
absent from the `_Static_assert` gate (L, MISSING); `PyTypeObject` 3.13/3.14
`tp_versions_used` tail ungated (M, VERSION_UNGATED); free-threaded
(`Py_GIL_DISABLED`) `PyObject` header ungated ×2 (M+L, VERSION_UNGATED); 3.14
immortal-refcount constants ungated (L, VERSION_UNGATED); `tp_version_tag`
`c_ulong` vs `unsigned int` type-correctness (L, LAYOUT_MISMATCH, benign).

- **Root fix.** Emit `_Static_assert`s for the 5 protocol tables + `PyCapsuleObject`
  in `gen_cpython_abi_layout.py` (they are all-pointer, so a reorder is
  sizeof-invisible today — this closes the last field-order-drift hole). Add
  **per-target-version gating**: when the M02 target is 3.13/3.14, emit the
  `tp_versions_used` tail (and the 3.14 immortal constants) or **refuse
  honest-early**; when a `Py_GIL_DISABLED`/free-threaded ABI is detected, emit an
  explicit honest-early error rather than compiling against the with-GIL header
  (M34: fail LOUD, not silent-corrupt). Change the Rust `tp_version_tag` to
  `c_uint` for type-fidelity (layout unaffected).
- **Files.** `tools/gen_cpython_abi_layout.py`, `abi_types.rs`, `include/Python.h`,
  a target-version/ABI-tier honest-early check in the build path.
- **Gate.** A teeth-test that reordering a `PyNumberMethods` field fails the C
  compile; a build-path test that a 3.13-tail / free-threaded target either
  produces the right shape or errors with a clear message (never the silent
  3.12-shape coercion that the lone `sizeof==416` assert produces today).

### Lane dependency & sequencing

```
Lane 2 (immortality authority) ─┬─► Lane 1 (singletons reuse the one encoding)
                                ├─► Lane 3 (types + PyExc_* immortal-init)
                                └─► Lane 5 (new PyExc_* immortal-registered)
Lane 4 (vectorcall) ── independent (object.rs call family) ── land any time
Lane 6 (gates + version-gating) ── land LAST or CONCURRENT ── ratchets 1–5
```

Lanes 2→1→3→5 share the static-object surface and MUST agree on the single
immortal encoding; land them under one owner or in that order. Lane 4 is
disjoint (`object.rs` call path) and Lane 6 is the teeth that keep 1–5 from
regressing — land Lane 6's gate *stubs* first so each fix lands green against a
real assertion.

---

## 4. Overlap with adjacent work (do not re-audit)

This binary matrix is the **layout/symbol tier**; three adjacent efforts cover
the behavior tier, the discovery tier, and the fixes already landed. The overlap
is deliberate — same objects, different contract — and is mapped here so no lane
re-does another's work.

- **C-API coverage matrix (`wf_4ab826d5`) + `CPYTHON_ABI_DIVERGENCE_LEDGER.md`
  (180 behavioral divergences).** Those catalogue **function behavior** —
  `MISSING_DISPATCH` (a slot never consulted), `DIVERGENT` (wrong result),
  `SILENT_SENTINEL` (−1/NULL with no exception), `STUB`, `THEATER`. This matrix
  is the **substrate they read**: e.g. the divergence ledger's
  `PyObject_Vectorcall` behavioral row and this matrix's *"never reads the
  vectorcall slot"* layout row are the **same defect** seen as behavior vs.
  contract — fix once in Lane 4. Likewise the ledger's PyType_Ready / member /
  buffer behavioral rows sit on the `PyTypeObject` / `PyMemberDef` / `Py_buffer`
  layout rows here (all MATCHES + gated). **Rule:** a function-behavior fix that
  also requires a struct/symbol change belongs to *this* matrix's lane; a pure
  dispatch fix belongs to the ledger's F4/lock-sweep lanes. Do not double-fix
  `PyObject_Vectorcall`, `PyExc_*`, or the singletons across both.

- **Discovery findings (`NATIVE_DISCOVERY_FRONTIERS.md` + the
  `DISCOVERY-FRONTIER-FIXES` claim).** The native numpy-init discovery engine
  **independently surfaced the exact singleton split this matrix flags as Lane 1**:
  it explicitly **DEFERRED** `_Py_NoneStruct`/`_Py_TrueStruct`/`_Py_FalseStruct`/
  `_Py_NotImplementedStruct`/`_Py_EllipsisObject` because an in-crate
  `global_asm` `.set` alias emits a *local* symbol on Mach-O (cannot satisfy
  numpy's dynamic lookup) — the correct fix is a linker `--defsym`/`-alias` at the
  **final native link**, which the discovery harness `build.rs` already does. So
  **Lane 1's fix has two halves**: the *source* half (single-primary
  `_Py_*Struct` + bridge-register + real bool shape — this matrix) and the
  *native-link* half (the alias, owned by molt's native-artifact link layer, per
  the discovery deferral). They must land coherently — the discovery lane already
  proved the naive same-crate alias does **not** work. Discovery also already
  landed the `datetime.datetime_CAPI` capsule (Lane-5-adjacent) and real
  allocators/whitespace/`_Py_Dealloc`.

- **Alignment / buffer fixes already LANDED (do not redo — this is *why* those
  rows are MATCHES).** The wasm32 4-byte-alignment clobber class is closed:
  `_molt_abi_layout.generated.h` pins `ob_type@4` and the bridge reads the
  trailing handle via `read_unaligned` (the `bridge.rs` `# Alignment` doc + the
  `a98ef2978e` wasm32-alignment class + the `PyMember_SetOne` `write_unaligned`
  fix). `Py_buffer` is byte-pinned on both widths by the `_Static_assert` gate
  (the Miri-C offset fix + `gen_cpython_abi_layout.py`); the buffer descriptor
  provenance UB (`format`/`shape`/`strides` raw-projection) was fixed at root
  under Miri (`MIRI_STRICT_PROVENANCE.md` finding C); the array buffer-export
  lease closes the resize-UAF (`ARRAY-BUFFER-EXPORT-INTERLOCK`); the bridge's
  `UnsafeCell<PyObject>` refcount-aliasing fix (Miri finding A) makes the
  refcount mutation-through-aliasing-pointers model sound. **Consequence for the
  lanes:** the *layout* substrate under Lanes 1–3 is already Miri-clean and
  alignment-safe — the remaining corruption is **object-model contract**
  (mortal statics, wrong shape, dual symbols), *not* offset/alignment. Do not
  reopen the alignment or buffer-layout rows; build the immortality/singleton/
  type-init fixes on top of the gated, provenance-correct substrate they leave.

---

*Matrix authored per M02 (≥3.12 version-gated subset), M05 (zero fakes — every
MATCHES kept only when the primary CPython source was reproduced, default
disposition DOWNGRADE), M06 (primary-source verification), and M45 (drift-gate as
the anti-regression authority). Line-number anchors re-anchor by symbol.*
