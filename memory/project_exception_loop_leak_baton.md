# Baton: per-raise exception-object leak in try/except functions (bench_exception_heavy 0.68× root)

**Status (2026-06-08):** OPEN, P0-class (leak + perf). Diagnosed by Tier-2 #77
(warm-red bench_exception_heavy 0.68×). The fix requires a **round-13 RC-flip /
value-tracking file** (`function_compiler.rs` block-tracked registration, and/or
`drop_insertion.rs` exception-CFG support), which #77 was scoped OUT of. Batoned
to **#58 / #19** with the complete inc/dec site analysis below. NOTHING is
half-landed: #77 shipped only (a) a zero-cost-when-off RC trace tool
(`MOLT_TRACE_EXC_RC`) and (b) a RED leak regression test — both pure additions,
no behavior change, no partial fix.

## Symptom (the council's warm-red target)
`bench_exception_heavy` (raise+catch ValueError in a 2M loop) runs 0.68× CPython.
#76 cycle-attributed it: `inc_ref`+`dec_ref` ≈ 22% of cycles, and found it LEAKS
~70 MiB / 30-inner-iterations. **#77 confirmed the hypothesis: the churn and the
leak are THE SAME ROOT (retention).** Every raised-and-caught exception is
retained at refcount 2 and never freed; the loop therefore re-allocates a fresh
ValueError every iteration (churn) while the dead ones pile up (leak).

## Hard evidence (MOLT_PROFILE, quiet)
A reduced raise-catch loop, varying iteration count N (raises = N for the no-`as`
shape):
- N=30 raises: `alloc_exception=30`, `live_objects` grows; **`dealloc_exception`
  counter does not exist / 0 exceptions ever deallocated.**
- N=60 raises: `alloc_exception=60`, live_objects grows by ~30 vs N=30.
- N=500_000 raises: `alloc_exception=500000`, **`live_objects=500628`** — EVERY
  exception is live at exit. `MOLT_ASSERT_NO_LEAK` (live ≤ 200_000) → LEAK → abort
  (exit 137). RSS itself stays small only because exception headers are ~64 B; the
  object-count leak is total and unbounded.

Reproduced byte-identically on BOTH native (Cranelift) and LLVM
(`alloc_exception=30`, `live_objects` 656 native / 706 llvm, dealloc 0) → the leak
is in the **shared frontend IR ownership model**, not a backend-specific quirk.

## The exception-object lifecycle map + EXACT inc/dec sites
Per raised-and-immediately-caught exception (global-slot path, no asyncio task;
captured live with `MOLT_TRACE_EXC_RC=1 MOLT_DEBUG_EXCEPTION_FLOW=1
MOLT_TRACE_EXCEPTION_STACK=1`). `alloc` makes rc=1 (owner = the
`exception_new_builtin_one` SSA result = the `raise` argument, "creation ref"):

```
exception_stack_push            (active-stack slot = None placeholder)
molt_raise(exc):
  record_exception → global_last_exception_store_recorded(same_ptr=false)
                                  INC #1  rc 1→2   [global last_exception slot owns +1]
                                  (exceptions.rs:972  global_last_exception_store_recorded)
  exception_context_set(exc)      INC #2  rc 2→3   [ACTIVE_EXCEPTION_STACK slot owns +1]
                                  (exceptions.rs:1232 inc_ref_bits in exception_context_set)
handler dispatch:
  exception_last_pending →        INC #3  rc 3→4   [fresh owned ref for match + `as e` bind]
                                  (exceptions.rs:6418 inc_ref_bits in exception_last_pending_bits)
  exception_clear → clear_exception → global_last_exception_take + dec
                                  DEC     rc 4→3   [releases slot #1]
                                  (exceptions.rs:1839 dec_ref_bits in clear_exception)
  (handler body runs; `as e` binds the INC #3 value via store_var — borrow-only uses
   by exception_match_builtin / exception_context_set do NOT consume it)
handler exit:
  EXCEPTION_CONTEXT_SET(None) → dec active-stack slot
                                  DEC     rc 3→2   [releases slot #2]
                                  (exceptions.rs:1221 dec_ref_bits, slot→None branch)
  exception_stack_pop             (slot already None — NO dec)
→ rc STAYS AT 2 FOREVER (LEAKED)
```

**3 inc, 2 dec → final rc = 2.** The two un-released owned references are:
1. **The creation ref** — the `exception_new_builtin_one` (or call) SSA result
   `exc_new_val`, the argument to `raise`. `molt_raise` BORROWS it (incs the slots
   #1/#2 with their own refs) and never consumes it; nothing releases it.
2. **The exception_last_pending ref (INC #3)** — the handler's matching/binding
   SSA value `exc_val`. Released by no one at handler exit.

The two DECs that DO fire release the slot refs (#1, #2), not these two SSA-owned
refs. **The leak is identical WITH or WITHOUT `except ... as e`** (proven: the
no-`as` shape leaks the same rc=2) → the leak is NOT the implicit-`del e`
lowering. The `del e` (frontend emits `store_var(missing)` with NO `dec_ref`,
confirmed in pre-midend IR) is a red herring here: at function scope it relies on
the value-tracking RC to release the displaced binding, which is exactly the
mechanism that is broken for these temps.

## ROOT CAUSE (structural)
A function containing a real `try`/`except` handler is **NEVER processed by the
TIR drop-insertion pass**:

    runtime/molt-backend/src/tir/passes/drop_insertion.rs:450
      if func.has_exception_handlers() || func.has_state_machine() { return stats; }

This bailout is PRINCIPLED, not a bug: drop placement keys on single-entry
dominance (per-block last-use, edge-dying at successor entry), which is unsound
over exception CFG (the handler block is reached from a non-dominating raise
site). So in any try/except function the TIR-drop release path is OFF, and the
**native value-tracking RC is the only release mechanism.**

But the exception-object-producing ops register their OWNED results in NEITHER
of the value-tracking RC's release sets:
- `exception_new` / `exception_new_builtin` / `exception_new_builtin_one` /
  `exception_new_from_class` (the creation ref), and
- `exception_last` / `exception_last_pending` (the handler ref)

are all dispatched to `fc::exceptions::handle_exception_op`
(`runtime/molt-backend/src/native_backend/function_compiler/fc/exceptions.rs`),
which calls the runtime and `def_var_named`s the result **without ever pushing
the result name into `block_tracked_obj` (block-scoped temps) or
`tracked_obj_vars` (function-scoped tracked vars).** Therefore the existing
`check_exception` diverted-control drain
(`function_compiler.rs:18092-18104`, which DOES correctly release
`block_tracked_obj` temps of the current block right before the brif to the
handler) never sees them, and func-end `drain_cleanup_tracked` never sees them.
Result: both owned exception temps leak, once per iteration, unbounded.

This is the SAME bug CLASS as genleak #46 (release deferred past the per-iteration
scope), specialized to exception-handler functions where drop-insertion is
disabled and the value-tracking RC simply never registers the exception temps.

## THE FIX (structurally correct — round-13 / value-tracking; #58/#19 owns these files)
Two acceptable end-states; (A) is the smaller, lower-risk one and is preferred:

**(A) Register exception-producing op results as value-tracked temporaries.**
In `fc::exceptions::handle_exception_op` (or at the dispatch site in
`function_compiler.rs:17913`), after `def_var_named` for the owned-result ops
(`exception_new*`, `exception_last`, `exception_last_pending`, and the
`exceptiongroup_*` producers), register `op.out` into `block_tracked_obj` for the
current block, EXACTLY like every other heap-producing op (string/list/dict
alloc, the generator `(value,done)` pair at `function_compiler.rs:13945`). The
result is rc=1-owned; registering it makes the existing per-block /
`check_exception` / loop-boundary drains release it at its real last-use within
the iteration. NOTE the borrow-only consumers: `exception_match_builtin`,
`exception_context_set`, and the `as e` `store_var` bind must keep their
last-use accounting correct so the drop fires AFTER the bind's lifetime — verify
against the existing `last_use` machinery, do not special-case.
  - Must mirror to ALL FOUR backends sharing this IR (native, LLVM, WASM, Luau) —
    asymmetry re-creates the leak on the unmigrated backend (CLAUDE.md).

**(B) Re-enable drop-insertion for exception-handler CFG.** Give
`drop_insertion.rs` def-reaching (not pure-dominance) liveness for handler
regions so the bailout at line 450 can be lifted for `has_exception_handlers()`
(the `has_state_machine()` half stays — that is the separate StateSwitch
follow-up). This is the larger structural arc and subsumes (A); it also fixes any
other owned temp that leaks in try/except functions today.

Do NOT attempt the localized frontend hack of "emit `DEC_REF(exc_new_val)` after
RAISE only when the raise arg is a fresh construction" — it special-cases ONE
raise form (`raise ValueError(i)`), silently re-leaks `raise <var>` / bare
`raise` / `raise ... from ...` / exception groups, and risks a double-free the
moment any of those paths starts being value-tracked. The correct fix is the
single uniform registration in (A).

## Byte-identical exception SEMANTICS that the fix MUST preserve (tripwires)
The fix frees the exception SOONER, so it must not free one still in use:
- `__context__` / `__cause__` chaining (implicit + `raise X from Y`), `__traceback__`
  content + caret columns, re-raise, bare `raise`, exception groups / `except*`,
  `finally` ordering + finally-raises-new chaining, the `sys.exc_info()` observable.
- The full exception differential corpus (`tests/differential/basic/exception_*`,
  `exceptiongroup_*`, `traceback*`) byte-identical native + LLVM. Spot-checked
  GREEN at baseline today: `exception_target_cleanup`, `exception_complex`,
  `exception_cause_context_chain` (all status=pass, output byte-identical).
- `exception_is_rooted` (`exceptions.rs:1069`) is the safety net: a rooted
  exception (still in a slot / active stack) RESURRECTS instead of freeing at rc 0
  — keep it; the fix should make rc reach 0 only AFTER the handler truly exits.

## Gates the fix must clear (the #77 contract)
- BEFORE/AFTER via #76 tooling on a QUIET machine:
  `python3 tools/perf_scoreboard.py --benchmark bench_exception_heavy --sample-hot-only
   --inner-repeat N --emit-cycle-profile` → report inc_ref/dec_ref % drop +
  warm_speedup before→after. Target warm > 1.00 native release-fast.
- The RED regression `tests/differential/memory/exception_raise_catch_loop_leak.py`
  (added by #77) must go GREEN under `MOLT_ASSERT_NO_LEAK=1` (live_objects O(1),
  not ~840k). Verify with `MOLT_TRACE_EXC_RC=1`: each exception must reach
  `EXC_RC_FREE` (rc 0) per iteration, not strand at rc 2.
- `cargo -p molt-runtime --lib` + `-p molt-backend --features "llvm native-backend"
   --lib` 0 fail / 0 warn. peel 9/9 ×2. compliance 46/46. No protected-green
  regression (class_hierarchy / struct / bytes_find).

## Diagnostic tool shipped by #77 (USE it — zero-cost when off)
`MOLT_TRACE_EXC_RC=1` prints every `TYPE_ID_EXCEPTION` refcount transition
(EXC_RC_INC / EXC_RC_DEC / EXC_RC_RESURRECT / EXC_RC_FREE). Routed through a
cached `OnceLock` flag `trace_exception_rc()` (`object/mod.rs`, sibling of
`debug_bigint_rc`), so it takes the libc environ lock once at first use and is a
single atomic load when off — it does NOT tax the hot inc/dec path. This is the
tool that pinned the 3-inc/2-dec/rc-2 retention; the fix is "done" exactly when it
shows EXC_RC_FREE per iteration.

## Repro one-liners
```
# rc-2 retention trace (2 raises):
printf 'def main():\n c=0\n for i in range(2):\n  try:\n   raise ValueError(i)\n  except ValueError:\n   c+=1\n print(c)\nmain()\n' > /tmp/e.py
python3 -m molt build --target native --release --output /tmp/e /tmp/e.py
MOLT_TRACE_EXC_RC=1 python3 tools/safe_run.py --rss-mb 512 --timeout 15 -- /tmp/e   # INC,INC,INC,DEC,DEC → rc 2

# leak at scale (trips MOLT_ASSERT_NO_LEAK):
MOLT_ASSERT_NO_LEAK=1 python3 tools/safe_run.py --rss-mb 1024 --timeout 40 -- /tmp/exc_regr  # exit 137, live≈840k
```
