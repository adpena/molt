# 48 — FinalizerRegion / deferred-drain: `__del__` as an effectful release event

Status: ACTIVE (2026-06-09). Closes #65 (finalizer exception-swallow) structurally
and gives #58 (finalizer ordering) a home. Council-chosen option **B** over the
catch_unwind bridge (A).

## 1. Why the post-call swallow is unreachable (the falsification)
`maybe_run_object_finalizer` calls `__del__` via `call_callable0(del_bits)` and then
runs a swallow/clear. PROVEN (object/mod.rs probe + a full fix-attempt that never
executed): when `__del__` raises from an **inline compiled drop** (`del x` in a
regular function → `dec_ref_ptr` → finalizer), `call_callable0` **does not return** —
the exception **unwinds to the nearest COMPILED landing pad** (`run()`'s), skipping
the Rust finalizer frame where the swallow lives. Counters: `BEFORE-CALL`=1,
`AFTER-CALL`=0, `SWALLOW`=0, exit=1. The matrix `raise_in_del` passes only because it
fires inside `gc.collect()` — a pure-runtime call boundary with no compiled pad above
— so the swallow runs. So the swallow site is dead-by-construction in the inline path;
no amount of channel-clearing after the call can fix it.

## 2. Why catch_unwind is rejected as the default
`std::panic::catch_unwind` is documented as NOT a general try/catch and not
guaranteed to catch all unwinds; the project contract rejects catch-unwind control
flow. It would also make finalizer semantics depend on Rust unwinding internals.
Allowed ONLY as a named, temporary, last-resort bridge under an explicit ruling, and
removed by this tranche. The structural fix is to stop running `__del__` inline.

## 3. FinalizerEvent (the scheduled record)
```
PendingFinalizer { object_bits, class_bits }   // minimal Phase-2 slice
```
Conceptually a FinalizerEvent carries: object, class, prior_exception_state,
`resurrection_allowed=true`, `finalizer_ran` once-bit, `exception_policy=Unraisable`.
The deep abstraction: `DecRef(v) -> ReleaseOutcome` =
`{ Noop, Decremented, Destroyed, FinalizerScheduled, Resurrected }` — release is a
Python semantic event, not raw `free`.

## 4. Where events are SCHEDULED
In `dec_ref_ptr`, at the rc 1→0 transition, for a **finalizer-sensitive**
`TYPE_ID_OBJECT` (class defines `__del__`, mirrors `finalizer_alloc_roots`/the
`HEADER_FLAG`): instead of calling `maybe_run_object_finalizer` inline, **set the
`HEADER_FLAG_FINALIZER_RAN` once-bit, ROOT/keep the object alive (do not free the
payload, do not clear weakrefs yet), push `PendingFinalizer`, and RETURN**. Ordinary
(non-finalizer) objects destroy immediately on the existing fast path — unchanged.

## 5. Where events are DRAINED (the safe boundary)
At the nearest **pure-runtime call boundary** where a callee's raise is value-based
(the property `gc.collect()`/atexit already have), NOT inside `dec_ref_ptr`'s Rust
frame. One authority `run_pending_finalizer_unraisable(py, event)` drains each:
calls `__del__`, and because it runs at a value-based boundary the raise is captured;
it writes-unraisable (sys.unraisablehook → else "Exception ignored while calling
deallocator <method>:" + traceback) and clears/restores exception state; handles
resurrection-once (refcount > 0 after → keep alive, `finalizer_ran` stays set); else
completes destruction (free payload, clear weakrefs).
Phase-1 drain trigger (smallest that fixes p65 with correct order): an extern-`C`
`molt_drain_pending_finalizers()` invoked by the compiled code at a safe point AFTER
the release boundary — preference order: (a) right after a finalizer-object DecRef in
the drop lowering, else (b) the function-return trampoline. Also drained by
`gc.collect()` and at process teardown so nothing is lost.

## 6. Promptness / ordering (CPython parity, NOT defer-to-exit)
Python finalizes at rc 0 promptly; we must NOT defer arbitrarily to process exit.
`del x; print("after")` must finalize before `print`. So the drain point is the
nearest safe boundary after the `del`/DecRef, not "later". If post-DecRef drain is
too invasive for Phase 1, the function-return trampoline preserves order for p65
(`x` dies as `run()` returns, before the caller's `print`). The rule: "not inside
`dec_ref_ptr` with compiled unwinding able to skip the swallow," NOT "late."

## 7. Resurrection
`run_pending_finalizer_unraisable` revives the object across the `__del__` call (inc
then dec, as today); if rc > 1 after, the object resurrected → keep alive, do not
free; `finalizer_ran` stays set so `__del__` never re-runs (CPython once semantics).

## 8. Exceptions → unraisable
One authority writes-unraisable + clears. No `__del__` exception ever propagates as a
normal exception. Pre-existing (surrounding) exception is preserved/restored.

## 9. Tests that prove the slice (Phase 5)
p65 `del x`-in-function raises → "after" printed, exit 0, unraisable not normal
traceback; gc.collect matrix still green; ordinary `__del__` fires once; resurrection-
once; pending-exception-before-finalizer preserved/restored; `del x` timing vs a
following statement; non-finalizer dec_ref/free unchanged (fast path); native +
LLVM/WASM (shared runtime path).

## Phase 6 — one authority, delete the rest
After green: remove the inline `__del__` call from the `dec_ref_ptr` zero path for
finalizer-sensitive objects; route gc.collect/atexit finalization through the same
`run_pending_finalizer_unraisable`; delete/redirect any alternate swallow. Exactly
ONE finalizer-unraisable authority — the TableGen/single-source principle in runtime
form.
