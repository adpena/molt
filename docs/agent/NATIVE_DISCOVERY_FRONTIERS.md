# Native numpy-import Discovery Frontiers

> **The payoff artifact.** This is the ORDERED list of every frontier real
> numpy `_multiarray_umath` init hits when driven **natively** against molt's
> CPython ABI + real `molt-runtime` hooks — surfaced in **one native sweep**
> (~seconds/frontier) instead of one-per-`~30-min` wasm-witness cycle.
>
> Produced by the **native C-extension discovery engine**
> (`tools/native_numpy_discovery.sh` → `runtime/molt-cext-discovery`). Re-run it
> after each fix; it advances to the next frontier in **~7 s**.

Engine status: **RUNS real numpy 1.26.4 `_multiarray_umath` `PyInit` against
molt's ABI natively** (verified 2026-07-10 on macOS arm64, `tertiary`).
Measured warm edit→frontier cycle: **6.85 s** (vs **~1800 s** for one wasm
witness cycle — ~260×).

## How to run

```bash
# One command: build the harness (incremental), static symbol-gap check,
# then drive PyInit and localise the runtime frontier via MOLT_TRACE_CAPI.
CARGO_TARGET_DIR=<fast-dir> tools/native_numpy_discovery.sh _multiarray_umath
```

Requires a Unix host with real `dlopen`/`RTLD_GLOBAL` (macOS or Linux — NOT
Windows: no flat-namespace). Needs a cp312 numpy wheel unpacked (see the script
header) and molt-runtime buildable natively (see Tier 0).

## Engine architecture (why it works where the old cext harness didn't)

`runtime/molt-cext-discovery` is a **single-static-pool `cdylib`** linking BOTH
`molt-runtime` (which owns `register_cpython_hooks`) AND `molt-lang-cpython-abi`
(the ABI shim + `loader`) into ONE image. A tiny C driver
(`tools/native_cext_driver.c`) `dlopen`s it `RTLD_GLOBAL`, calls
`molt_cext_discovery_init` (registers the **REAL** hooks — not the no-op
`STUB_HOOKS` the old `cext_integration.rs` used), then the loader `dlopen`s the
real prebuilt `_multiarray_umath.so`; its `-undefined dynamic_lookup` `Py*`
imports bind to THIS image, so numpy drives the exact same ABI + hooks the wasm
witness drives — but natively, with real backtraces, in seconds.

---

## Tier 0 — native-compilation blockers (fixed to even build molt-runtime natively)

molt-runtime had **never been compiled for a native unix target** through the
CPython-ABI-hook path; two genuine breaks had to be fixed first. Both are real
molt bugs, not harness artifacts:

| # | File | Bug | Fix |
|---|---|---|---|
| T0.1 | `runtime/molt-runtime/src/lib.rs` | `crate::libc_compat` is re-exported only under `#[cfg(target_arch="wasm32")]`, but native-unix code (`async_rt/process/child_resources.rs`, `async_rt/channels/websocket/tls_native.rs`) does `use crate::libc_compat as libc`. **molt-runtime cannot compile for native unix.** | Alias the real `libc` crate as `libc_compat` on `not(wasm32)` (LANDED in this change). |
| T0.2 | `runtime/molt-runtime-math/src/math.rs` (hypot, ~L2427) | aarch64 NEON block calls `#[target_feature]` intrinsics **without an `unsafe {}` block** — a hard error under edition-2024 / rustc 1.96.1. The x86_64 and dist() branches wrap correctly; hypot's aarch64 branch does not. **molt does not compile on aarch64 with its own pinned toolchain when `stdlib_math` is enabled.** | Wrap the block in `unsafe {}` (NOT landed here — `stdlib_micro` avoids `molt-runtime-math`; recorded for a follow-up aarch64 fix). |

> Note: both offered discovery hosts (`tertiary` macOS, `molt` linux) are
> **aarch64**, so there is no x86 escape from T0.2 — it is a real latent aarch64
> break worth fixing in molt.

---

## Tier A — symbol-gap frontiers (static, discovered instantly)

numpy links **301** `Py*` symbols. molt's ABI is missing or **misnames** the
following. Each is a guaranteed runtime frontier (on macOS `dlopen(RTLD_NOW)`
aborts on the first unresolved DATA symbol — this is why the singletons block
init before it even starts). The harness resolves them (identity-correct aliases
+ loud instrumentation) to advance; the REAL fixes belong in
`molt-cpython-abi`.

### A.1 — Singleton data symbols are **misnamed** (highest severity)

Every C extension references CPython's canonical singleton **struct** symbols.
molt exports them under different names, so **no stock C extension can link
molt's None/True/False/Ellipsis/NotImplemented**:

| numpy needs (C symbol) | molt exports | Fix |
|---|---|---|
| `_Py_NoneStruct` | `Py_None` | `#[export_name = "_Py_NoneStruct"]` (same storage — identity) |
| `_Py_TrueStruct` | `Py_True` | `#[export_name = "_Py_TrueStruct"]` |
| `_Py_FalseStruct` | `Py_False` | `#[export_name = "_Py_FalseStruct"]` |
| `_Py_NotImplementedStruct` | `Py_NotImplementedSentinel` | `#[export_name = "_Py_NotImplementedStruct"]` |
| `_Py_EllipsisObject` | `Py_EllipsisObject` (off by one leading `_`) | `#[export_name = "_Py_EllipsisObject"]` |

This is a foundational, high-value discovery the wasm witness could not surface:
the singleton ABI symbol names are wrong for real C extensions.

### A.2 — Private CPython symbols molt does not export

| numpy needs | Notes |
|---|---|
| `_Py_Dealloc` | object finalization (`Py_DECREF` → 0). molt has no exported `_Py_Dealloc`. |
| `_PyErr_BadInternalCall` | molt exports only the PUBLIC `PyErr_BadInternalCall`. |
| `_PyDict_GetItemStringWithError` | molt exports only `PyDict_GetItemString`. |

### A.3 — `PY_SSIZE_T_CLEAN` variadic variants (numpy defines PY_SSIZE_T_CLEAN)

molt exports the base functions but not the `_SizeT` names numpy actually links.
molt already exports `_Py_BuildValue_SizeT`; these five remain:

`_PyArg_ParseTuple_SizeT`, `_PyArg_ParseTupleAndKeywords_SizeT`,
`_PyArg_VaParseTupleAndKeywords_SizeT`, `_PyObject_CallFunction_SizeT`,
`_PyObject_CallMethod_SizeT`.

They are ABI-identical to the base functions (molt is already ssize-clean), so
the fix is `#[export_name = "_PyArg_ParseTuple_SizeT"]` on the same impls (a
linker `-alias` works for the DATA singletons but NOT for these functions on
macOS ld64). **These 5 are the only unresolved symbols left, and numpy does not
reference them during `_multiarray_umath` init** — they bind lazily at array-op
runtime — so they do not block the init sweep.

### A.4 — Allocators + misc functions molt does not export

`PyObject_Malloc`, `PyObject_Calloc`, `PyObject_Realloc`, `PyObject_New`,
`PyObject_GC_New` (trivially = the C allocator + PyObject header init);
`PyOS_setsig`, `PyThreadState_GetDict`, `PyUnicode_IsWhitespace`,
`Py_HashDouble`, `PyStructSequence_New`, `PyStructSequence_InitType2`; and the
`_Py_ascii_whitespace[128]` classification table.

`PyStructSequence_*` is a genuine feature gap (numpy uses struct-sequences for
e.g. `finfo`/version named tuples).

---

## Tier B — runtime / semantic frontiers (numpy `PyInit` actually running)

With the symbol wall punched through, `PyInit__multiarray_umath` **runs** against
molt's real ABI + hooks. It proceeds through module creation and type setup
(none of the instrumentation stubs for those paths fire), then hits:

### B.1 — `PyCapsule_Import("datetime.datetime_CAPI")` silently returns NULL  ⟵ current numpy-init blocker

```
[MOLT_TRACE_CAPI] call PyCapsule_Import(datetime.datetime_CAPI)
[MOLT_TRACE_CAPI] silent-failure PyCapsule_Import(datetime.datetime_CAPI)
===MOLT_DISCOVERY_STUB_FIRST_CALL: _Py_Dealloc — object finalization
===MOLT_DISCOVERY_FRONTIER (LoadError): InitReturnedNull { name: "_multiarray_umath" }
```

numpy's datetime support does `PyDateTime_IMPORT` →
`PyCapsule_Import("datetime.datetime_CAPI")` to fetch the `datetime` module's C
API capsule. molt has **no importable `datetime` module exposing a
`datetime_CAPI` capsule**, so `PyCapsule_Import` records a silent failure and
returns NULL; numpy `Py_DECREF`s (→ `_Py_Dealloc`) and returns NULL from
`PyInit`. Class: **silent-failure** (M34/M05) — molt should either provide the
`datetime` CAPI capsule or make `PyCapsule_Import` raise a proper `ImportError`.

**To advance past B.1:** provide the `datetime.datetime_CAPI` capsule (molt's
`datetime` module must register it), then re-run — the engine will surface the
next init frontier in ~7 s. This is the whole point: linear 30-min wasm
discovery is now a native sweep.

---

## Reproduction / measured cycle time

| Step | Time |
|---|---|
| Cold build (`molt-runtime` `stdlib_micro` + `molt-cpython-abi` + harness) | ~minutes (dominated by `molt-runtime`) |
| **Warm edit → next frontier** (`touch` a source file → rebuild → run) | **6.85 s (measured)** |
| One numpy/scipy **wasm witness** frontier cycle (baseline) | ~1800 s (~30 min) |

Host: `tertiary` (Tailscale `100.65.24.39`), macOS 26.5 arm64, rustc 1.96.1,
numpy `1.26.4` cp312 wheel. Worktree `~/molt-disc` off `origin/main`.
