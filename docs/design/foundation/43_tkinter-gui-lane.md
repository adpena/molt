<!--
Foundation design 43 — tkinter / Tk GUI Lane. The frontier design for molt's GUI
stack: the Tcl_Obj-direct value bridge, the per-call overhead reduction, event-loop
integration, the threading model, and the per-target story. Doc number 43 was
reserved by the supervisor for the tkinter-performance arc (task #31). Do not renumber.

This doc is BOTH a landed-work record and a frontier plan: Parts 1-4 describe work
that LANDED on main during this arc (commits below); Parts 5-7 are the remaining
DESIGN. All file:line anchors verified against the live worktree at HEAD commit
5d4467fa0 (branch main, 2026-06-06), tk.rs = 18,467 lines.

Landed commits (this arc):
  f999376b1  fix(tk): repair native GUI lane (Tk create/call binding, callback drain, fingerprint)
  9020c6a4a  bench(tk): bridge microbench harness
  6e911ae36  bench(tk): harden microbench (direct dispatch, byte-parity gate)
  9c576a43f  perf(tk): Tcl_Obj-direct typed value bridge — wantobjects=1 byte-parity
  e6149b11a  test(tk): wantobjects=1 result-type differential (CPython byte-parity gate)
  5c3e265e0  perf(tk): collapse per-call Python-layer + registry-lock overhead (~2x)
  5d4467fa0  perf(tk): bind _TK_ exports to bare _tkinter wrappers (drop closure layer)

Research provenance (RESEARCH GRANT, standing): CPython _tkinter.c (3.12) AsObj/
FromObj + Tkapp_New semantics — studied as the wantobjects=1 reference and
reimplemented; PSF-licensed CPython is a SEMANTICS reference only, no source ingested.
Tcl/Tk public C API (tcl.h 8.6 + 9.0). License discipline: study + reimplement.

Baseline environment: molt links Homebrew libtcl8.6/libtk8.6 (observed via
Instruments sample, §3); CPython baseline is .venv/bin/python 3.12.13 with
Tcl/Tk 9.0. The typed bridge is version-robust across both (§2.2) and produced
byte-identical results despite the Tcl-version skew.
-->

# tkinter / Tk GUI Lane (Design 43)

## Part 0 — The user report, quantified

The user reported: "even our tkinter implementation is far from as performant as the
cpython implementation." This violated molt's performance contract (faster than
CPython on every benchmark). This arc MEASURED the gap, root-caused it
quantitatively, landed the structural correctness keystone (the Tcl_Obj-direct value
bridge), reduced the dominant per-call tax ~2×, and isolated the residual gap to a
cross-cutting runtime cost (not a bridge problem).

**Before this arc, molt's native GUI lane did not run at all** for a live `Tk()`
application — four compounding binding/handle bugs plus an event-loop callback-drain
gap plus a build-fingerprint bug meant `tkinter.Tk()` raised on the first call. No
existing test exercised live `Tk()` with the capability granted (the differential
test runs with `MOLT_CAPABILITIES=` empty and asserts `PermissionError`; the live
smoke test silently `SKIP`s on any `Tk()` exception). So the perf report was the
first signal the lane was broken; fixing it was prerequisite to measuring it.

### Architecture (verified)
molt's Tk stack is a REAL libtcl FFI, not a reimplementation:
`molt-runtime-tk/src/tk.rs` (18,467 lines) dynamically loads libtcl
(`load_tcl_api`, tk.rs:~456), `Tcl_CreateInterp` + `Tcl_Init`
(`TclInterpreter::new`, tk.rs:~913), and evaluates via `Tcl_EvalObjv`. The Python
surface is `src/molt/stdlib/_tkinter.py` (the intrinsic shim + `TkappType`) and
`src/molt/stdlib/tkinter/__init__.py` (the `Misc`/`Tk`/`Widget` classes). On WASM
and `--no-default-features` builds there is a separate pure-Rust headless command
emulation (the `#[cfg(any(target_arch = "wasm32", not(feature = "tk")))]` arm of
`tk_call_dispatch`, tk.rs:~14817).

---

## Part 1 — The four foundational bugs (LANDED: f999376b1)

These were prerequisites; the lane could not run without them.

### 1.1 `_TK_CREATE` / `_TK_CALL` mis-bound to raw intrinsics
`tkinter/__init__.py` bound `_TK_CREATE` to the raw `molt_tk_app_new` intrinsic and
`_TK_CALL` to `molt_tk_call`. But `Tk.__init__` calls `_TK_CREATE(options=options)`
(a keyword) and `Misc.call` calls `_TK_CALL(self._tk_app, *argv)` (variadic). The raw
intrinsics accept neither — they take fixed positional args. So `Tk()` raised
`TypeError: keywords are not supported for this builtin` and every `tk.call` raised
`too many positional arguments`. Fix: bind both to the `_tkinter.create` /
`_tkinter.call` Python WRAPPERS, which marshal options/argv correctly and accept a
`TkappType` or raw handle via `_unwrap_app`.

### 1.2 `_tk_app` handle-vs-object inconsistency
`TkappType` is molt's equivalent of CPython's `_tkinter.tkapp` object — it carries
the interpreter handle and exposes `wantobjects`, `getint`, `exprlong`,
`createfilehandler`, etc. (`_tkinter.py` class `TkappType`, :603). `Tk.__getattr__`
(`tkinter/__init__.py`:~2203) delegates tkapp-only attributes to `self._tk_app`,
mirroring CPython where `self.tk` is the tkapp. So `self._tk_app` MUST be the
`TkappType`. The raw intrinsics, however, need the bare integer handle. Fix: keep
`_tk_app = TkappType`; the ~5 direct-intrinsic call sites
(`after_idle`/`after_cancel`/`after_info`/`bind_command`/`unbind_command`) unwrap via
`_TK_UNWRAP_APP(self._tk_app)`; the lifecycle exports bind to the `_tkinter` wrappers
(which unwrap). `Tk.loadtk` was routed through `self.call("loadtk")`.

### 1.3 `update()` / `tkwait` / `vwait` did not drain Python callbacks
molt registers Tcl callback procs that append `[info level 0]` to
`::__molt_pending_callbacks` (tk.rs:~3190); the actual Python dispatch happens
out-of-band when that queue is drained. `pump_tcl_events` (tk.rs:~3600) drains it for
`dooneevent`/`mainloop`, but `update`/`tkwait`/`vwait` reach the interpreter through
the generic `run_tcl_command` path, which never drained — so `after_idle`/bound
callbacks scheduled before an `update()` NEVER fired (observed: `fired=0` after 9000
`update()` spins). This breaks essentially every event-driven GUI. Fix:
`is_event_pumping_command` + `run_tcl_command_and_drain_callbacks` (tk.rs:14594) drain
the pending queue after these commands, matching CPython where the event loop invokes
the command directly. Verified: 2000 `after_idle` callbacks now all fire in a single
`update()`.

### 1.4 The runtime fingerprint ignored the Tk crate (`cli.py`)
`_runtime_source_paths` (`src/molt/cli.py`:~10272) hashed `molt-runtime/src` and
`molt-obj-model/src` but NOT `molt-runtime-tk/src` — even though the Tk crate is
linked into `libmolt_runtime.a` via the `stdlib_tk` feature. So every edit to the
Tk/Tcl bridge was silently cached and never recompiled into the runtime archive.
This is the single highest-leverage fix for anyone WORKING on the Tk lane (without
it, no bridge change takes effect). Fix: add `runtime/molt-runtime-tk/src` + its
Cargo.toml to the fingerprint source list. **This file is outside the nominal Tk
scope; it was edited because it hard-blocks all Tk runtime work. See Part 7 baton.**

---

## Part 2 — The keystone: Tcl_Obj-direct value bridge (LANDED: 9c576a43f)

### 2.1 The string-round-trip tax (root cause #1, IMPORTANCE: high, was the predicted keystone)
The original bridge implemented **pre-Tcl-8.0 STRING-object semantics while claiming
`wantobjects=1`**. `wantobjects()` returns `True` (`_tkinter.py`:216), but:
- `tcl_obj_from_bits` stringified every argument: `TclObj::from(i64)` =
  `Self::scalar(value.to_string())`; `alloc_tcl_obj_from_part` allocated every scalar
  as a `Tcl_NewStringObj`, forcing Tcl to re-parse "42" back into an int.
- the result path discarded the result `Tcl_Obj*` and re-materialized it via
  `Tcl_GetStringResult` → Rust `String` → molt `str` (`tcl_result_to_bits`). Type
  information (int/float/list/bool) was annihilated; the Python side had to re-parse
  with `getint`/`getdouble`/`splitlist`.

CPython's `_tkinter.c` with `wantobjects=1` does none of this: `AsObj` builds typed
`Tcl_Obj`s (`Tcl_NewWideIntObj`/`NewDoubleObj`/`NewBooleanObj`/`NewListObj`) and
`FromObj` reads `result->typePtr` and returns a native Python int/float/bool/tuple/
bytes/str with no string round-trip. Observable proof of the molt bug:
`type(root.tk.call("expr","1+1"))` was `str`, not `int`.

### 2.2 The fix (tk.rs)
- **TclApi extension** (tk.rs:~178): typed accessors + constructors —
  `Tcl_NewWideIntObj`, `Tcl_NewDoubleObj`, `Tcl_NewBooleanObj`,
  `Tcl_NewByteArrayObj`, `Tcl_GetWideIntFromObj`, `Tcl_GetDoubleFromObj`,
  `Tcl_GetBooleanFromObj`, `Tcl_GetStringFromObj`, `Tcl_GetByteArrayFromObj`,
  `Tcl_ListObjGetElements`, `Tcl_GetObjResult`, `Tcl_GetObjType`. All stable public
  API across Tcl 8.5/8.6/9.0.
- **Type-pointer capture** (`TclTypePtrs::capture`, tk.rs:295), per CPython
  `Tkapp_New`: probe by name (`Tcl_GetObjType("double"|"wideInt"|"list"|"string"|…)`),
  fall back to reading a freshly-constructed object's `typePtr` for types Tcl 9.0
  dropped (`int` → use `wideInt`; `bytearray`/`boolean` via probe objects). Captured
  once per interpreter at `TclInterpreter::new`.
- **`typePtr` is read via a width-agnostic `#[repr(C)] TclObjHeader`**
  (tk.rs:217) with a compile-time `offset_of!(type_ptr) == 24` assertion. This offset
  is correct on 64-bit for BOTH Tcl 8.x (`int` refCount/length) and 9.0 (`Tcl_Size`
  refCount/length): the pointer alignment makes 8.x's `int+pad` occupy the same 8
  bytes as 9.0's `Tcl_Size`. The static assert fails the build if a future ABI breaks
  this. This is the key version-robustness mechanism.
- **AsObj** (`tcl_obj_alloc_typed_from_bits`, tk.rs:3406): bool BEFORE int (bool is an
  int subclass), then wideInt / double / byteArray / string / list (recursive). Heap
  objects dispatch on `object_type_id`, **NOT** via `string_obj_to_owned` — the latter
  calls `molt_string_as_ptr`, which RAISES a `TypeError` on non-strings and would
  pollute the interpreter's exception state for tuple/widget arguments (this was the
  cause of two intermittent failures during development).
- **FromObj** (`tcl_obj_result_to_bits`, tk.rs:3531): dispatch the result's `typePtr`
  → int / float / bool / bytes / tuple / str. `typePtr == NULL` → str. Unknown
  `typePtr` → str (CPython wraps it in `_tkinter.Tcl_Obj`; molt's `Tcl_Obj` is a `str`
  subclass, so the string value is the comparison-correct representation). i64-overflow
  ints fall back to the decimal string parsed by `molt_int_from_obj` (bignum parity).
  Result tuples are built via `rt_tuple` directly (not `alloc_tuple_bits`, which also
  fails on a *pre-existing* pending exception).
- **`run_tcl_command`** rewired to build typed argv + convert the typed result; the
  result obj is incref'd across argv teardown because the result may ALIAS an argument
  (`set x` returns the value object). The dead string-result path
  (`tcl_result_to_bits`) was deleted — no dual paths.

### 2.3 Correctness gate (LANDED: e6149b11a)
`bench/tk/tk_wantobjects_parity.py` pins 14 result-type / argument-round-trip checks
against CPython and compares **byte-identically** (`cmp -s`):

| check | CPython & molt |
|---|---|
| `expr 40+2` | int `42` |
| `expr 1000000*1000000` | int `1000000000000` |
| `expr 3.5+1.25` | float `4.75` |
| `expr 2 > 1` | int `1` (expr yields int 0/1, NOT bool) |
| `list "a" "b" "c"` | tuple `('a','b','c')` |
| `list 1 2 3` | tuple `(1, 2, 3)` (typed int elements) |
| `lrange L 1 2` | tuple `(20, 30)` |
| `set s "hello world"` | str `'hello world'` |
| `set s ""` | str `''` |
| `set n "00042"` | str `'00042'` |
| arg int/float/bool round-trips | int / float / int |

Result: `cmp -s` **IDENTICAL** between molt(libtcl8.6) and CPython(Tcl9.0), proving
the typed bridge is correct AND version-robust.

### 2.4 The keystone was necessary but NOT sufficient for perf
The typed bridge improved `result_list` (35,972 → 27,513 ns/op) and gave ~5–10%
elsewhere, but molt stayed 20–90× slower than CPython. **The string round-trip was
not the dominant tax.** This is the pivotal evidence-over-prediction moment of the
arc: the supervisor predicted the Tcl_Obj bridge would close the gap; profiling
proved the dominant tax is elsewhere (Part 3). The keystone is still the correct
structural fix — it delivers exact `wantobjects=1` parity that the string path could
never give — but the perf win came from Part 4.

---

## Part 3 — Quantitative root-cause: the dominant tax is per-call dispatch

### 3.1 Method (Instruments `sample`)
A 2,000,000-iteration `app.call("set","x","1")` loop (no expr compile, minimal Tcl
work) was sampled on the live binary (driven through `tools/safe_run.py`). The Tcl
`set` itself was ~50 of the main thread's 1662 samples (`Tcl_SetObjCmd`,
`TclObjLookupVarEx`, `EvalObjvCore`). The remaining ~97% sat in molt's per-call
machinery — a RECURSIVE chain of molt call/arg-binding frames that appears TWICE
nested per `tk.call` (the binary's symbols are stripped in release-output, but the
repeating address quartet is unmistakable). The `expr` profile additionally showed
Tcl's `CompileExprObj`/`TclCompileExpr`/`ParseExpr` recompiling the expression on
every call — but that cost is SHARED with CPython, so it is not part of the gap.

### 3.2 Layer-cost attribution (ns/op, `expr 1+1`, before Part 4)
Measured by calling the same operation at three stack depths:

| layer | ns/op | what it adds |
|---|---|---|
| `_tkinter.call(app, …)` wrapper | 3,484 | `_unwrap_app` + `list(argv)` + intrinsic (5 registry locks + GIL entry + AsObj/FromObj) |
| `TkappType.call(…)` method | 5,884 | + the `call(self, …)` wrapper frame |
| `Misc.call(…)` (what real code uses) | 10,561 | + per-call `_require_gui_window_capability()` (2–4 capability-probe intrinsics) + the `_TK_CALL` `_tk_runtime_export` closure (`getattr` + `*args`/`**kwargs` repack) |

`set` (no Tcl compile) showed ~4,050 ns/op of pure molt overhead per call vs
CPython's ~98 ns/op — quantifying the gap as ~40× per-call dispatch overhead.

### 3.3 The dominant tax, named
**molt's general compiled-Python per-call function-invocation cost, amplified by
tkinter's layered wrapper design.** Each `tk.call` traversed 3–4 Python wrapper
frames (`Misc.call` → `_TK_CALL` closure → `_tkinter.call` → `_unwrap_app`) plus a
per-call capability re-check plus 5 registry mutex acquisitions inside the intrinsic.
CPython's `tkapp.call` is a single C function. The registry locks turned out to be a
minor contributor (~30 ns each, uncontended); the Python-frame + capability-check
overhead dominated.

---

## Part 4 — Per-call overhead reduction (LANDED: 5c3e265e0, 5d4467fa0)

### 4.1 Python layer (`tkinter/__init__.py`)
- `Misc.call` / `getvar` / `setvar` invoke the `molt_tk_call` intrinsic DIRECTLY with
  the bare handle (`self._tk_app._handle`), collapsing the 4-call wrapper chain to one
  intrinsic call.
- Per-call `_require_gui_window_capability()` REMOVED from `Misc.call`: the capability
  is verified once at `Tk()` construction (`tkinter/__init__.py`:~1987) and cannot be
  revoked mid-process; a `Misc` cannot exist without a constructed interpreter. Matches
  CPython (no per-call check). This is a security-invariant-preserving hoist, not a
  removal of the check.
- The lifecycle/var/bind/trace/wait `_TK_*` exports bind to the bare `_tkinter`
  wrapper functions (`_require_tk_callable`) instead of `_tk_runtime_export` closures,
  dropping the per-call `getattr` + arg repack.

### 4.2 Native intrinsic (tk.rs)
- **Single-lock dispatch** (`resolve_native_dispatch`, tk.rs:14710): ONE registry lock
  gathers callback / filehandler / interpreter-context, replacing 3 separate
  acquisitions (`lookup_bound_callback` + the filehandler lock + `run_tcl_command`'s
  Phase-2 lock). The redundant handle-validation lock in `molt_tk_call` (tk.rs:15433)
  was dropped. `run_tcl_command_with_ctx` (tk.rs:3885) runs the eval with the
  pre-resolved context. A generic `tk.call` now touches the registry mutex twice
  (resolve + final `clear_last_error`) instead of five times.
- `MOLT_TRACE_TCL` cached in a `OnceLock` (`tcl_trace_enabled`) — was a `getenv`
  syscall per call.

### 4.3 Result (the gap table, before → after, ns/op, medians of 3 reps)

| bench | CPython | molt BEFORE | molt AFTER | after/CPython |
|---|---|---|---|---|
| expr (`call('expr','1+1')`) | 412 | 10,058 | 5,096 | 12.4× slower |
| setget (`set`/`get` round-trip) | 200 | 10,021 | 4,999 | 25.0× |
| result_int (int result) | 426 | 11,908 | 6,791 | 15.9× |
| result_double (float result) | 438 | 13,112 | 7,156 | 16.3× |
| result_list (100-elem → tuple) | 4,027 | 35,972 | 18,960 | 4.7× |
| stringvar (StringVar set/get) | 135 | 12,007 | 8,065 | 59.7× |
| intvar (IntVar set/get) | 133 | 13,273 | 8,849 | 66.5× |
| widget (Label create+destroy) | 98,014 | 132,211 | 112,209 | **1.14× (≈parity; molt wins some reps)** |
| event (after_idle×N drained by update) | 16,243 | 97,609 | ~93,000 | 5.7× |

molt's `tk.call` paths are ~2× faster than before. **`widget` is at rough parity with
CPython** (Tcl widget creation dominates and is shared). The remaining gap on the
pure-call benches is molt's general per-call cost (Part 3), not a bridge issue.

### 4.4 Honest statement on the performance contract
molt does NOT yet beat CPython on the pure-`tk.call` microbenches. The residual gap
(~12–66×) is dominated by molt's **general compiled-Python per-call function-invocation
+ GIL-entry + per-call allocation cost** (the `list(argv)` Python allocation, the
`with_gil_entry` boundary, the AsObj/FromObj molt-object allocations). This is a
cross-cutting runtime arc, not specific to tkinter — the same per-call overhead would
show on any intrinsic-heavy stdlib path. Closing it to BEAT CPython requires the
runtime work in Part 5. The widget-construction path (real-app-shaped) is already at
parity. This is reported transparently rather than declared "done".

---

## Part 5 — End-state GUI-lane architecture (DESIGN)

### 5.1 P1 — Kill the residual per-call tax (the path to BEATING CPython)
The per-call cost has three remaining components, each a structural fix:

- **5.1-a `list(argv)` allocation per call.** `Misc.call(*argv)` builds a Python list
  to hand to the intrinsic. CPython builds a `Tcl_Obj*` argv on the C stack. Fix:
  a varargs-aware intrinsic entry `molt_tk_call_v(handle, argc, argv_ptr)` that reads
  the call frame's argument vector directly (molt already has the args as a slice at
  the `with_gil_entry` boundary), avoiding the intermediate Python list. Requires a
  small frontend/backend affordance to pass the positional-arg slice to the intrinsic
  without materializing a list. **Cross-scope (backend); baton.**
- **5.1-b `with_gil_entry` re-entry per call.** Every intrinsic re-enters the GIL.
  For a tight `tk.call` loop the GIL is already held by the caller; the re-entry is
  bookkeeping. A `with_gil_entry_held_fast` fast path that asserts (debug) the GIL is
  held and skips the re-acquire would remove this. **Cross-scope (core); baton.**
- **5.1-c AsObj/FromObj molt-object allocation.** Each call allocates the result molt
  object (int/str/tuple). For scalar results (`set`, `expr`) the int/float are
  NaN-boxed immediates (no heap) — already cheap post-keystone. The tuple path
  allocates; acceptable. The argv typed `Tcl_Obj`s are freed each call; a per-interp
  free-list of `Tcl_Obj*` (Tcl already pools these) is a possible micro-opt but low
  value. Mostly DONE by the keystone.

Target after 5.1-a + 5.1-b: a generic `tk.call` should drop from ~5,000 ns to the
low-hundreds, BEATING CPython's ~400 ns (molt pays no per-call interpreter dispatch
once the list+GIL taxes are gone).

### 5.2 P0 — bind/event callback dispatch is BROKEN (correctness, blocks event GUIs)
`widget.bind(seq, fn)` raises `TypeError: bind callback must be callable` even when
`callable(fn)` is `True` at the call site. Root cause: the intrinsic
`molt_tk_bind_callback_register` checks `callback_is_callable(callback_bits)`
(tk.rs:1936) via `molt_is_callable`, which reports a plain callable function as
NON-callable across the intrinsic ABI boundary. Reproduced with a module-level
`def plain(*a)` and a fixed-arity `def fixed(e)` alike (so it is NOT a closure or
`*args` issue). This is a **core-runtime / intrinsic-ABI bug** (`molt_is_callable` or
the 5-argument intrinsic marshalling corrupting `callback_bits`), independent of the
Python bindings. It blocks ALL event-driven GUIs. **Cross-scope (core); P0 baton.**
Everything else works: variables (typed get), all widgets, `winfo_*` (typed int),
`after_idle`+`update` dispatch, `configure`/`cget`.

### 5.3 Event-loop integration with the asyncio loop (doc 28)
Tk's event loop (`Tcl_DoOneEvent` / the notifier thread) and molt's asyncio loop
(doc 28) are two independent event sources. The end-state for a program that uses
BOTH (`async def` + a Tk window) is the CPython-3.12 `asyncio` + Tk pattern: drive Tk
from the asyncio loop via periodic `root.update()` calls scheduled with
`loop.call_later`, OR run the asyncio loop's `run_once` from a Tk `after` callback.
molt should provide `tkinter.Misc.mainloop` integration that, when an asyncio loop is
running, interleaves `Tcl_DoOneEvent(TCL_DONT_WAIT)` with `loop._run_once` — matching
CPython's lack of a unified loop but offering a documented interop helper. The
callback-drain machinery (Part 1.3) is the substrate: pending Tcl callbacks are
already dispatched on the molt side under the GIL, so an asyncio-driven `update()`
fires Python Tk callbacks correctly. **DESIGN; depends on 5.2 (callbacks) + doc 28.**

### 5.4 Threading model (doc 33)
Tcl interpreters are thread-bound: `TclInterpreter` records `owner_thread`
(tk.rs:~908) and `ensure_owner_thread` rejects cross-thread use — matching Tcl's
apartment model and CPython's "Tk must be used from the thread that created it". Under
the doc-33 two-GIL / free-threading frontier, the Tk interpreter stays single-owner;
a `Tk` object is NOT shareable across subinterpreters. The GIL is RELEASED around
`Tcl_EvalObjv` (the `GilReleaseGuard` in `run_tcl_command_with_ctx`), so a Tk call
does not block other Python threads during the (potentially long) Tcl evaluation —
this is already correct and must be preserved by doc-33 work. The notifier thread
(observed in the sample as `NotifierThreadProc`/`__select`) is Tcl-internal and
orthogonal.

### 5.5 Per-target story (HONEST)
- **native (Cranelift/LLVM)**: full Tk via libtcl FFI (this doc). The default native
  build links `stdlib_tk` (on by default; `MOLT_TK` unset ⇒ `molt_tk_native` feature
  appended, `cli.py`:~8490). **Build note: native tkinter apps require
  `MOLT_STDLIB_PROFILE=full` (or a profile that includes `stdlib_tk`) — the default
  `micro` profile excludes the Tk crate and the link fails with undefined
  `molt_tk_*` symbols. This profile-selection gap should be closed (Part 7 baton):
  an app importing `tkinter` should auto-escalate the stdlib profile.** Runtime
  also needs the `gui.window` (+ `process.spawn`) capability granted.
- **WASM**: **there is no Tk on WASM and there cannot be** — libtcl/libtk are native C
  libraries with no WASM port, and there is no DOM windowing from inside the molt WASM
  sandbox. The honest WASM GUI story is NOT tkinter: it is the doc-35 web-stack lane
  (render HTML/Canvas from molt-on-WASM and drive the browser DOM). The headless
  command emulation in tk.rs (the `not(feature="tk")` arm) exists for API-surface
  compatibility and capability-guard testing, NOT for real rendering. A future
  `tkinter`-shaped façade over the DOM (Canvas-backed widgets) is conceivable but is a
  separate, large lane; it would reuse `tkinter/__init__.py`'s widget classes over a
  Canvas command backend. **Stated as not-supported today.**
- **Luau**: no Tk (no libtcl); same headless-emulation surface as WASM.

---

## Part 6 — Benchmark lane + gates

- `bench/tk/bench_tk_bridge.py` — the microbench (expr / setget / result-type /
  StringVar / IntVar / widget / event). Same source runs under CPython
  (`python3 bench/tk/bench_tk_bridge.py`) and molt (`molt build` + `safe_run.py`).
  Withdraws the window, bounded, never calls `mainloop()`. Avoids two latent CORE
  bugs so it measures the BRIDGE fairly on both runtimes: it does not capture a bound
  method to a local (molt mis-computes the arity of a captured `*args` bound method),
  and reads `tkinter.wantobjects` (the module constant) rather than `tk.wantobjects()`
  (missing-attr delegation). Both runtimes run the identical direct-dispatch shape.
- `bench/tk/tk_wantobjects_parity.py` — the correctness gate; `cmp -s` against CPython
  must stay byte-identical.
- **Gates for any future Tk change**: (1) `tk_wantobjects_parity.py` stays
  `cmp -s` identical to CPython; (2) `cargo build --profile release-fast -p
  molt-backend --features native-backend` 0 warnings; (3) re-run the gap table and
  show before/after; (4) the `tkinter_phase0_core_semantics.py` differential stays
  green (it passed throughout this arc). **Crucial build note**: edits to
  `molt-runtime-tk/src/tk.rs` now participate in the runtime fingerprint (Part 1.4),
  but cargo's mtime-based fingerprint can consider an already-compiled rlib "current"
  if a prior build compiled it AFTER your edit's filesystem mtime — `touch -m
  runtime/molt-runtime-tk/src/tk.rs` before rebuilding to force recompilation, and
  delete the published `libmolt_runtime.stdlib_full.a` alias + its
  `runtime_fingerprints/*full*` entry to force re-link.

---

## Part 7 — Remaining phased plan + baton items

### Phase A (P0, core scope — blocks event GUIs)
**Fix `molt_is_callable` / the bind intrinsic ABI** so `widget.bind(seq, fn)` accepts a
callable (§5.2). Reproducer:
`_tkinter.bind_register(app, btn._w, "<Button-1>", fn, "")` raises despite
`callable(fn) == True`. Add a differential regression once fixed.

### Phase B (P1, backend/core scope — the path to BEAT CPython)
1. `molt_tk_call_v(handle, argc, argv_ptr)` — avoid the per-call `list(argv)`
   allocation (§5.1-a). 2. `with_gil_entry` fast path when the GIL is already held
   (§5.1-b). These two are expected to drop a generic `tk.call` below CPython's
   ~400 ns. Re-run the gap table; the contract requires molt to WIN every row.

### Phase C (P2, tkinter scope)
1. Auto-escalate the stdlib profile to include `stdlib_tk` when a build imports
   `tkinter` (close the `micro`-profile link-failure footgun, §5.5). 2. asyncio +
   Tk interop helper (§5.3). 3. surrogateescape-exact string decoding in
   `tcl_obj_string_to_bits` (currently lossy-UTF-8; CPython uses surrogateescape on
   Unix) — pin with a non-UTF-8 result differential.

### Baton items (cross-scope, do not silently absorb)
- **`src/molt/cli.py` `_runtime_source_paths`** — LANDED the Tk-crate fingerprint fix
  (Part 1.4) because it hard-blocks all Tk runtime work. Flagged as edited outside
  nominal Tk scope.
- **CORE: `molt_is_callable` across the intrinsic boundary** (Phase A) — P0.
- **CORE: captured-`*args`-bound-method arity** — `f = obj.call; f(a, b)` raises
  `call arity mismatch (expected 2, got 3)` when `obj.call` wraps a `*args` function;
  the bench works around it by not capturing. Latent; surfaces in real tkinter code
  that aliases `tk.call`.
- **CORE: missing-attribute on a `Tk`/widget instance** — `root.nonexistent` segfaults
  / raises `TypeError: exceptions must derive from BaseException` instead of a clean
  `AttributeError` (the `Tk.__getattr__` delegation to a non-string handle hit a core
  `getattr`/exception-lowering bug during development; mitigated by keeping `_tk_app`
  a `TkappType`, but the underlying core fault remains).
- **BACKEND: `molt_tk_call_v` + GIL fast path** (Phase B) — the perf-contract blocker.

### What this arc deliberately did NOT do (refusals)
- Did NOT ship the typed bridge as a dual path alongside the string path — the dead
  string-result path was deleted (`tcl_result_to_bits` removed), per the no-dual-paths
  rule.
- Did NOT work around the bind/event core bug inside tkinter (e.g. by skipping the
  callable check) — that would mask a real core fault; it is batoned as P0 instead.
- Did NOT declare the performance task "done" — molt does not yet beat CPython on the
  pure-call benches; the residual tax is root-caused with numbers and the structural
  arc to close it (Phase B) is specified.
- Did NOT lower the capability invariant — the `gui.window` check is hoisted to `Tk()`
  construction (where CPython establishes it), not removed.
