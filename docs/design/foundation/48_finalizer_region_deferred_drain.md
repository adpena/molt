# 48 — `__del__` exception swallow: the uncaught-exception terminator, not a native unwind

Status: **PARTIAL RUNTIME PRIMITIVE LANDED (2026-06-09; evidence refreshed
2026-06-12).** The runtime-only synthetic-handler authority for `__del__` exists:
run `__del__` INLINE at the rc→0 point (CPython-prompt) under a SYNTHETIC
exception-handler frame. NO deferral, NO `catch_unwind`, NO backend landing pad.
The standalone raising-finalizer lane, native path-sensitive scope-exit ordering
gate (`tests/differential/basic/finalizer_scope_exit_ordering.py`), plain
non-finalizer object guard, object-attribute release smoke, and exit-semantics
lane are green as of 2026-06-12 after preserving the frontend's `defines_del`
result fact through native's TIR -> SimpleIR optimization round-trip and adding
class/instance finalizer-sensitivity bits. The explicit local `del` /
`gc.collect()` resurrection-once gate is also green as of 2026-06-12 after
promoting `DeleteVar` to carry the old slot occupant as an explicit TIR operand
and release it after storing the missing sentinel. This does **not** close every
finalizer composition yet: container-owned release boundaries and the broader
resurrection/leak matrix remain fail-closed xfails with raw mismatches
preserved. The "FinalizerRegion /
deferred-drain" design that this file originally proposed was built on a
**misdiagnosis** and has been reverted; this document records the true root
cause, the falsification of the deferral premise, the landed runtime primitive,
and the remaining executable gates. The original deferral text is preserved in
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
value-based, and returns; `call_callable0` returns with the exception pending.
The swallow below is the single runtime authority for finalizer unraisable
handling. It is proven for the standalone raising-finalizer lane, but broader
resurrection/container composition remains gated by the differential tests named
in the status block above. This is Molt's runtime form of CPython's implicit "ignore exceptions
during finalization" boundary, mirroring the compiled try-frame
(`molt_exception_push` / `molt_exception_pop`). `__del__` still runs INLINE at the
rc→0 point, so finalization stays CPython-prompt where drop placement reaches
that boundary (`del x; print()` finalizes before `print`).

The swallow itself (unchanged in shape) writes-unraisable to stderr ("Exception
ignored while calling deallocator:" + traceback) and clears all exception channels,
or — if a surrounding exception was active — preserves/restores it (CPython
semantics).

2026-06-12 update: finalizer *sensitivity* is now class metadata, not an
ordinary dying-instance attribute probe. `HEADER_FLAG_CLASS_HAS_FINALIZER` is
refreshed when a class MRO or class-level `__del__` binding changes, and
`object_set_class_bits` copies that to `HEADER_FLAG_INSTANCE_HAS_FINALIZER` on
fresh `TYPE_ID_OBJECT` instances. The rc→0 hot path first checks the instance
flag; non-finalizer objects never enter `__del__` lookup, so plain objects and
objects with only an instance attribute named `__del__` cannot emit false
unraisable AttributeErrors. For finalizer-sensitive instances,
`maybe_run_object_finalizer` resolves raw `__del__` through the class MRO and
uses the shared descriptor binder before `call_callable0`, preserving the single
synthetic-handler swallow authority while matching CPython's type-special
finalizer lookup.

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

## 5. Verification and remaining gates
GREEN for the currently proven runtime primitive and placement gates (stdout
byte-identical where applicable, exit 0, unraisable to stderr):
`finalizer_standalone_raise_swallow.py`, `finalizer_scope_exit_ordering.py`,
`finalizer_plain_object_no_false_positive.py`,
`finalizer_object_attr_release.py`, `finalizer_exit_semantics.py`, and
`finalizer_resurrection_once.py`.

FAIL-CLOSED xfail with raw mismatch preserved:
`finalizer_container_clear.py`, `finalizer_matrix.py`,
`resurrect_with_exception_in_del.py`, and
`finalizer_resurrection_leak_gauge.py`. These cover container-owned release
boundaries and broader resurrection/leak composition where Molt still observes
empty container event lists where CPython runs nested `__del__` paths.

CLOSED for the native scope-exit ordering gate:
`finalizer_scope_exit_ordering.py` is no longer `expect_fail=molt`. The closing
evidence is the raw one-file differential with daemon off, rebuild/no-cache
forced, and RSS measurement enabled:
`tmp/diff/finalizer_scope_exit_ordering_after_custody.json` reports `passed=1`,
`failed=0`, build RSS 1457776 KB, run RSS 10272 KB.

The focused resurrection-once differential
`tests/differential/basic/finalizer_resurrection_once.py` is now a must-pass
gate. The 2026-06-12 closing run used the daemon-off, rebuild/no-cache, guarded
single-file differential and reported `[PASS]` with build RSS 1444208 KB and run
RSS 16352 KB. This proves the explicit local `del` boundary stores the missing
sentinel before the old binding's finalizer can observe the frame. The remaining
finalizer verification split is six PASS lanes and four fail-closed XFAIL lanes.

The runtime authority is backend-neutral because every backend routes rc→0
through `dec_ref_ptr` → `maybe_run_object_finalizer`, but backend support is only
claimable where drop placement and the relevant differential/backend evidence are
green.

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
