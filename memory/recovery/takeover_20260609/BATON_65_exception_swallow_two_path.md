# Baton #65: finalizer exception-swallow — TWO-PATH proof + next trace (2026-06-09)

## Confirmed live on origin a2fbab1a2/60a1373dd (native)
Exception raised in `__del__` PROPAGATES (molt exit 1) instead of being swallowed
(CPython: stderr "Exception ignored…", stdout continues, exit 0). Minimal repro:
```python
class A:
    def __del__(self): raise ValueError("boom in del")
def run():
    x = A(); del x
run(); print("after del, still alive")
# CPython: after del, still alive  (exit 0)
# molt:    <traceback> ValueError  (exit 1)
```
Fails identically at MODULE scope, in a function, with/without `gc.collect()`, and
with a bare `A()` (no binding). Task #65 ("composition-dependent") UNDERSTATES it:
it fails in the minimal standalone case.

## THE KEY PROOF: there are TWO finalizer-invocation paths (trace-confirmed)
Added a gated trace `[FIN65]` right after the `call_callable0(__del__)` in
`maybe_run_object_finalizer` (object/mod.rs ~1755), rebuilt the runtime, ran with
`MOLT_TRACE_FIN65=1` under safe_run:
* `finalizer_matrix.py` (has gc.collect + escaping finalizers): **11 [FIN65] lines**
  → `maybe_run_object_finalizer` IS reached, trace works, env passthrough works,
  exception-swallow works (matrix `raise_in_del` prints "survived").
* p65 (standalone `del x`): **0 [FIN65] lines** on a FRESH build with the trace
  confirmed in source → p65's `__del__` runs via a path that BYPASSES
  `maybe_run_object_finalizer` entirely (and therefore its swallow).

`maybe_run_object_finalizer` (object/mod.rs:1704) is the ONLY `b"__del__"` lookup
in the entire runtime + sub-crates (grep-verified); the frontend only RESOLVES
`defines_del`, never calls `__del__`. So the second path does not call `__del__`
through the byte-literal lookup — it must be PROCESS TEARDOWN finalizing the
surviving object (round-13 baton: minimal `del x` objects "reclaimed only at
process teardown"), where either (a) `maybe_run_object_finalizer` runs but its
stderr trace is invisible post-teardown, or (b) a teardown sweep calls `__del__`
via a different mechanism with NO exception isolation.

## Why composition MASKS it (matrix passes)
`maybe_run_object_finalizer`'s no-prior-exception branch is
`else if exception_pending(py) { clear_exception(py) }`. The matrix's prior 8
finalizer sections leave a non-None "last exception", so `raise_in_del` takes the
`Some(prior)` branch which calls `exception_set_last_bits_raw(prior)` —
neutralizing the `__del__` exception REGARDLESS of `exception_pending`. Standalone
(no prior) relies on the weaker `else if`, AND on this case the finalizer fires on
the second (teardown) path that never reaches that code at all.

## VERDICT (2026-06-09, ONE-CYCLE counters + provenance — STOP the trace loop): D = SWALLOW BOUNDARY
Counters for p65 `del x`-in-function (MOLT_DEBUG_OBJECT_RC=1 + MOLT_TRACE_FIN65=1,
FULL output — my earlier "type_id=100 never enters" was a `head -20` TRUNCATION
artifact; the type_id=100 lines are at the END):
* PROVENANCE: `strings bin | grep FIN65` = 3 → trace IS linked (NOT cache/linkage; A ruled out).
* `OBJECT DEC` (type_id=100 decref) = 2; `OBJECT DEC→0 FREE` (zero transition) = 1
  → the user object IS dec-ref'd and DOES hit refcount 0 inline (NOT missing-release B).
* `FIN65-ENTRY type_id=100` = 1 → `maybe_run_object_finalizer` IS entered for it
  (NOT a second path C).
* `[FIN65] after __del__` (the post-call swallow trace at object/mod.rs:1765) = **0**,
  yet `__del__` ran (raised) → execution LEAVES `maybe_run_object_finalizer` BETWEEN
  `call_callable0(del_bits)` (~1755) and the swallow code (1763-1778). molt prints a
  NORMAL unhandled-exception traceback (`File … in A.__del__ / ValueError`), NOT
  CPython's "Exception ignored while calling deallocator" unraisable.

**=> VERDICT D: the swallow boundary (object/mod.rs:1763-1778) is STRUCTURALLY
BYPASSED.** The `__del__` exception escapes `call_callable0` before the swallow
runs. The matrix `raise_in_del` "survives" ONLY because it fires during
`gc.collect()`, whose downstream call site catches/clears the escaped exception —
the finalizer's OWN swallow has never been doing the work. So #65 is NOT #58/B; the
dispatch + drop placement are correct here.

**MECHANISM (READ-confirmed, NOT panic): molt exceptions are VALUE-BASED** —
`call/function.rs`/`dispatch.rs` use `raise_exception` (returns a sentinel) +
`exception_pending` + `enforce_no_pending_on_success`; there is NO `catch_unwind`
anywhere near finalizer/dunder calls. So `call_callable0(del_bits)` RETURNS when
`__del__` raises → the after-call trace at 1765 SHOULD fire. Its absence therefore
means, on the observed `type_id=100` finalizer entry, EITHER:
  (b1) `del_bits == missing_bits` → the whole `if del_bits != missing_bits` block
       (call_callable0 + the 1765 trace) is SKIPPED, and `A.__del__` actually runs
       via a SEPARATE dispatch the trace did not cover (there is exactly ONE other
       `type_id=100` FIN65-ENTRY-eligible site? — re-audit: the entry trace fired
       once for type_id=100; instrument the `else { return false }` of the
       `del_bits != missing` check AND a trace at 1752's `molt_get_attr_name_default`
       result), OR
  (b2) there are TWO finalizations of the SAME object (boom prints twice; OBJECT
       DEC=2): one inert (FINALIZER_RAN path) and one that actually calls `__del__`
       through a path that is NOT lines 1754-1759.
=> The "swallow is bypassed" is real, but the FIX is NOT catch_unwind. The decisive
next probe (one rebuild): trace BEFORE `call_callable0` (1755), in the `del_bits ==
missing` else-branch, and at the `molt_get_attr_name_default` result (1752), for
p65. That pins whether `__del__` is even invoked from THIS function for the user
object, or from a second site.

**FIX DIRECTION (verdict D, value-based):** ensure the `__del__`-raise exception is
written-unraisable + CLEARED on EVERY channel (task slot AND global slot AND active
exception stack) at the finalizer site, regardless of the `exception_pending` gate
(the current `else if exception_pending { clear_exception }` is too weak — make the
no-prior branch ALWAYS clear). Emit CPython's "Exception ignored while calling
deallocator" header for parity. If probe shows a SECOND `__del__` call site for
type_id=100, route THAT through the same swallow. Add a SELF-CONTAINED regression
(matrix `raise_in_del` passes only via gc.collect composition — split it so the
standalone `del x` case is gated). Verify native + LLVM + WASM.

## (superseded — head-truncation artifact) ENTRY-TRACE "never enters" was WRONG
The entry trace DID show one type_id=100 entry; it was below the `head -20` cut.
Below preserved for the record only:

## ENTRY-TRACE RESULT (2026-06-09, RESOLVES "which path"): user instance NEVER enters the finalizer
Added a SECOND trace at the TOP of `maybe_run_object_finalizer` (before the
type_id early-return) printing `type_id`. p65 run → 21 entries, ALL builtins:
type_id 200 (STRING), 206 (TUPLE), 221 (FUNCTION), 243 (CODE). `TYPE_ID_OBJECT`
= **100** (type_ids.rs:2). NOT ONE entry is 100 → the user `A` instance NEVER
reaches `maybe_run_object_finalizer` at all, yet `A.__del__` runs and propagates.
Combined with: (a) `maybe_run_object_finalizer` is the ONLY `b"__del__"` lookup in
the tree; (b) `_emit_delete_name` emits a releasing DEC_REF for `del x` ONLY in
`molt_main` (frontend/__init__.py:13806-13851), regular functions get NO release;
(c) `runtime_teardown_for_process_exit` (lifecycle.rs:107) clears subsystem state
but does NOT sweep-finalize live objects. So the in-function `A` instance is
released+finalized by a path that is NEITHER the frontend del-emit NOR
`maybe_run_object_finalizer` NOR the teardown sweep.
=> PRIME SUSPECT: the NATIVE value-tracking RC release (dormant-native substrate;
round-13 baton "on dormant native the value-tracking substrate released it") lowers
a `defines_del` object's drop to a release that invokes `__del__` as a RESOLVED
METHOD CALL (not via the `b"__del__"` byte lookup, which is why the grep missed it)
with NO exception isolation. Next: dump p65's TIR/native asm (MOLT_DUMP_IR /
objdump the drop site) to find the emitted `__del__` call, OR grep the native
backend value-tracking-RC lowering for a method-resolved finalizer call. The fix:
that release MUST route through the finalizer-aware swallow (save/clear/restore
unraisable), identical to `maybe_run_object_finalizer`'s tail.

## (superseded) earlier NEXT TRACE plan — entry trace now done
Instrument the TOP of `maybe_run_object_finalizer` (before the early returns at
1706/1709/1720) AND the `del_bits == missing_bits` branch. Rebuild runtime
(MUST set `MOLT_PROJECT_ROOT=<worktree>` — see DX note), run p65 with the trace:
* If entry-trace fires for p65 → it IS `maybe_run_object_finalizer` at teardown;
  the bug is stderr-visibility OR the teardown caller doesn't run the swallow
  branch. Find the teardown sweep (NOT in the obvious grep terms — try the
  generated main epilogue / module-phase finalizer in
  native_backend/function_compiler.rs:25360 "executable finalizer … process-exit"
  and simple_backend.rs:2821/3064 "drop finalizer over its TIR module").
* If entry-trace does NOT fire → `__del__` runs via a wholly separate site; widen
  the hunt to the generated entrypoint / atexit.

## FIX DIRECTION (once the path is found)
Route the second (teardown) finalization through the SAME finalizer-aware
release authority (`maybe_run_object_finalizer`'s save/clear/restore unraisable
semantics), OR wrap the teardown `__del__` call in the identical exception
isolation. CPython semantics: any exception in a finalizer is written-unraisable
to stderr and CLEARED; surrounding exception state preserved. Add a SELF-CONTAINED
exception-swallow regression (the matrix's `raise_in_del` only passes via
composition — split it out so the standalone case is gated). Ties to #58/round-13-C
(the object survives to teardown BECAUSE `del x` in a regular function emits no
release + drop_insertion is finalizer-unaware).

## DX note (now hardened — tools/molt_dev.py difftest, commit 57ea962ec)
Testing a RUNTIME edit from a worktree REQUIRES `MOLT_PROJECT_ROOT=<worktree>`
alongside `PYTHONPATH=<worktree>/src`. PYTHONPATH redirects only the frontend; the
runtime/backend fingerprint+build stays on the canonical checkout otherwise, so a
runtime edit is silently NOT compiled in (cost this session ~2 stale-build cycles
before root-cause). `molt_dev.py difftest <prog> --root <wt>` now does this
correctly (and diffs vs CPython); for TRACE inspection (stderr) use a direct
build+safe_run since difftest captures only stdout for the diff.
The trace edit is UNCOMMITTED in /tmp/wt_fin/runtime/molt-runtime/src/object/mod.rs.
