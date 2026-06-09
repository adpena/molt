# Round-13 baton: finalizer dispatch — root-caused, ONE class fixed, the broader arc mapped

## TL;DR of round-12's verdict was WRONG about the layer; round-13 found the real root

The round-12 baton's hypothesis ("TIR `DecRef` lowering frees through a path
WITHOUT the finalizer hook; the fix is to route the drop DecRef through the
finalizer-aware release the tracked path uses") is **FALSIFIED by direct
evidence**. The release authority is ALREADY unified:

* `molt_dec_ref` (ptr) and `molt_dec_ref_obj` (bits) BOTH route through
  `dec_ref_ptr` (`runtime/molt-runtime/src/object/mod.rs:1771`), which at the
  rc 1→0 transition calls `maybe_run_object_finalizer` (`:1685`) — the SINGLE
  finalizer-aware authority (`__del__` dispatch + `HEADER_FLAG_FINALIZER_RAN`
  once-bit + resurrection abort + exception-swallow + unraisable parity).
* EVERY lane's `DecRef` lowering already calls the finalizer-aware symbol:
  - LLVM  `llvm_backend/lowering.rs:1453` → `molt_dec_ref_obj`
  - WASM  `tir/lower_to_wasm.rs:890` and generic `wasm.rs:12554` → `dec_ref_obj`
  - Native `function_compiler.rs:16421` (`dec_ref`/`release` arm) → `local_dec_ref_obj`
    (= `molt_dec_ref_obj`); `emit_dec_ref_obj` only INLINES the tag-check then
    calls the same symbol on the heap branch.

So the symbol axis was never the bug. **Instrumenting `maybe_run_object_finalizer`
(env `MOLT_DEBUG_FINALIZER`, since removed) proved the `Demo` instance
(`TYPE_ID_OBJECT`=100) NEVER reaches `dec_ref_ptr`'s zero path at all** — on LLVM
and WASM. The object's refcount never reaches 0; it's reclaimed only at process
teardown (clean RSS, silent skipped finalizer). The bug is UPSTREAM of the
lowering: the owning `DecRef` is never placed (or the object is never even
constructed).

## What round-13 LANDED (the complete, regression-free piece)

`runtime/molt-backend/src/tir/passes/drop_insertion.rs` — new section **§1b
"Dead-on-arrival owned results"** (42 lines, right after the §1 straight-line
last-use loop, ~line 725).

ROOT CAUSE of the dead-on-arrival class: the drop pass's §1 last-use scan is
keyed by **operand** uses (`last_use` is built iterating `op.operands`). A
freshly-minted OWNED value with **zero** subsequent operand-uses never appears in
`last_use`, so it was NEVER dropped. The 2 committed finalizer differentials
collapse to exactly this shape: `item = Demo(1); del item` in a regular function
optimizes down to a bare `call_bind -> [v]` whose result is unused (the
frontend's `del` in a non-`molt_main` function emits NO releasing DecRef — see
the remaining-arc section — so the store + del are dead-store-eliminated, leaving
the bare construction). On dormant native the value-tracking substrate released
it; on the drop lanes (LLVM/WASM/activated-native) the drop pass is the SOLE RC
authority, so it leaked AND skipped the finalizer.

FIX: after §1, scan op RESULTS; for each `result` that is `droppable` (fresh-owned
root, heap, not param/stack/raw/non-owning-copy), not already in `last_use`, not a
conditional-iterator result (`iter_cond_value_results`), not transferred by a
branch-arg/terminator, and not live-out — plan a `DecRef(result)` immediately
after its defining op (its definition IS its last point of liveness). Same
soundness rails as §1; FAIL-CLOSED (leak, never UAF) preserved. Adversarial
review covered: multi-result ops, alias-class copies, later-block uses, double-add
to `after_op` (disjoint from §1 because §1b requires `r ∉ last_use`). 18/18
`drop_insertion` unit tests pass.

### Verification (all GREEN)
* 3 finalizer differentials byte-identical to CPython 3.14 on **LLVM** and
  **flipped-native** (`finalizer_exit_semantics`, `finalizer_resurrection_once`,
  `object_finalizer_dict_class_lifetime` — incl. resurrection-once + dict-lifetime
  + resurrection). Native-dormant still passes (tracked-RC, unchanged).
* `bench_counter_words` == **97360** on dormant-native AND flipped-native.
* `loop_while_true_break_drops` byte-identical on flipped-native (round-10 class
  stays dead).
* 78-test object/class/attr/dunder slice on flipped-native: the only 9 failures
  ALSO fail on dormant-native (pre-existing task-#78 parity gaps, NOT my change —
  single-variable confirmed by rebuilding dormant and re-running the identical 9).
* Gates: native lib 1025/0, native+llvm lib 1094/0, runtime lib 510/0(+16 ign),
  clippy native + native+llvm `-D warnings` clean ×2, `check_suite_honesty.py`
  OK (native=146, llvm=0, wasm=4, luau=0; 150 tracked within baseline).
* Perf: §1b adds a runtime DecRef ONLY for a fresh-owned heap value with zero
  operand-uses — none exist in numeric hot loops (fib/sieve/struct), so the hot
  path emits zero new calls; the nonzero inline fast-path is untouched. fib/
  struct/class_hierarchy on LLVM unchanged (startup-dominated, compute fast).

Commit: drop_insertion.rs ONLY (pathspec). Flip NOT committed (reverted to
`NativeCranelift => false`, pass_manager.rs:118). wasm/*.sha256 left unstaged
(pre-existing partner/build-artifact churn).

## THE REMAINING ARC (NOT landed — genuinely separate, multi-subsystem)

Three more finalizer scenarios still diverge. They are NOT regressions of the
landed fix; they are pre-existing and span DCE + frontend + the WASM emitter.
Each is reproducible TODAY:

### (A) DCE eliminates `__del__`-bearing constructions whose result is "used then dead"
Repro (`/tmp` during round-13; recreate): a regular function with
`item = Demo(1); x = item.value; del item; gc.collect()` (or even WITHOUT `del`,
just scope-exit) — CPython prints `[1]`, molt prints `[]` on **LLVM AND native**.
The PRE-DROP TIR dump (`MOLT_DEBUG_PREDROP`, a removed diag — re-add the
file-artifact dump in `drop_phase.rs::finalize_function_drops` before
`run_drop_phase`) shows the `Demo(1)` `call_bind` is **entirely gone** — an
upstream pass (dead_store_elim / dce / copy_prop chain) eliminated the
construction because (a) its result feeds only dead code and (b) nothing marks an
object construction whose class defines `__del__` as carrying an observable
end-of-life side effect. CPython NEVER elides object creation when `__del__`
exists. FIX DIRECTION: a construction (`ObjectNewBound` / `call_bind` of a class
whose MRO defines `__del__`) is NOT dead even with an unused result — teach the
purity/DCE oracle (and the frontend `del`, see C) that such a value is
side-effecting at drop. The cleanest carrier is a `has_finalizer` fact computed
in the frontend (`_resolve_method_info(class_id, "__del__")` walks the MRO —
fail-CLOSED to `True` when the MRO is not statically resolvable) and threaded
ObjectNewBound→serialization→OpIR→SSA-attr→passes, mirroring how
`class_size_bytes`/`_type_hint` already flow (calls.py:6015/6075 emit
`OBJECT_NEW_BOUND`; serialization.py:1314; ir.rs `OpIR`; ssa.rs:1144 attr copy).
NOTE the documented-but-UNENFORCED invariant at `tir/types.rs:34` ("instances of
a class with no `__del__` and no weakref support can be stack-allocated") — the
SAME `has_finalizer` fact must also gate `escape_analysis::apply` stack-promotion
+ RC-elision (escape_analysis.rs:824-851 rewrites ObjectNewBound→…Stack and
retains-out IncRef/DecRef with NO finalizer guard; `dict_requiring_alloc_roots`
at :720 is the exact pattern to mirror for a `finalizer_requiring_alloc_roots`).

### (B) WASM never fires `__del__` even when the DecRef IS placed
With the landed fix, the WASM drop dump shows `DecRef([v])` IS inserted for the
Demo instance, and it lowers to `dec_ref_obj`. But `MOLT_DEBUG_FINALIZER`
(DECOBJ/INCOBJ traces, removed) proved the object is **over-inc'd by exactly 1**
vs LLVM: LLVM does `INCOBJ→2, DECOBJ 2→1, DECOBJ 1→0` (finalizer fires); WASM
does `INCOBJ→2, INCOBJ→3, DECOBJ 3→2, DECOBJ 2→1` (stuck at 1). ROOT: the WASM
emitter (`wasm.rs`) has **ZERO `drop_inserted`-marker gating** — unlike native's
~18 `!drop_inserted` sites (`function_compiler.rs`) that suppress the competing
automatic temp-RC for drop-processed functions. WASM's auto-RC (live-object-local
keepalive incs around calls, call-arg cleanup) runs UNCONDITIONALLY on top of the
drop pass's ops → double-count. For values the suite covers this stays balanced
(inc before / dec after a call); it manifests as the +1 leak whenever the drop
pass owns the release. FIX DIRECTION: mirror native — read the `drop_inserted`
marker in `wasm.rs` and suppress WASM's automatic temp-RC for marked functions
(the drop pass is the sole authority). This is the WASM half of the round-7
"every backend honors `drop_inserted`" contract that native got and WASM never
did. Sizeable change in the 878KB `wasm.rs` emitter; do it as its own arc with
the finalizer matrix + a non-finalizer RC-balance corpus as the gate.

### (C) Frontend `del` is `molt_main`-only for the release
`_emit_delete_name` (`src/molt/frontend/__init__.py:13648`) emits the releasing
`DEC_REF` for the bound value ONLY inside `if self.current_func_name ==
"molt_main"` (the boxed-cell branch :13674 AND the plain-local branch :13693 are
both inside that gate). For a regular function, `del x` falls through to
:13711-13716 and just `_store_local_value(name, missing)` — NO release. This is
why (A) DCE sees the construction as dead. The drop pass is supposed to be the
sole authority on drop lanes (so a frontend DEC_REF there would double-drop under
tracked-RC / dormant native — the reason it was gated), so the RIGHT fix is NOT
to ungate the frontend DEC_REF unconditionally; it is to make the
purity/DCE/drop layer treat `del x` (and scope-exit) of a `__del__`-bearing local
as a release point even when the frontend emits no explicit op. (A) and (C) are
two faces of the same root and should land together.

## Re-deriving the evidence (diag harness was removed before commit)
* `MOLT_DEBUG_FINALIZER` (runtime, object/mod.rs): traced `maybe_run_object_finalizer`
  entry/early-returns + per-`TYPE_ID_OBJECT` inc/dec. Re-add as `[FIN]`/`[INCOBJ]`/
  `[DECOBJ]` eprintlns; on WASM the runtime blob rebuilds with the trace (the
  WASM build recompiles the runtime when sources change).
* `MOLT_DEBUG_PREDROP` (backend, drop_phase.rs): pre-drop TIR per function to a
  `predrop/<fn>.txt` artifact (use `write_debug_artifact`, NOT eprintln — the
  build subprocess swallows stderr; `MOLT_DEBUG_DROP`'s post-drop dump uses the
  artifact path and works).
* For ANY backend diag env var to reach the build subprocess it MUST be in BOTH
  cli.py knob lists (`_BACKEND_REQUEST_ENV_KNOBS` ~:175 AND the
  `_BACKEND_DIAGNOSTIC_ENV_KNOBS` frozenset ~:547). `MOLT_DEBUG_DROP`/
  `MOLT_DUMP_IR` are already there; new vars are silently dropped otherwise (cost
  ~30 min this round). `MOLT_DEBUG_ARTIFACT_DIR=/tmp/...` controls the artifact
  root (keep it OUT of repo `tmp/` which `molt clean` wipes).

## OPS notes (round-13)
* LLVM `bench_counter_words` currently FAILS TO BUILD (pre-existing, unrelated):
  `re__error___init__` called with wrong arg count (LLVM module verify) — `re`
  exception-class arity bug. Use native for the 97360 keystone; pick re-free
  benches (fib/struct/class_hierarchy) for LLVM perf.
* Build exit 144 = harness detach (build continues) — re-run incrementally to
  finish; exit 101 was the `re` LLVM verify fail above, not a detach.
