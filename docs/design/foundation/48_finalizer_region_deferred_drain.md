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

## CRITICAL implementation invariant (RC accounting — get this wrong → RC corruption)
`maybe_run_object_finalizer` ASSUMES it is called with the object at **refcount 0**
(it `inc_ref`s self→1 at entry, runs `__del__`, `fetch_sub`→0 at exit, and treats
`prev > 1` as RESURRECTION). Therefore the SCHEDULE step must **NOT** `inc_ref`/root
the object — leave it at refcount 0, just (a) set `HEADER_FLAG_FINALIZER_SCHEDULED`
(new, bit `1<<18`; FINALIZER_RAN is `1<<16`, INTERNED `1<<17`, CONTAINS_REFS `1<<19`),
(b) push the raw `ptr` into the queue, (c) **do not free** (skip the dealloc tail),
(d) return. The object sits at refcount 0, unfreed, SCHEDULED — the moral equivalent
of CPython's pending-finalizer/trashcan state; nothing references it so no further
dec underflows. DRAIN reuses `maybe_run_object_finalizer` UNCHANGED (refcount-0
context preserved → its resurrection math stays correct), then for the non-resurrected
case performs the SAME free tail that `dec_ref_ptr` runs today (1962→end:
DEALLOC_COUNT/bytes commit, weakref clear, payload + cold-header free). => EXTRACT
that free tail of `dec_ref_ptr` into `fn finalize_free_object(py, ptr, type_id,
size_class, cold_idx, dealloc_bytes)` and call it from BOTH `dec_ref_ptr` (today's
inline non-finalizer path) AND the drain. Do NOT duplicate it. The `dec_ref_ptr`
hook at the rc1→0 site becomes: `if schedule_object_finalizer(py, ptr) { return; }`
(schedule sets SCHEDULED + queues + returns true for a runnable-`__del__` object that
is neither RAN nor already SCHEDULED) `else { finalize_free_object(...) }`.
Leak gauge stays exact: dealloc is committed only inside `finalize_free_object`
(at true free), never at schedule.

## PHASE-2 IMPLEMENTATION RESULT (2026-06-09): deferral works, but the SWALLOW needs a COMPILED landing pad
Implemented (WIP, local commits on wt_fin; patch preserved in
`memory/recovery/takeover_20260609/finalizer_region_wip.patch`): `dec_ref_ptr`
schedules finalizer-sensitive objects (rc 0, SCHEDULED, unfreed) via
`schedule_object_finalizer`; `gc.collect` + teardown drain via
`drain_pending_finalizers` → `maybe_run_object_finalizer` → re-entry free; detection
via class-MRO lookup (never touch the dead instance — fixed a SIGSEGV); the no-prior
swallow rewritten to write-unraisable + clear all channels; a local
`exception_stack_baseline` around the `__del__` call.

**What WORKS:** `finalizer_matrix.py` BYTE-IDENTICAL to CPython (all 9 sections incl.
`raise_in_del survived`); the rewritten swallow is CORRECT when reached
(`pending_before=true → unraisable → pending_after=false`); `p65_func` now CONTINUES
past the `del` (deferral proven — body completes, "after del, still alive" prints).

**What does NOT work (and WHY — definitively measured):** `p65_gc`/`p65_func` still
exit 1; the swallow fires ZERO times for a STANDALONE finalizer. Proven by trace +
catch_unwind experiment:
* The object IS scheduled (`SCHED YES type_id=100`=1) and the drain DOES call
  `maybe_run_object_finalizer`, but `call_callable0(__del__)` does NOT return — it
  UNWINDS past the swallow.
* The unwind is molt's CUSTOM native unwind to the nearest COMPILED landing pad:
  `std::panic::catch_unwind` around the drain caught NOTHING (0 events); a local
  `exception_stack_baseline_set(depth)` did NOT catch it either. So it is NEITHER a
  Rust panic NOR baseline-controlled — it bypasses ALL Rust frames (drain, catch).
* It is COMPOSITION-dependent: the matrix's `raise_in_del` returns value-based ONLY
  because ~5 non-raising finalizers drained first; a lone/first raising finalizer
  unwinds.

**=> Both A (catch_unwind) and B (runtime drain) are INSUFFICIENT alone.** The swallow
needs a COMPILED landing pad above the `__del__` execution; a Rust drain frame has none.

**STRONGEST LEAD for the real fix:** `atexit` swallows callback exceptions VALUE-BASED
using the SAME `molt_call_bind` (`atexit.rs:344`, then checks `exception_pending` at
:503 and handles a RETURNED-raised-exception sentinel via
`callback_returned_raised_exception` at :508/:268). The finalizer's
`call_callable0 → call_type_via_bind → molt_call_bind` on a BOUND METHOD unwinds where
atexit's `molt_call_bind` on a FUNCTION returns. NEXT INVESTIGATION: (1) why the
bound-method dispatch unwinds where the function dispatch returns value-based; (2)
route `__del__` through atexit's value-based call path (or its returned-exception
sentinel convention) — likely the actual fix, runtime-only, no catch_unwind, no
backend landing pad. If that fails, the fix is a backend-emitted compiled try-frame
around the drain (`molt_exception_stack_enter` + a real compiled landing pad).

**Timing (separate, Phase 2E):** even once the swallow works, deferral moves `__del__`
to gc/teardown → `fires_once`/`resurrect` ordering regresses (DEL after "end"). Needs
the selective compiled safe-point drain after finalizer-object DecRefs (NOT every
DecRef — perf). The WIP is therefore NOT shippable as-is.

## Phase 6 — one authority, delete the rest
After green: remove the inline `__del__` call from the `dec_ref_ptr` zero path for
finalizer-sensitive objects; route gc.collect/atexit finalization through the same
`run_pending_finalizer_unraisable`; delete/redirect any alternate swallow. Exactly
ONE finalizer-unraisable authority — the TableGen/single-source principle in runtime
form.
