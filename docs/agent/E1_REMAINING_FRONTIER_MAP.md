# E1 Remaining Runtime and Compute Frontier Map

Date: 2026-07-11  
Lane: `E1-FRONTIER-MAP`  
Authority: current `origin/main` at `08cd807242` plus native Linux probes from this worktree.

## Executive result

The requested all-at-once native execution is **not currently capable of producing a per-op pass/fail matrix**. Two independent facts bound the result:

1. The standalone C-extension discovery engine drives a real NumPy 2.5.1 `_multiarray_umath` `PyInit`, but its micro-runtime has no AOT registry entry for NumPy's pure-Python siblings. On current main it stops at `PyImport_ImportModule("math")` in the harness; the full witness has already advanced much farther. This is a discovery-harness limitation, not an E1 runtime frontier.
2. The full native Molt program could not be built on Windows current main because `molt-lang-cpython-abi` links both `molt_pyarg_shims.lib` and `libmolt_pyarg_shims.a`, producing duplicate variadic C-API symbols. The separate wasm witness is presently stopped before execution by the operator-owned NumPy libc++/long-double link lanes. Therefore no current-main execution reaches `field_solve` operations.

The map below is consequently split into: **confirmed remaining runtime classes**, **resolved/stale frontiers**, **complete static extension surface**, and an **honest operation matrix whose execution state is UNKNOWN rather than guessed**.

## Evidence

- Native NumPy 2.5.1 drive: `tmp/e1_numpy_2_5_1_discovery.log`.
- Native remaining ABI repros: `tmp/e1_frontier_repro_ignored.log`.
- Windows full-native attempt: proof rows `20260711T105720-e1-frontier-import-801d573350a34fde` and `20260711T105901-e1-frontier-import-rerun-aa6efb0a528f4362`.
- Existing whole-extension inventory: `docs/agent/NATIVE_DISCOVERY_FRONTIERS.md`.
- Kernel authority: `collab/pact/pact_witness_kernel/field_solve.py`.

## Ordered batch-attack frontier set

### 1. `PyLong_AsLong` overflow semantics

- **Exact divergence:** native repro passes `2**31+5`; Molt returns `2147483653` with no pending exception. The CPython contract for an out-of-range C `long` is `-1` with `OverflowError` set.
- **Classification:** missing C-API numeric conversion semantics; silent wrong-value frontier.
- **CLASS vs leaf:** **CLASS**. It affects shape, stride, axis, index, buffer-size, and C-extension argument conversions, not one NumPy call.
- **Source references:** Molt `runtime/molt-cpython-abi/tests/frontier_repro.rs` (`frontier_08_pylong_aslong_silent_overflow`); CPython `Objects/longobject.c` conversion family; NumPy array/index paths consume `PyLong_AsLong` broadly.
- **Suggested fix:** route inline and foreign integers through one checked signed-long conversion authority; set `OverflowError` on range failure; sweep `PyLong_AsLong`, `PyLong_AsUnsignedLong`, `PyLong_AsSsize_t`, and mask variants for shared range policy.
- **Rough size:** M (one shared conversion authority plus a family sweep and tests).
- **Wasm risk:** low platform divergence; integer-width differences make wasm32 especially sensitive.

### 2. `PyObject_Str` / `PyObject_Repr` dispatch semantics

- **Exact divergence:** native repro of `PyObject_Str(2147483653)` returns an empty/theater string instead of `"2147483653"`.
- **Classification:** incomplete object protocol dispatch; wrong-value frontier.
- **CLASS vs leaf:** **CLASS**. NumPy/SciPy use string/repr conversion for dtype names, error construction, warnings, signatures, and diagnostics.
- **Source references:** Molt `runtime/molt-cpython-abi/tests/frontier_repro.rs` (`frontier_06_pyobject_str_theater`); CPython `Objects/object.c` `PyObject_Str`/`PyObject_Repr`.
- **Suggested fix:** implement the canonical null/string fast paths and `tp_str`/`tp_repr` dispatch, including recursion/error propagation; delete fallback theater values.
- **Rough size:** M.
- **Wasm risk:** low; platform-independent ABI semantics.

### 3. CPython extension dynamic export surface

The exact NumPy 2.5.1 ELF wheel drive reports 21 unresolved dynamic names:

`PyArg_ParseTuple`, `PyArg_ParseTupleAndKeywords`, `PyArg_UnpackTuple`, `PyErr_Format`, `PyErr_WarnFormat`, `PyOS_snprintf`, `PyOS_string_to_double`, `PyOS_strtol`, `PyOS_strtoul`, `PyObject_CallFunctionObjArgs`, `PyTuple_Pack`, `PyUnicode_FromFormat`, `PyUnicode_FromFormatV`, `Py_BuildValue`, `_PyArg_ParseTupleAndKeywords_SizeT`, `_PyArg_ParseTuple_SizeT`, `_PyArg_VaParseTupleAndKeywords_SizeT`, `_PyLong_Sign`, `_PyObject_CallFunction_SizeT`, `_PyObject_CallMethod_SizeT`, `_Py_BuildValue_SizeT`.

- **Exact failure mode:** these names are absent from the Linux `molt-cext-discovery` dynamic symbol table. The current Windows full-native build simultaneously fails because the same variadic shim archive is linked twice.
- **Classification:** export/link custody, not 21 semantic leaves.
- **CLASS vs leaf:** **CLASS**, with two sub-classes: variadic shim ownership/export and private/public alias/export completeness.
- **Source references:** NumPy 2.5.1 `_multiarray_umath` undefined symbol table; Molt `runtime/molt-cpython-abi/shims/pyarg_variadic.c`; `tools/native_numpy_discovery.sh`.
- **Suggested fix:** establish one archive and one export authority per target. Export the C shim symbols from the discovery cdylib; stop linking both `.lib` and `.a` on MSVC; generate SizeT aliases and export lists from the same manifest.
- **Rough size:** M-L because the fix must cover ELF, Mach-O, MSVC, static wasm, and the discovery harness without duplicate ownership.
- **Wasm risk:** high difference in manifestation. Static wasm may already resolve symbols that the ELF cdylib does not export; the duplicate-archive failure is Windows-only.

### 4. Full-package native discovery provider

- **Exact error:** the 2.5.1 discovery drive reaches `PyCapsule_Import(datetime.datetime_CAPI)`, then `PyImport_ImportModule(math)` and returns `ImportError: import of 'math' failed (runtime import error pending)`.
- **Classification:** discovery instrumentation/AOT registry limitation, **not a remaining full-witness runtime bug**.
- **CLASS vs leaf:** **CLASS** in the reconnaissance harness: every pure-Python NumPy/SciPy sibling import is unavailable unless compiled into the registry.
- **Source references:** `runtime/molt-cext-discovery`, `tools/native_numpy_discovery.sh`, and the honest-limit section of `docs/agent/NATIVE_DISCOVERY_FRONTIERS.md`.
- **Suggested fix:** if this reconnaissance mode is retained, build a source-derived AOT module provider for the exact sealed NumPy/SciPy closure. Do not add filesystem fallback or fake modules.
- **Rough size:** L.
- **Wasm risk:** none as a witness bug; the wasm/full-AOT pipeline already owns package closure.

### 5. Field-solve compute execution frontier

- **Exact state:** **UNKNOWN on current main**. No current-main native or wasm execution reached the first `field_solve` operation. Any claim that individual ops pass, trap, or diverge would be fabricated.
- **Classification:** an execution aperture blocked by link/export custody before runtime; once opened, failures should be assigned by backing extension class, not by Python API leaf.
- **CLASS vs leaf:** three parallel compute classes:
  1. NumPy core ndarray/ufunc/multiarray operations (`_multiarray_umath`).
  2. NumPy linear algebra (`_umath_linalg` plus `lapack_lite`).
  3. SciPy ndimage (`_nd_image` plus PEP-489 `_ni_label`).
- **Suggested fix approach:** after the two operator-owned wasm link lanes land, run one instrumented witness with per-op stage markers and persist every output before parity. In parallel, repair the native export/link class so the same operation matrix can run on Linux and Windows.
- **Rough size:** discovery S once execution is available; fixes are unknown until observed.

## Resolved or stale frontiers (do not dispatch again)

### Indexed-loop registration: `cannot add indexed loop to ufunc add with NPY_BYTE`

- **Status:** **FIXED on current main**, not remaining.
- **Root cause:** `PyTuple_Type.tp_richcompare` was null, so NumPy's `get_info_no_cast` compared distinct-but-equal DType tuples by identity and failed to find the registered indexed loop.
- **Fix present:** `runtime/molt-cpython-abi/src/api/sequences.rs::molt_tuple_richcompare`, wired in `abi_types.rs`; native `ufunc_frontier_tuple_structural_richcompare` now passes.
- **NumPy source:** `_core/src/umath/dispatching.cpp` `get_info_no_cast`; generated ufunc initialization from `_core/code_generators/generate_umath.py`.
- **Why it still appears as “latest canonical”:** the later witness cannot execute far enough to revalidate because separate libc++/long-double link frontiers intervene.

### `_umath_linalg` and `_ni_label`

- **Status:** build/seal/loader custody is present; **actual `eigh` and `label` execution is not proven by this reconnaissance**.
- `_umath_linalg` + `lapack_lite` artifacts were sealed for the witness, but artifact existence is not numerical execution proof.
- `_ni_label` PEP-489 loader support and a wasm artifact were sealed, but artifact existence is not connected-component correctness proof.

## Field-solve operation matrix

`UNKNOWN` means the operation was not executed under current Molt main. CPython reference generation is green, but that is not Molt evidence.

| operation | backing class | current Molt result | source route / likely failure class |
|---|---|---|---|
| `np.asarray`, `zeros`, `astype`, `nonzero` | NumPy core | UNKNOWN | ndarray construction/conversion in `_multiarray_umath`; sensitive to integer conversion and item protocol classes |
| `np.sort`, `argsort`, `lexsort`, `argmax` | NumPy core | UNKNOWN | `numpy/_core/fromnumeric.py` and `multiarray.py` -> ndarray methods/C sort machinery |
| `np.where`, `bincount`, `clip`, `stack` | NumPy core | UNKNOWN | `multiarray.py`, `fromnumeric.py`, `shape_base.py`; ufunc/multiarray dispatch |
| `np.gradient`, `percentile` | NumPy core + Python closure | UNKNOWN | `numpy/lib/_function_base_impl.py`; broad ndarray arithmetic/sort/indexing |
| boolean masks, fancy indexing, slices | NumPy core | UNKNOWN | C mapping/item slots; class-level risk, not a leaf |
| `np.linalg.eigh` | linalg | UNKNOWN | `numpy/linalg/_linalg.py:eigh` -> `_umath_linalg` gufunc + `lapack_lite` |
| `ndimage.distance_transform_edt` | SciPy ndimage | UNKNOWN | `_morphology.py` -> `_nd_image.euclidean_feature_transform` |
| `ndimage.gaussian_filter` | SciPy ndimage | UNKNOWN | `_filters.py` -> repeated `_nd_image.correlate1d` |
| `ndimage.maximum_filter`, `minimum_filter` | SciPy ndimage | UNKNOWN | `_filters.py` -> `_nd_image.min_or_max_filter*` |
| `ndimage.label` | SciPy label | UNKNOWN | `_measurements.py` -> `_ni_label._label`; PEP-489 extension |

## Counts

- **Confirmed remaining real runtime classes:** 3 (`PyLong` checked conversion, object str/repr dispatch, extension export/link custody).
- **Reconnaissance-harness-only classes:** 1 (AOT pure-Python module provider).
- **Latent compute classes awaiting execution:** 3 (NumPy core, NumPy linalg, SciPy ndimage/label; split into independent lanes after first execution evidence).
- **Confirmed isolated leaves:** 0. The evidence supports classes, not one-off operation patches.
- **Resolved/stale frontier:** 1 indexed-loop registration class; do not reassign.

## Recommended batch ordering and parallelization

1. **Link lanes already owned:** finish NumPy libc++ and long-double wasm link work; this is the shortest path to authoritative execution.
2. **Parallel runtime lane A:** checked integer-conversion family centered on `PyLong_AsLong`.
3. **Parallel runtime lane B:** `PyObject_Str`/`PyObject_Repr` protocol family.
4. **Parallel DX/runtime lane C:** unify variadic shim archive/export custody across MSVC, ELF, Mach-O, and wasm; this restores native all-at-once iteration.
5. **After execution opens, dispatch by backing extension, not by operation:** NumPy core; NumPy linalg; SciPy `_nd_image`; SciPy `_ni_label`. Each lane runs all sibling operations and parity fields owned by that extension.

## Acceptance rule

This map is reconnaissance, not acceptance. The only final authority remains a real `field_solve.py` Molt-WASM run producing `candidate_outputs.npz` and passing `collab/pact/pact_witness_kernel/check_parity.py` (via the canonical proof-queue acceptance lane).
