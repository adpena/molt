# 48 — `__del__` exception swallow: the uncaught-exception terminator, not a native unwind

Status: **LANDED (2026-06-09).** Closes #65 (finalizer exception-swallow). The fix
is runtime-only: run `__del__` INLINE at the rc→0 point (CPython-prompt) under a
SYNTHETIC exception-handler frame. NO deferral, NO `catch_unwind`, NO backend
landing pad. The "FinalizerRegion / deferred-drain" design that this file
originally proposed was built on a **misdiagnosis** and has been reverted; this
document is rewritten to record the true root cause, the falsification of the
deferral premise, and the landed fix. The original deferral text is preserved in
git history (commit 48418a3bf and the WIP commits on the `wt_fin` branch).

## 1. The true root cause (definitively measured)
A raise inside `__del__` is **NOT** "molt's custom native unwind to the nearest
compiled landing pad" (the original premise, now FALSIFIED). molt's exception
model is fully **value-based**: `molt_raise` calls `record_exception` and then
RETURNS `none()` bits; compiled code polls `exception_pending()` after each call
and branches to its handler or propagates by returning. There is exactly ONE
non-value-based exit in `molt_raise` (exceptions.rs): the **uncaught-exception
terminator** —

```
record_exception(_py, exc_ptr);
if exception_handler_active() { exception_context_set(...); }
if !exception_handler_active() && !generator_raise_active() && !task_raise_active() {
    // format traceback, eprintln, then:
    std::process::exit(1);
}
```

`exception_handler_active()` is `!EXCEPTION_STACK.is_empty()` — purely whether a
handler frame is on the stack. When a finalizer runs at the rc→0 point of a plain
`del x`, the handler stack is EMPTY (no surrounding `try:`), so a raise inside
`__del__` hits `std::process::exit(1)` and **kills the process** before the Rust
swallow code (which lives AFTER the `call_callable0` return) can run.

This explains every prior observation:
* `catch_unwind` caught **nothing** — a `process::exit` is not an unwind.
* Setting `exception_stack_baseline` did **nothing** — the baseline does not gate
  the terminator; an empty `EXCEPTION_STACK` does.
* The behavior was **composition-dependent** — the matrix's `raise_in_del`
  survived only because a surrounding `try:` (or a prior frame) left a handler on
  the stack, so `molt_raise` took the value-based path and the swallow ran. A
  lone/first standalone finalizer found an empty stack and exited.

## 2. The landed fix
In `maybe_run_object_finalizer` (object/mod.rs), wrap the `__del__` invocation in a
SYNTHETIC handler frame:

```rust
crate::builtins::exceptions::exception_stack_push();   // EXCEPTION_STACK now non-empty
let result_bits = call_callable0(py, del_bits);        // raise -> value-based, returns
crate::builtins::exceptions::exception_stack_pop(py);  // pop synthetic frame
```

Now `molt_raise` sees `exception_handler_active() == true`, records the exception
value-based, and returns; `call_callable0` returns with the exception pending; the
swallow below runs in EVERY context (standalone, composed, gc.collect). This is
exactly CPython's implicit "ignore exceptions during finalization" boundary, in
runtime form, mirroring the compiled try-frame (`molt_exception_push` /
`molt_exception_pop`). `__del__` still runs INLINE at the rc→0 point, so
finalization stays CPython-prompt (`del x; print()` finalizes before `print`).

The swallow itself (unchanged in shape) writes-unraisable to stderr ("Exception
ignored while calling deallocator:" + traceback) and clears all exception channels,
or — if a surrounding exception was active — preserves/restores it (CPython
semantics).

## 3. Why the deferral design was wrong (and reverted)
The original design deferred `__del__` to a value-based drain boundary
(`PENDING_FINALIZERS` queue, schedule-at-`dec_ref` / drain-at-gc/teardown), on the
theory that the inline raise was an uncatchable native unwind. Since the real
cause is `process::exit` on an empty handler stack — which fires identically
whether `__del__` runs inline OR at a drain — deferral could **never** have fixed
the swallow. Worse, it introduced a correctness regression: deferral moves `__del__`
off the prompt rc→0 point, so `del x; print()` finalizes AFTER `print`
(`fires_once`/`resurrect` ordering regressed), which would have required a backend
"selective compiled safe-point drain" (the abandoned Phase 2E) just to recover
timing that inline finalization gives for free. The synthetic-handler fix is
strictly smaller, needs no backend change, and preserves prompt timing. All the
deferral machinery (`schedule_object_finalizer`, `drain_pending_finalizers`,
`PENDING_FINALIZERS`, `HEADER_FLAG_FINALIZER_SCHEDULED`, the gc.collect/teardown
drains, the `molt_drain_pending_finalizers` extern) is deleted.

## 4. Two independent layers (do not conflate)
Finalizer behavior splits into two layers; #65 is entirely in layer (2):
* **(1) DecRef PLACEMENT** — does the compiler emit the releasing DecRef? Frontend
  `del` lowering + `drop_insertion.rs` §1/§1b on the drop lanes (LLVM/WASM/
  flipped-native); the value-tracking substrate on dormant-native. Gaps here
  (e.g. #63 loop-body `for i: x=R(i); del x` on dormant-native; a child instance
  attribute not dropped on dormant-native) mean `__del__` never runs because the
  object never reaches rc 0. These are the round-13 / #63 arc, UPSTREAM of #65.
* **(2) DecRef→0 EXECUTION** — when a DecRef DOES reach rc 0, does `__del__` run,
  swallow its exception, handle resurrection-once? This is `maybe_run_object_
  finalizer`, the single finalizer authority. #65 fixes the exception-swallow here.

## 5. Verification (native, byte-identical to CPython 3.14)
GREEN (stdout byte-identical, exit 0, unraisable to stderr): `p65_gc`, `p65_func`
(the standalone-finalizer-raise repros — previously exit 1), `fires_once` (prompt
timing: DEL before "after del"), `resurrect` (resurrection-once), `finalizer_
matrix` (all 9 sections incl. `raise_in_del survived`), `stress_raise` (20000
swallowed raises — synthetic push/pop balance holds at scale), `stress_reraise_
active` (finalizer raises while a surrounding exception is handled — prior-exc
preserved), `sr2_resurrect` (20000 true resurrections via the function-return drop
path). The runtime is backend-agnostic, so the same fix applies to native / LLVM /
WASM / Luau; LLVM is spot-checked because its drop lanes place the DecRefs that
dormant-native's value-tracking does not (isolating layer 1 from layer 2).

KNOWN (layer-1, pre-existing, NOT #65 — each verified against the clean main
baseline so they are NOT regressions of this change):
* loop-body `for i: x=R(i); del x` does not fire `__del__` on dormant-native (the
  per-iteration DecRef is not placed) — #63.
* an object-valued instance attribute (`self.child = Leaf()` where `Leaf` defines
  `__del__`) does not fire the child's `__del__` when the parent is freed — on
  BOTH native AND LLVM (so NOT a dormant-native-only placement gap). Measured:
  parent `__del__` fires for all N parents, child `__del__` fires 0 times; the
  same result on the clean main runtime (no #65 change), proving pre-existing.
  Filed as its own task (object-valued-attribute finalizer cascade). The parent's
  free dec-refs `instance_dict_bits`, so the child should reach rc 0 and finalize;
  it does not — root cause is in the attribute-drop / dict-free cascade, UPSTREAM
  of `maybe_run_object_finalizer`.

## 6. One authority
`maybe_run_object_finalizer` is the SINGLE finalizer-unraisable authority — every
`DecRef` lowering on every backend routes rc→0 through `dec_ref_ptr` →
`maybe_run_object_finalizer`. There is no alternate swallow and no second
finalizer path. Finalizer ORDERING (#58) is a separate, layer-1 (drop-placement)
concern and gets its home on the ownership lattice per the council doctrine, NOT
here.
