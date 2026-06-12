# Baton: per-raise exception-object leak in try/except functions (bench_exception_heavy 0.68× root)

**Status (2026-06-08, REFINED by #77 recovery/excfix2):** OPEN, P0-class (leak +
perf). The leak has TWO owned-temp components with DIFFERENT correct fixes; one is
value-tracking-expressible (and was prototyped correct), the other genuinely needs
**handler-region (exception-CFG) liveness** = the drop-pass arc (#58 / #19). The
`per_iter_exception_temps` WIP (warm worktree `/tmp/wt_excfix`, backup
`memory/recovery/excfix_wip/`) was REVERTED because it closes only the first
component and leaves the loop still leaking unbounded (rc 2→1, NOT rc→0) — an
asymmetric partial fix the zero-workarounds policy rejects. NOTHING half-landed:
on HEAD only (a) the zero-cost `MOLT_TRACE_EXC_RC` tool and (b) this baton; the
RED leak regression `tests/differential/memory/exception_raise_catch_loop_leak.py`
is being (re)added as a pure documentation addition (byte-identical output, RED
under `MOLT_ASSERT_NO_LEAK`).

## Symptom (the council's warm-red target)
`bench_exception_heavy` (raise+catch ValueError in a 2M loop) runs 0.68× CPython.
#76 cycle-attributed it: `inc_ref`+`dec_ref` ≈ 22% of cycles, and found it LEAKS.
The churn and the leak are THE SAME ROOT (retention): every raised-and-caught
exception is retained and never freed, so the loop re-allocates a fresh ValueError
(plus its arg string) every iteration (churn) while the dead ones pile up (leak).

## Hard evidence (HEAD, native, quiet — re-measured 2026-06-08)
`exception_raise_catch_loop_leak.py` (3 shapes, the regression):
- `MOLT_ASSERT_NO_LEAK=1`: **live_objects=840657** (alloc=840686, dealloc=29) →
  FAIL (expected_live ≤ 200000). Output byte-identical to CPython 3.14
  (19999900000 / 200000 / 120000) — the defect is purely the leak.
- Reduced shapes @ 200k iters each: as-bind `live≈200622`, no-as `live≈200623`,
  raise-from chain `live≈400629` (2 exceptions/iter). EVERY caught exception
  leaks ≥1 object/iter on BOTH native and LLVM.

## EXACT refcount lifecycle (live `MOLT_TRACE_EXC_RC` trace, 1 raise, global slot)
Per raised-and-immediately-caught exception (`raise ValueError(i); except ValueError`),
`alloc` makes rc=1 owned by the **creation ref** (the `exception_new_builtin_one`
SSA result = the `raise` argument). Op indices are from the final func IR (dump via
`MOLT_DUMP_FINAL_FUNC_IR=main` → `tmp/molt-backend/native/final_ir/…_main.txt`):

```
op55 exception_new_builtin_one out=v_create   rc=1   [creation ref, last_use = op56 (the raise)]
op56 raise(v_create):
       record_exception → global slot store   INC rc 1→2   [global last_exception owns +1]
       (handler active) exception_context_set  INC rc 2→3   [ACTIVE_EXCEPTION_STACK owns +1]
op66 jump → handler:  dec_ref(v_create)        DEC rc 3→2   [** creation ref RELEASED here **]
op77 exception_last_pending out=v_match        INC rc 2→3   [handler-match ref, FRESH owned]
       last_use(v_match) = op93  (the ELSE-branch re-raise — see below)
op78 exception_clear → clear_exception(global) DEC rc 3→2   [global slot released]
op82 if v116(match?)  → MATCHED branch op83-91 (exception_clear, exception_context_set(v_match),
       inplace_add, …):  v_match used ONLY by BORROW ops — never dec'd on this path
op95 end_if;  op100 exception_pop;  op103 check_exception
handler exit: exception_context_set(None)      DEC rc 2→1   [active stack released]
→ rc STAYS AT 1 (LEAKED).  v_match's only owned reference is never released on the matched path.
```

**Net on the caught path: 3 inc, 3 dec, final rc=1.** The single un-released owned
reference is the **handler-match ref** (`exception_last_pending` result `v_match`).
(Historical note: the prior baton said rc=2 with the creation ref ALSO leaked. The
frontend IR has since evolved AND the `exception_new*` result IS now release-able
by value-tracking — see "WIP post-mortem" — so the creation-ref half is no longer
the blocker. The handler-match half is.)

## ROOT CAUSE — refined, two components (one fixable in value-tracking, one not)

### Tracking IS now wired (correcting the prior baton)
The prior claim "exception ops register their results in NEITHER `block_tracked_obj`
NOR `tracked_obj_vars`" is **OUTDATED**. The exception-op match arm in
`function_compiler.rs` (≈ line 18050, `handle_exception_op` dispatch) does NOT
`continue`; it falls through to the **generic per-op tail registration**
(`function_compiler.rs` ≈ line 24992: any `out_name` that is not `none`,
`!drop_inserted`, not slot-backed-join, not `rc_skip_dec`, not a param → pushed
into `block_tracked_obj` for the current block, or `tracked_obj_vars`/`entry_vars`
at entry-block/loop_depth 0). Exception-op results ARE owned-obj (not ptr), so they
land in `block_tracked_obj`. Confirmed by probe: `REGISTER op55 exception_new… last_use=56`
and `REGISTER op77 exception_last_pending… last_use=93`. So they ARE drained by the
ordinary `jump` / `check_exception` / `end_if` drains — *at their `last_use`*.

### Component A — creation ref (`exception_new*`): VALUE-TRACKING-EXPRESSIBLE ✓
`last_use(v_create) = op56` (the `raise`). The func_end Swift-ARC lifetime
extension (`function_compiler.rs`, the three `last_use` → func_end loops:
structured-loop, back-edge, alias-group) OVER-extends it to func_end, so the per-
iteration `jump`-drain (op66) never fires within the iteration. The `per_iter_exception_temps`
WIP fixed exactly this: exclude `exception_new*` (and the other owned exception
producers) that are per-iteration-dead from the func_end extension, so `last_use`
stays at op56 and the op66 jump-drain releases the creation ref. **This is the SAME
pattern as genleak #46** (release deferred past the per-iteration scope), and the
release point (op56, after `molt_raise` has recorded its own independent slot refs)
is lifecycle-correct (verified: `record_exception`/`global_last_exception_store_recorded`/
`exception_context_set`/`molt_exception_last*` ALL take their OWN inc'd reference —
releasing the SSA temp can never dangle a slot; `sys.exc_info()` reads the active
stack, not the temp). The WIP's release was confirmed by trace (the op66 `DEC 3→2`).

### Component B — handler-match ref (`exception_last*` / `exception_active` / `exception_current`): NEEDS REGION LIVENESS ✗
`last_use(v_match) = op93`, which is the `raise v_match` on the **ELSE (no-match)
branch** of the handler's `if match?` (op82). On the **matched** branch (the common
case — the exception IS caught) v_match is touched only by BORROW ops
(`exception_match_builtin`, `exception_context_set`) and its global last_use (op93)
is in the SIBLING branch that does not execute. The value-tracking RC keys releases
on a SINGLE global `last_use` index, so it inserts the only `dec` at op93 — which
never runs on the matched path. ⇒ **v_match leaks on every caught exception.** This
is NOT fixable by the func_end-extension exclusion (the WIP already keeps v_match's
last_use at op93, not func_end — it still leaks). It is the exception-CFG liveness
divergence: the handler-match ref's correct release point is **handler-region exit**
(CPython clears the caught exception when the `except` block completes — implicit
`del`), which is a per-PATH fact a single-last-use model cannot express.

## THE FIX (structurally correct) — release the handler-match ref at handler-region exit
The handler-region exit is marked in the IR by **`exception_pop`** (op100/op101),
which is reached on ALL paths: matched (op…→end_if→jump→exception_pop) AND
re-raise/propagate (op93 raise→end_if→…→exception_pop→check_exception→propagate).
Two acceptable end-states; both are the deeper arc (#58 / #19 owns these files):

**(B1) Region-aware value-tracking release.** Treat the owned handler-match
producers (`exception_last`, `exception_last_pending`, `exception_active`,
`exception_current`, and the `exceptiongroup_*` match results) as a distinct class
whose release is bound to the enclosing handler region's `exception_pop`, not to
their SSA last_use: extend their `last_use` to (and carry them via the normal
block-carry through jumps/branches to) the `exception_pop` op, and drain there on
the merged post-handler path. Feasibility CONFIRMED for the simple cases:
  - Matched path: v_match's only owned ref → release at `exception_pop` = rc→0 free. ✓
  - Re-raise/propagate path: op93 `raise` recorded a NEW independent slot ref, so
    releasing v_match's handler-match ref at `exception_pop` is safe (the slot keeps
    the propagating exception alive). ✓
  - `except E as e` with INLINE use (no user store_var/`del e` emitted — verified in
    the optimized IR, the `as e` binding folds into direct SSA uses of v_match):
    no double-free, since nothing else releases v_match. ✓
  MUST still be proven for: (i) `except E as e` where the frontend DOES emit an
  explicit `store_var(e)` + `del e` (store_var-displaced-binding release could
  double-free with the region release — check the `is_join_slot` / store_var
  retain-new/release-old accounting); (ii) NESTED handler regions (from-chain:
  inner `exception_pop` vs outer — each match ref must bind to ITS region's pop, so
  the carry must be region-scoped, e.g. an `exception_push`/`pop` depth stack);
  (iii) `finally` regions and `except*`/exceptiongroup splits; (iv) the value
  stored into `__context__`/`__cause__` (op82/op91 `exception_context_set`) must
  remain reachable through the chain — releasing the handler-match ref must not free
  the object while a slot/chain link still borrows it (it won't, because every slot
  holds its own inc'd ref — but assert it in the chain tests).
  This is the ExceptionRegion design: a region table (push/pop balanced) that maps
  each handler-match SSA temp to its region-exit op, with the release emitted once
  on the merged exit. Mirror to ALL FOUR backends sharing the IR (native, LLVM,
  WASM, Luau) — asymmetry re-creates the leak on the unmigrated backend.

  Land Component A (the creation-ref `per_iter_exception_temps` exclusion — already
  prototyped and lifecycle-verified) **together with** B1 as ONE structural arc, so
  the two owned exception temps are released symmetrically. Landing A alone is the
  asymmetric partial fix that was reverted.

**(B2) Re-enable drop-insertion for exception-handler CFG.** Give
`drop_insertion.rs` def-reaching (not pure-dominance) liveness for handler regions
so the bail at `drop_insertion.rs:450` (`has_exception_handlers()`) can be lifted
(the `has_state_machine()` half stays — separate StateSwitch follow-up). This is the
larger arc and SUBSUMES both A and B1: the drop pass naturally places the release on
the matched path (def-reaching liveness sees v_match dead at handler exit on that
path) and on the creation-ref's true last-use. Preferred long-term; B1 is the
smaller bridge if the drop-pass re-enable is not yet ready.

Do NOT attempt the localized hack "emit `DEC_REF` after the matched branch only" —
it special-cases ONE handler shape, silently re-leaks re-raise / nested / `except*`,
and risks double-free the moment any of those is touched.

## Byte-identical exception SEMANTICS the fix MUST preserve (tripwires)
The fix frees the exception SOONER, so it must not free one still in use:
- `__context__`/`__cause__` chaining (implicit + `raise X from Y`), `__traceback__`
  content + caret columns, re-raise, bare `raise`, exception groups/`except*`,
  `finally` ordering + finally-raises-new chaining, the `sys.exc_info()` /
  `sys.exception()` observable (reads the ACTIVE stack, which holds its own ref —
  releasing the SSA temp must not change it; ADD a differential that calls
  `sys.exc_info()` at the END of the handler, after the match-ref's apparent SSA
  last-use, and asserts it still returns the live exception).
- A STORED/returned exception (`saved = e`, append to list) must SURVIVE across
  iterations: the in-loop `store_var` gives the slot an independent inc'd ref, so
  releasing the source temp at its last use does not free the stored object — but
  the region release must NOT fire on a match ref that was stored (verify the
  store_var-target exclusion still applies, or the region table excludes stored
  refs).
- The full exception differential corpus (`tests/differential/basic/exception_*`,
  `exceptiongroup_*`, `traceback*`) byte-identical native + LLVM. Spot-checked GREEN
  at HEAD: `exception_target_cleanup`, `exception_complex`, `exception_cause_context_chain`.
- `exception_is_rooted` (`exceptions.rs:1069`) is the safety net: a rooted exception
  (still in a slot / active stack) RESURRECTS instead of freeing at rc 0 — keep it;
  the fix should make rc reach 0 only AFTER the handler truly exits.

## Gates the fix must clear (the #77 contract)
- BEFORE/AFTER via #76 tooling on a QUIET machine:
  `tools/perf_scoreboard.py --benchmark bench_exception_heavy --backend native
   --profile release-fast --require-quiescent --repeat 5 --inner-repeat --emit-cycle-profile`
  → report inc_ref/dec_ref % drop + warm_speedup before→after + RSS-plateau.
  Target warm > 1.00 native release-fast (leak-fixed-but-warm-flat = CORRECTNESS_FIX
  + DIMENSIONAL_WIN; warm>1 = GREEN heal).
- `tests/differential/memory/exception_raise_catch_loop_leak.py` GREEN under
  `MOLT_ASSERT_NO_LEAK=1` (live_objects O(1), not ~840k). Verify with
  `MOLT_TRACE_EXC_RC=1`: each exception reaches `EXC_RC_FREE` (rc 0) per iteration,
  not strand at rc 1 (Component B) or rc 2 (the historical pre-A state).
- `cargo -p molt-runtime --lib` + `-p molt-backend --features "llvm native-backend"
   --lib` 0 fail / 0 warn. peel 9/9 ×2. compliance 46/46. No protected-green
  regression (class_hierarchy / struct / bytes_find).

## Diagnostic tools (USE them — zero-cost when off)
- `MOLT_TRACE_EXC_RC=1` (on HEAD): every `TYPE_ID_EXCEPTION` rc transition
  (EXC_RC_INC / EXC_RC_DEC / EXC_RC_RESURRECT / EXC_RC_FREE). Cached `OnceLock` flag
  `trace_exception_rc()` — one atomic load when off. This pinned the 3-inc/3-dec/rc-1
  retention; the fix is "done" when it shows EXC_RC_FREE per iteration.
- `MOLT_DUMP_FINAL_FUNC_IR=main` (+ `MOLT_DEBUG_ARTIFACT_DIR=…`) writes the post-
  midend func IR (kind/out/var/args per op) to `<dir>/native/final_ir/…_main.txt` —
  this is how the op55/op66/op77/op93/op100 lifecycle above was mapped. The backend
  daemon's stderr goes to its LOG (`…/.molt_state/backend_daemon/*.log`), not the
  client; `--no-cache` forces a fresh compile so any temporary probe fires.
- `MOLT_DEBUG_EXCEPTION_FLOW=1` + `MOLT_DEBUG_EXCEPTION_RC=1`: interleaved
  `molt exc raise/SET/context_set/last_pending/clear` flow markers + rc snapshots.

## Repro one-liners
```
# rc-1 retention trace (1 raise) — see the single un-released handler-match ref:
printf 'def main():\n c=0\n for i in range(1):\n  try:\n   raise ValueError(i)\n  except ValueError:\n   c+=1\n print(c)\nmain()\n' > /tmp/e.py
PYTHONPATH=$PWD/src python3 -m molt build --target native --output /tmp/e /tmp/e.py
MOLT_TRACE_EXC_RC=1 MOLT_DEBUG_EXCEPTION_FLOW=1 python3 tools/safe_run.py --rss-mb 512 --timeout 15 -- /tmp/e
#   → INC,INC,DEC(creation@op66),INC(match),DEC(clear),DEC(active None) → rc 1, no EXC_RC_FREE

# leak at scale (trips MOLT_ASSERT_NO_LEAK):
MOLT_ASSERT_NO_LEAK=1 python3 tools/safe_run.py --rss-mb 1024 --timeout 40 -- /tmp/leak_regr  # FAIL, live≈840k
```
