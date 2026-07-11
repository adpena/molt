# Witness-Frontier CLASS audit — the no-leaves close-out plan

**Purpose.** The pact witness (numpy + scipy → WASM) is driven to green
frontier-by-frontier; each frontier surfaces only after a ~16-min run, so blockers
arrive leaf-by-leaf. Each landed frontier fix is an INSTANCE of a bug **class**.
This doc statically enumerates the COMPLETE set of remaining witness-runtime
frontier bug-classes so they can be **batch-closed** (one lane per class) instead
of chased one at a time.

**Method.** grep + READ the real code; every finding verified against the actual
source at the anchor below (M05: a PASS is a hypothesis until reproduced — here,
until read in the tree). Where a numpy contract is cited it is verified against the
vendored numpy source, not memory (M06).

**Anchor.** `origin/main` @ `5be9dad3e1` (worktree off it).

**Version correction (load-bearing).** The witness builds **numpy 2.5.1** + **scipy
1.18.0**, NOT 1.26.4. numpy 2.x lives in `numpy/_core/` (not `numpy/core/`) and its
`_multiarray_umath` init is a **`Py_mod_exec` slot** (`_multiarray_umath_exec`,
`bench/friends/repos/numpy_off_the_shelf/numpy/_core/src/multiarray/multiarraymodule.c:4909`),
not a classic `PyInit` body. Any audit keyed to 1.26.4 layout mis-maps.

**Sibling docs (cross-ref, no dup).** This doc is the *witness-runtime frontier
class* view. It complements — and cites rather than repeats —
`CPYTHON_ABI_DIVERGENCE_LEDGER.md` (per-defect, 180 rows),
`CPYTHON_ABI_COVERAGE_MATRIX.md` (spec coverage, 345 rows),
`NATIVE_DISCOVERY_FRONTIERS.md` (the 14 Tier-A), `POISON_ORPHAN_LEDGER.md`,
`PANIC_REACHABILITY_LEDGER.md`, `CPYTHON_ABI_BINARY_CONTRACT_MATRIX.md`.

**The four landed instances this generalizes.**
- (a) `PyTuple_Type` was `mem::zeroed()` with NULL `tp_richcompare` → **ZEROED-SHELL** (Class 1).
- (b) a duplicate `hook_dict_set` appended only the flat order vector, bypassing the
  canonical hash/lookup/refcount machinery → **HALF-WIRED / DUPLICATE HOOK** (Class 1).
- (c) a raw C-extension identity handle (`0xA11C…`) decoded as a molt `float` in value
  dispatch → **molt↔C BOUNDARY MIS-DECODE** (Class 2).
- (d) `molt_cpython_abi_cext_call_trampoline` rejected a callable at a reserved index →
  **DISPATCH / TRAMPOLINE COMPLETENESS** (Class 3).

---

## Class counts (headline)

| Class | Defective instances found | Status of the landed instance | Batch lane |
|---|---|---|---|
| 1 — Half-wired / duplicate / stub hooks | **0 / 61** hooks defective (vtable fully wired) + 4 missing-guard hardening | (b) dict-hook FIXED | CLASS1-HARDEN (low) |
| 1b — Zeroed-shell builtin type-slots | **≥4 P0** DUAL_INHERIT-copied NULLs (+ broad `tp_*` NULLs across 15 builtin types) | (a) tuple/list/dict `tp_richcompare` FIXED; siblings OPEN | **CLASS1-SLOTS (P0)** |
| 2 — molt↔C boundary mis-decode | **~28 decode-as-value sites** (7 HIGH incl. richcompare / hash / repr-str / float-decode / IsTrue, 6 MED, ~15 LOW; all ~80 `pyobj_to_handle` sites classified in §2.4) — reachable via raw-registered type/exc/dtype objects | (c) object.rs call path FIXED; sibling API sites OPEN | **CLASS2-DECODE (P0)** |
| 3 — Dispatch / trampoline completeness | **1** unhandled kind (METH_METHOD, latent) + reserved-table completeness (latent) | (d) `molt_type_new` reserved slot CLOSED (index 1) | CLASS3-DISPATCH (P2/P3) |

Ranking uses the witness call-surface evidence in §4 (numpy 2.5.1 init).

---

## Class 1 — HALF-WIRED / DUPLICATE / STUB HOOKS

### 1.1 RuntimeHooks vtable — VERIFIED CLEAN (0 defects)

The vtable (`runtime/molt-cpython-abi/src/hooks.rs:63`, 61 fn-pointer fields) is
constructed in `runtime/molt-runtime/src/cpython_abi_hooks.rs:2134` (`register_cpython_hooks`).
Every `hook_*` was read; each routes to a **canonical runtime authority** — none
re-implements arithmetic/hash/order/refcount logic locally. Representative wiring:

| Hook | Routes to (canonical authority) |
|---|---|
| `hook_dict_set` (:325) | `dict_set_in_place` — **the (b) fix**: the duplicate that only pushed the flat order vector is gone; hash/lookup/refcount now canonical |
| `hook_dict_get/_del/_len/_entry` | `dict_get_in_place` / `dict_del_in_place` / `dict_len` / `dict_order` |
| `hook_list_insert/_sort/_reverse/_set_slice` | `c_api::PyList_Insert/_Sort/_Reverse` (`ins1`/compare authority) |
| `hook_number_binary_op/_unary_op/_power` (:814/855/873) | `c_api::PyNumber_*` (arbitrary-precision, overload dispatch); unknown discriminant → SystemError, never fake success |
| `hook_set_*` (:907–932) | `c_api::PySet_*` (hash table, dedup) |
| `hook_dict_op` (:879) | `c_api::PyDict_Copy/Keys/Values/Items` |
| `hook_object_call` (:524) | `molt_call_bind` (single call authority) |
| `hook_object_get_attr/_set_attr` (:505/509) | `builtins::attributes::molt_get_attr_name/_set_attr_name` |
| `hook_object_dir` (:938) | `molt_object_dir_method` (fails closed on pending exc) |
| `hook_float_repr` (:601) | `object::float_repr::repr_float` (round-half-to-even authority) |
| `hook_foreign_new` (:581) | `object::foreign::foreign_new` (`TYPE_ID_FOREIGN` — the (c) crossing fix) |

The pre-init `STUB_HOOKS` table (hooks.rs:692) fails **closed** (0 / -1 / NULL
sentinels) — not fake successes — and is only installed when the runtime is not
linked (pure-ABI tests). Not a production path.

**Verdict:** the duplicate/bypass hook class is closed on the vtable surface. No
second `hook_dict_set`-style duplicate exists.

### 1.2 Missing-type-guard hooks — HARDENING (low)

Four construction-primitive hooks omit the `object_type_id` guard that their
`*_len` siblings carry, so a mis-routed handle silently corrupts instead of
no-op'ing:

| Hook | file:line | Gap |
|---|---|---|
| `hook_list_append` | cpython_abi_hooks.rs:153 | `seq_vec(ptr).push` with no `TYPE_ID_LIST` guard |
| `hook_list_item` | :174 | `seq_vec_ref(ptr).get` no guard (cf. `hook_list_len` :162 which guards) |
| `hook_tuple_set` | :278 | no `TYPE_ID_TUPLE` guard; also **grows** the vec on OOB `i` (tuples are fixed-size) |
| `hook_tuple_item` | :305 | no guard (cf. `hook_tuple_len` :293 which guards) |

Low risk (callers pass freshly-allocated handles), but a defensive `TYPE_ID`
guard makes the class total. Overlaps `PANIC_REACHABILITY_LEDGER` Lane 2
(PANIC-PLUMB "missing TYPE_ID_LIST/TUPLE type guards").

### 1.3 ZEROED-SHELL builtin type-object slots — OPEN, P0 (the (a) class, not fully swept)

`runtime/molt-cpython-abi/src/abi_types.rs:936–1021,1713–1733`: every builtin
`Py*_Type` static is `unsafe { std::mem::zeroed() }`. `init_static_types`
(abi_types.rs:1034) patches them but fills only:
`tp_name`, `tp_flags`, `tp_base`, `tp_basicsize`/`tp_itemsize`; `tp_dealloc`
(subset); **`tp_richcompare` on tuple/list/dict ONLY** (:1083/1088/1089 — the (a)
fix + its two swept siblings); **`tp_call` on type/cfunction/method** (:1112–1124).

**NULL on ALL builtin types:** `tp_hash`, `tp_as_number`, `tp_as_sequence`,
`tp_as_mapping`, `tp_str`, `tp_repr`, `tp_iter`, `tp_getattro`, `tp_methods`,
`tp_getset`. **`tp_richcompare` NULL** on str, bytes, int, float, complex, bool,
set, frozenset, bytearray. (Verified: exactly three `Py*_Type.tp_richcompare`
assignments exist across the whole crate; **zero** `Py*_Type.tp_hash` assignments.)

**Why this is witness-reachable P0 (verified against numpy source).** numpy's
`setup_scalartypes` COPIES these builtin slots straight off molt's statics during
init — `multiarraymodule.c`:

```
4782  Py##child##ArrType_Type.tp_hash = Py##parent1##_Type.tp_hash;        // DUAL_INHERIT
4796  Py##child##ArrType_Type.tp_richcompare = Py##parent1##_Type.tp_richcompare;  // DUAL_INHERIT2
4798  Py##child##ArrType_Type.tp_hash = Py##parent1##_Type.tp_hash;
4827  DUAL_INHERIT (Double,  Float,   Floating);        // -> PyFloat_Type.tp_hash   (NULL)
4831  DUAL_INHERIT (CDouble, Complex, ComplexFloating); // -> PyComplex_Type.tp_hash (NULL)
4834  DUAL_INHERIT2(String,  Bytes,   Character);       // -> PyBytes_Type.{tp_richcompare,tp_hash} (NULL)
4835  DUAL_INHERIT2(Unicode, Unicode, Character);       // -> PyUnicode_Type.{tp_richcompare,tp_hash} (NULL)
```

Copying a NULL slot leaves numpy's `Double`/`CDouble`/`String`/`Unicode` scalar
types **unhashable and non-comparable**. Molt's own `PyType_Ready` reinforces the
propagation: `inherit_fn!(tp_hash)` / `inherit_fn!(tp_richcompare)`
(typeobj.rs:502/509) inherit the base's NULL when numpy readies a scalar type with
`tp_base = &PyFloat_Type`. `PyObject_Hash` on a NULL `tp_hash` fails closed
(typeobj.rs:2143, correct-but-fatal).

**Secondary facet (open).** numpy reads *methods* off the builtin type objects:
`PyObject_GetAttr((PyObject*)&PyBytes_Type/&PyUnicode_Type, method_name)`
(multiarraymodule.c:4129/4133) — needs `tp_methods` + a `tp_getattro` on the
builtin str/bytes types (both NULL today).

**OPEN P0 instances (enumerated):**

| Builtin type-slot | file:line (NULL site) | numpy reach | Fix |
|---|---|---|---|
| `PyFloat_Type.tp_hash` | abi_types.rs:1195 (never set) | DUAL_INHERIT :4827 | generic `molt_num_hash` slot → `bridge::molt_hash_from_bits` |
| `PyComplex_Type.tp_hash` | :1204 | :4831 | same |
| `PyBytes_Type.{tp_richcompare,tp_hash}` | :1220 | DUAL_INHERIT2 :4834 | `molt_bytes_richcompare` (mirror `molt_tuple_richcompare`) + hash slot |
| `PyUnicode_Type.{tp_richcompare,tp_hash}` | :1211 | :4835 | `molt_str_richcompare` + hash slot |
| `PyLong_Type.tp_richcompare`, `PyFloat_Type.tp_richcompare` | :1174/1195 | scalar compare / `tp_bases` PyType_Ready inherit | `molt_{long,float}_richcompare` |
| `Py{Bytes,Unicode}_Type.{tp_methods,tp_getattro}` | (never set) | GetAttr :4129/4133 | expose the runtime str/bytes method table via bridge getattro |

**Canonical fix (one authority pattern already in-tree):** `molt_tuple_richcompare`
/ `molt_list_richcompare` / `molt_dict_richcompare` (api/sequences.rs, api/mapping.rs)
are the template. Add `molt_{str,bytes,long,float,complex}_richcompare` +
one generic numeric/str `tp_hash` slot routing to `bridge::molt_hash_from_bits`
(the authority `PyObject_Hash` already uses at typeobj.rs:2134), and wire them in
`init_static_types`. This closes the zeroed-shell frontier as a batch instead of
one scalar type per witness run.

---

## Class 2 — molt↔C BOUNDARY MIS-DECODE

### 2.1 The architecture and the poison bit-pattern

`runtime/molt-cpython-abi/src/bridge.rs` converts `*mut PyObject` ↔ molt handle
three ways:

| Converter | file:line | For a genuine C-extension ("foreign") object it returns |
|---|---|---|
| `pyobj_to_handle` (IDENTITY) | :394 | **`Some(0xA11C…)`** — a synthetic raw handle (base `0xA11C_0000_0000_0000`, +0x10 each). **NOT a valid MoltObject** |
| `molt_handle_for_pyobj` (genuine molt only) | :404 | `None` (raw-registered excluded) |
| `molt_value_for_pyobj` (SAFE crossing) | :432 | a fresh `TYPE_ID_FOREIGN` wrapper (via `foreign_new`) |

The synthetic `0xA11C_0000_0000_0000` decodes through the NaN-box
(`MoltObject::from_bits`) as a **finite negative f64** — `is_float()` returns
**true**. That is precisely the (c) bug: "decoded as a molt float".

**The defect:** any C-API fn that calls `pyobj_to_handle(op)` (identity) and then
DECODES the returned bits as a molt VALUE — `is_float()`/`as_float()`,
`molt_hash_from_bits`, arithmetic, `as_ptr()` — silently mis-reads a foreign
object's `0xA11C` anchor. A pure `is_some()`/`is_none()`/`_Check`/re-bridge use is
SAFE.

### 2.2 Reachability — HIGH (verified)

`PyType_Ready` **raw-registers every type object** it processes
(`typeobj.rs:80`, also :229/:1468/:1496; capsules at capsule.rs:115). numpy calls
`PyType_Ready` on ~50 types/dtypes during `_multiarray_umath` init (§4 Rank 1), so
**every numpy init type/dtype object carries a live `0xA11C` anchor in `from_py`**
→ `pyobj_to_handle(<numpy type/dtype>)` returns `Some(0xA11C)`. Any decode-as-value
site reached with a numpy type/dtype object mis-decodes.

### 2.3 The (c) fix was scoped to ONE call path; siblings remain

The (c) landing converted only the `PyObject_Call` path in `api/object.rs` to the
safe converters (`molt_handle_for_pyobj` at :2085/:2125/:2162; foreign objects fall
to a C-tuple marshal / honest TypeError). The **same raw `pyobj_to_handle`→decode
pattern survives elsewhere.** Independently verified:

| Site | file:line | Bucket | Evidence |
|---|---|---|---|
| **`native_value_richcompare`** (via `do_richcompare`) | typeobj.rs:2591→2608 | **B — RISK, highest** | `do_richcompare` calls `native_value_richcompare` **FIRST, before any `tp_richcompare` slot**. `pyobj_to_handle(v)`/`(w)` → `Some(0xA11C)` for two raw-registered dtype/type objects; `as_num` (:2608) `is_float()`-matches both anchors → returns a numeric compare of two garbage floats and **preempts the DType's own richcompare**. numpy compares DType tuples during cast/promoter registration at init (§4 Rank 3) → silent mis-registration |
| **`PyFloat_AsDouble`** | numbers.rs:1252→1255 | **B — RISK** | `pyobj_to_handle(op)` → `obj.is_float()` matches the `0xA11C` anchor → returns garbage `as_float()` instead of dispatching `nb_float` (the :1287 foreign path is never reached for a raw-registered object) |
| **`PyObject_Hash`** | typeobj.rs:2132→2134 | **B — RISK** | routes any `pyobj_to_handle`-`Some` into `molt_hash_from_bits`, which `is_float()`-hashes the `0xA11C` bits (bridge.rs:1261) → bogus hash; the :2136 foreign-`tp_hash` branch is dead for raw-registered objects. numpy keys cast/promoter registries by dtype |
| `py_long_value` | numbers.rs:77 | C — guarded-by-luck | `0xA11C` decodes as float → matches neither `as_int()` nor `is_ptr()` → falls through to `__index__` (correct answer, but by non-match, not by design) |
| `is_int_like` | numbers.rs:173 | C — guarded-by-luck | same non-match → correctly `false` |

The `native_value_richcompare` case is the sharpest: because `do_richcompare` runs
the native path *before* slot dispatch, a raw-registered foreign object cannot fall
through to its own `tp_richcompare` — the mis-decode is not merely a missed fast
path, it is the answer. The canonical fix here MUST use `molt_handle_for_pyobj`
(exclude raw), so two foreign operands yield `None` → slot dispatch proceeds.

**Canonical fix (the (c) pattern, generalized):** at every decode-as-value site,
replace `pyobj_to_handle` with `molt_handle_for_pyobj` (so a raw-registered foreign
object yields `None` and falls to the object's own `nb_*`/`tp_hash` slot), or route
the crossing through `molt_value_for_pyobj` when a first-class molt value is needed.
For `PyFloat_AsDouble`/`PyObject_Hash` specifically: `molt_handle_for_pyobj` makes
them fall to the foreign `nb_float`/`tp_hash` path that already exists below.

### 2.4 Exhaustive site enumeration

All ~80 `pyobj_to_handle(` call sites in `runtime/molt-cpython-abi/src/` were read
and classified. **Bucket A** = safe (identity / `is_some`/`is_none` / `_Check`-tag
the float-anchor fails / re-bridge). **Bucket B** = decode-as-value RISK. **Bucket
C** = guarded (an `is_ptr`/`is_none`/`classify` value-tag routes the anchor to a
foreign-safe path before any wrong value). Only buckets B and C are listed (A sites
are safe by construction; ~40 of them, all identity/`_Check`/test-only).

**Which objects trigger it (reachability):** `pyobj_to_handle` returns the `0xA11C`
anchor only for **raw-registered** objects — builtin **type objects**, **exception
singletons**, `Ellipsis`/`NotImplemented`/UTC, **capsules**, **descriptors**
(register sites: abi_types.rs:1330/1338/1341, capsule.rs:115,
typeobj.rs:80/229/1468/1496; confirmed by in-tree tests abi_types.rs:1836/1996). A
*fresh unregistered* numpy scalar returns `None` and dodges the bug today. numpy
init passes exactly the triggering kinds around (compares/hashes/reprs dtype & type
objects; `GetAttr((PyObject*)&PyArray_Type, …)`), so the HIGH sites are
witness-reachable.

**Bucket B — HIGH (silently-wrong scalar/string/hash/bool for common registered objects):**

| # | Site | file:line | Defect |
|---|---|---|---|
| 1 | `PyFloat_AsDouble` | numbers.rs:1252 | `is_float(anchor)`→garbage f64 vs `nb_float` dispatch (also drags `PyComplex_AsCComplex`/`RealAsDouble`/`ImagAsDouble` which delegate here) |
| 2 | `PyObject_Repr` / `PyObject_Str` | typeobj.rs:2447 / 2483 | `native_stringify(anchor)` → garbage-float string vs `tp_repr`/`tp_str` |
| 3 | `PyObject_Hash` | typeobj.rs:2132 | `molt_hash_from_bits(anchor)` `is_float`-hashes vs `tp_hash` |
| 4 | `native_value_richcompare` (via `do_richcompare`) | typeobj.rs:2591 | `as_num` `is_float(anchor)` → numeric compare of garbage floats, **preempts slot dispatch** |
| 5 | `PyFloat_Check` | numbers.rs:1591 | `is_float(anchor)`→reports registered foreign as a float |
| 6 | `PyNumber_Check` | numbers.rs:1613 | `is_float(anchor)`→reports registered foreign as a number |
| 7 | `PyObject_IsTrue` | object.rs:1021 | `is_float(anchor)` short-circuits truthiness, skips `nb_bool`/container path |

**Bucket B — MEDIUM (arithmetic/format/attr paths):**

| # | Site | file:line | Defect |
|---|---|---|---|
| 8 | `PyNumber_*` binary/unary/power (via `resolve_bits`) | abstract_number.rs:23,344,368,501 | `number_binary_op(anchor,…)` garbage arithmetic vs foreign `nb_*` slot |
| 9 | `PyObject_Format` | object.rs:1216 | `object_format(anchor,…)` garbage-float format |
| 10 | `%d`/`%i`/`%u` int-format helper | strings.rs:~1783 | `as_float(anchor) as i128` truncation vs `PyLong_AsLongLong`/`__index__` |
| 11 | `PyObject_GenericSetAttr` (receiver) | object.rs:901 | `object_set_attr(anchor,…)` on garbage receiver vs `foreign_generic_setattr` |
| 12 | `PyObject_Dir` | object.rs:2018 | `object_dir(anchor)` over garbage receiver |
| 13 | `value_str_message` (exc message) | errors.rs:243 | `as_float(anchor).to_string()` as exception text (diagnostic) |

**Bucket B — LOW (key/receiver type-confusion; needs a registered-foreign object AS a dict key or container receiver):**
- Dict keys hashed as float: mapping.rs:81/348/550, object.rs:1602/1698/1727 — fix by routing **keys** through `molt_value_for_pyobj` (the pattern already used for the *values* two lines below each).
- Container receivers lacking an `is_ptr`/classify guard: sequences.rs:76/137/302/409/487/878/1051/1176, mapping.rs:68/344/821, abstract_sequence.rs:1125 — fix by gating through the existing `resolve_native_list`/`resolve_native_dict` helper (or the `is_ptr()`+`classify_heap==<Tag>` check that `PyTuple_GetItem`/`PyList_SetSlice` already use).

**Bucket C — guarded (no fix needed, documented so the sweep doesn't churn them):**
numbers.rs:77/177/204/756/1455/1481/1499/1545/1563; object.rs:240/1305/1594/1624/1650/1690/1720/3196; sequences.rs:20/353/449/577/589/1092/1220; mapping.rs:21/633; strings.rs bytes/str readers (hook returns null on non-str/bytes tag); errors.rs:538/724; abstract_mapping.rs:21; abstract_sequence.rs:34.

**Related raw-bits decode (distinct from `pyobj_to_handle`, same class):**
`read_bridge_header_bits` (bridge.rs:661) reads a trailing `u64` off an arbitrary C
pointer via `read_unaligned`; used as the `None`-branch fallback in modules.rs:61,
object.rs:3199 (`PyCFunction_NewEx` self), errors.rs `bridge_pyobj_to_bits`,
loader.rs:130. For a genuine foreign object with no `BridgeHeader` trailer it yields
uncontrolled bits used as a MoltObject value/identity — route the same foreign-safe
way. (The alignment facet of this read is already tracked by E1-WITNESS-TO-GREEN;
this is the *decode-semantics* facet.)

**Canonical fix (whole batch):** the root cause is `pyobj_to_handle(op) == Some ⇒
genuine Molt value`, false for raw-registered foreign objects. Swap
`pyobj_to_handle` → `molt_handle_for_pyobj` on every bucket-B fast path (registered
foreign → `None` → the existing foreign/slot dispatch), and route foreign dict
**keys** through `molt_value_for_pyobj`. Same one-line pattern as the (c) fix,
applied across ~13 primary + ~15 low sites in one sweep.

---

## Class 3 — DISPATCH / TRAMPOLINE COMPLETENESS

There are **two independent dispatch surfaces**; both were audited.

### 3.1 Surface A — C-ext call trampoline (module functions)

`molt_cpython_abi_cext_call_trampoline` (cpython_abi_hooks.rs:1869) is the shared
trampoline for every C function registered via `hook_register_c_function`
(:2046). It decodes a registry id from the closure and dispatches on
`CExtDispatchKind` (:1035). Handled kinds:
`NoArgs, OneObject, VarArgs, VarArgsKeywords, FastCall, FastCallKeywords`.

`CExtDispatchKind::from_flags` (:1046) **rejects**: `METH_METHOD` (:1048),
fastcall with any flag outside `FASTCALL|KEYWORDS`, keywords without varargs, or an
unknown conv-flag combination. A rejected kind → `hook_register_c_function` returns
0 → registration fails loudly.

The tp_call slot `molt_cfunction_call` (api/object.rs:2955) is the second entry to
the same conventions and **also rejects `METH_METHOD`** (:2969, "served by the
vectorcall path").

The vectorcall path `vectorcall_function` (object.rs:2483) requires **both**
`Py_TPFLAGS_HAVE_VECTORCALL` **and** `tp_vectorcall_offset > 0` (:2491/2495).
Neither is set on `PyCMethod_Type`/`PyCFunction_Type` (verified: `tp_vectorcall_offset`
is never assigned anywhere in the crate; `set_name!` leaves `tp_flags = READY`
only). **Therefore `METH_METHOD` is unhandled by all three paths.**

- **Reachability: LATENT (P3).** numpy `_core` C sources contain **zero**
  `METH_METHOD` uses (verified grep). Fix if a later surface needs it: set
  `HAVE_VECTORCALL` + `tp_vectorcall_offset` on `PyCMethod_Type` and serve
  `METH_METHOD` via the cmethod vectorcall, or add a `defining-class` branch.

The trampoline error paths — non-int closure (:1878), **out-of-range registry id**
(:1892), null args ptr (:1906) — are not completeness gaps; they fire only on a
registration/dispatch bug. The registry itself (`cext_callable_registry`, :1139) is
a **growable `Vec`** with no fixed ceiling, so numpy's 68 module methods + 128
ufuncs + ~484 type-method entries (§4 Rank 4) register without a ceiling reject.

### 3.2 Surface B — reserved wasm-runtime callable table (the "index 2688" origin)

`runtime/molt-runtime/src/builtins/functions/wasm_callables_generated.rs`
(`RESERVED_RUNTIME_CALLABLES`, `RESERVED_WASM_RUNTIME_CALLABLE_COUNT = 24`) maps
runtime callable symbols to fixed table slots. `function_abi.rs:52` resolves a
table index to a reserved callable; an index outside the region →
`None` → the fixed-arity call lane rejects it as a malformed trampoline (the
"reserved index" reject class). The 24 entries cover the object/type/exception/`types`
model constructors + `molt_cpython_abi_cext_call_trampoline` +
`molt_importlib_import_transaction`.

- **`molt_type_new` is present (index 1)** — the M58 "molt_type_new reserved-callable
  frontier" (d) is **CLOSED**.
- **Gap class (latent, P2):** a runtime callable numpy reaches via the fixed-arity
  lanes but NOT among the 24 → reject. The table is generated from
  `runtime/molt-backend-wasm/src/wasm_abi_manifest.toml` (via `tools/gen_wasm_abi.py`)
  and guarded by the generated-file drift gate (`tools/molt_dev_gates.toml:216`).
  The 24 cover init needs; completeness for later surfaces should be asserted by a
  gate that cross-references the numpy-reachable runtime-callable set against the
  table rather than discovered by a witness reject.

---

## Class 4 — numpy/scipy call-surface RANKING (witness-reachability)

Evidence-based ranking of what numpy 2.5.1 `_multiarray_umath_exec` + `_core`
import actually call (source citations under
`bench/friends/repos/numpy_off_the_shelf/numpy/_core/src`), mapped onto the classes
above. Init-reachable = P0.

| Rank | Witness surface (phase) | Volume | Maps to | Status |
|---|---|---|---|---|
| 1 | `PyType_Ready` storm + **builtin slot-copy** (DUAL_INHERIT `tp_hash`/`tp_richcompare` off CPython builtins), multiarraymodule.c:4738–4835,4981–5154 (init) | ~45 Ready + 4 DUAL_INHERIT | **Class 1.3 (P0)** + Class 2 (types raw-registered) | tuple/list/dict richcompare CLOSED; **str/bytes/float/complex hash+richcompare OPEN** |
| 2 | `PyDict_SetItem`/`SetItemString` on module & type dicts (init) | 73+26 static, dozens at init | dict hooks (§1.1) | **CLEAN** (verified) |
| 3 | Tuple `richcompare` in ufunc/cast registration, dispatching.cpp:133/1385, legacy_dtype_implementation.c:41–73 (init) | many | Class 1.3 | tuple CLOSED; sibling container/scalar compares OPEN |
| 4 | **128 ufunc + ~484 `METH_*` + 68 module methods** registration (init) | 484 METH_ (0 METH_METHOD) | Class 3.1 registry (growable, OK) + 3.2 reserved table (OK for init) | no ceiling reject; METH_METHOD not used |
| 5 | `Py_BuildValue` `{}`/`()` units, multiarraymodule.c:4780/4794/4976 (init) | several | (tracked: check_table_drift `Py_BuildValue` inline-C, M45) | — |
| 6 | Foreign-C-object attr/call: `GetAttrString(&PyArray_Type,"__array_finalize__")` :5196; `CallFunction(_add_dtype_helper,"Os",&PyArray_StringDType)` :5228 (init) | tens | **Class 2** (type/dtype objects raw-registered → decode-as-value) | object.rs call path CLEAN; `PyObject_Hash`/`PyFloat_AsDouble` siblings OPEN |
| 7 | `PyObject_Call*` incl. mid-init Python import `npy_import` :4972 (init) | ~99 static | object.rs call path (§2.3) | **CLEAN** (verified) |
| 8 | Python-level dict-iter / `set()` unions / `_sanity_check` array op (import) | — | runtime | later |
| — | **`PySet_*` C-API** | **0 in numpy `_core`** | set hooks | NOT a witness surface |
| — | **`PyNumber_*`** | runtime-only (scalar math), NOT init | number hooks | deprioritize |
| — | **scipy.ndimage** | small `methods[]`, trivial `PyInit__nd_image` | — | later; rank all numpy-init gaps ahead |

---

## Batch-fix lanes (one lane per class — the close-out plan)

| Lane | Class | Scope | Priority |
|---|---|---|---|
| **CLASS1-SLOTS** | 1.3 | Populate builtin type-object slots as a batch: `tp_hash` (generic → `molt_hash_from_bits`) on int/float/complex/str/bytes; `tp_richcompare` (`molt_{str,bytes,long,float,complex}_richcompare`, mirroring `molt_tuple_richcompare`) on str/bytes/int/float/complex; `tp_methods`+`tp_getattro` on str/bytes. Wire in `init_static_types`. Closes the numpy-scalar unhashable/non-comparable frontier batch. | **P0** |
| **CLASS2-DECODE** | 2 | Swap `pyobj_to_handle`→`molt_handle_for_pyobj` on the 7 HIGH + 6 MED bucket-B fast paths (§2.4), route foreign dict keys through `molt_value_for_pyobj`, and gate the ~15 LOW receiver/key sites through `resolve_native_list/_dict`. One sweep, the (c) pattern. Mask-proof test: a raw-registered foreign object (a builtin type / exception / capsule) through each HIGH entry must fall to its foreign slot, never a garbage float/hash/string. Also route the `read_bridge_header_bits` `None`-branch fallbacks foreign-safe. | **P0** |
| **CLASS3-DISPATCH** | 3 | (a) reserved-callable **completeness gate**: assert the numpy-reachable runtime-callable set ⊆ `RESERVED_RUNTIME_CALLABLES` (P2). (b) `METH_METHOD` via `PyCMethod_Type` vectorcall offset+flag (P3, latent). | P2/P3 |
| **CLASS1-HARDEN** | 1.2 | Add `TYPE_ID` guards to `hook_list_append/_item`, `hook_tuple_set/_item`. | P3 |

**Top lanes to run now:** **CLASS1-SLOTS** (Rank-1 witness surface, unconditional at
init) and **CLASS2-DECODE** (Rank-6 surface, reachable via the raw-registered
type/dtype objects every `PyType_Ready` mints). These two close the highest-ranked
remaining init frontiers as batches rather than leaf-by-leaf.

---

*Refresh protocol.* Re-anchor to current `origin/main`; re-run the four greps that
back this doc (vtable field list; `Py*_Type.tp_{hash,richcompare}` assignments;
`pyobj_to_handle(` sites; `RESERVED_RUNTIME_CALLABLES` count) plus the numpy
`DUAL_INHERIT` / `METH_METHOD` source greps. Code beats this doc; a stale hook line
once cost a 30-min detour (MEMORY M-index).
