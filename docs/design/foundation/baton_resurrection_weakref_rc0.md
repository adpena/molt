# Baton: weakref-callback resurrection at rc=0 (use-after-free)

**Status:** root-caused by static analysis; fix designed, NOT implemented. No current
SIGSEGV repro in the suite (latent) — a repro must be added with the fix.
**Severity:** P0 — memory corruption (council #1 class: resurrection/finalizer/weakref).
A weakref callback that resurrects an object being collected produces a dangling
reference → UAF/SIGSEGV. Invalidates trust in the memory model.

## Root cause (file:line)

`runtime/molt-runtime/src/object/mod.rs`:
- `:2163-2177` — `dec_ref_ptr` zero-transition: `maybe_run_object_finalizer(py, ptr)`
  runs `__del__` under a TEMPORARY `inc_ref` (`:1874` revive 0→1, run `__del__`
  `:1898-1929`, `:1963` `fetch_sub(1)` back to 0, return whether resurrected). Returns
  `false` (not resurrected) → fall through to dealloc.
- `:2187` — `weakref_clear_for_ptr(py, ptr)` then executes the weakref callbacks
  (`weakref.rs:36-56`, `call_callable1(_py, cb_bits, weak_bits)`) **while the object is at
  rc=0** — the finalizer's temporary inc_ref was already dropped at `:1963`.

The defect is the **refcount STATE during weakref callback execution**, not the order
(weakref-after-`__del__` matches CPython `PyObject_CallFinalizer`). With the object at
rc=0:
1. a weakref callback resurrects the object (re-increments rc 0→1);
2. callback returns; `weakref.rs:56` `dec_ref_bits(weak_bits)` drops it 1→0;
3. `dec_ref_ptr(0)` runs AGAIN — but `FINALIZER_RAN` is set, so `__del__` is skipped;
4. the object is freed;
5. the reference the callback stashed is now dangling → UAF on next use.

## CPython contract violated

CPython runs weakref callbacks (`PyObject_ClearWeakRefs`) after `tp_finalize` and before
`tp_dealloc`, but the object's storage is live and the resurrection check covers the
whole finalize+weakref window. molt drops the revival inc_ref before weakref clearing,
so the resurrection window does not cover the weakref callbacks.

## Fix (structural — preferred Option C; minimal Option A)

- **Option A (minimal, correct):** at `:2187`, `inc_ref` 0→1 BEFORE
  `weakref_clear_for_ptr`, run the callbacks at rc=1, then `dec_ref` 1→0 and perform a
  SECOND resurrection check; only dealloc if still not resurrected. The single revival
  window now covers `__del__` AND the weakref callbacks (matching CPython).
- **Option C (council-aligned, ownership lattice):** mark weakref-callback operands
  `FinalizerSensitive` so drop-insertion tracks the object's lifetime THROUGH the
  callback boundary and defers release to the frame-teardown boundary, on
  `ownership_lattice_min.rs` — not as another `dec_ref_ptr` special-case. This makes
  "weakref callback runs within the object lifetime" a lattice invariant, so the rc=0
  window is unexpressible. Prefer C; A is the stop-gap if C is too large this session.

Either way: ONE revival window covering finalize + weakref-clear + a post-window
resurrection check.

## Verification (mandatory — a wrong fix is leak OR UAF)

- ADD a repro: an object with `__del__`-free finalization whose weakref callback stashes
  the weakref's referent into a global (resurrection in the callback); assert correct
  value + no UAF, native AND LLVM/WASM/Luau.
- Full `tests/differential/memory/` green: `finalizer_resurrection*`, the weakref/leak
  gauges, `cycle_leak_*`, `custom_object_loop_phi_retain`. Bounded RSS (no leak from the
  extra revival window on the non-resurrect path).
- `MOLT_ASSERT_NO_LEAK` = actual destruction holds.

Companion: [[loop-iv-modulo-carrier-bug]] (the OTHER active memory P0, an int-repr bug),
the GC cycle collector design (w7bs8ouu1 — composes with this finalizer ordering).
