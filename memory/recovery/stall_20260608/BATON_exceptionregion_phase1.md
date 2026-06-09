# ExceptionRegion Phase 1 — recovery baton (2026-06-08 stall)

**Status:** agent ac1776 STOPPED (machine OOM-danger: 7% free, swap 916M/21G, load
17.5 — I over-subscribed build lanes; backed off to 0 of my build agents). WIP
**excellent and correctly designed but INCOMPLETE** — preserved at
`excregion_function_compiler_wip.patch` (653-line diff vs c05a4aff0,
function_compiler.rs only, +51 net at last read). Relaunch from this patch when
the machine is healthy (free > ~30%, swap not exhausted, load < ~8).

**What the agent got RIGHT (adopt verbatim):**
- Two owned exception temps, released at known exception-event ops (not SSA
  last-use): **Component A — CreationRef** (`exception_new*` SSA result, the
  `raise` arg): EXCLUDE from the func_end Swift-ARC last-use extension so its real
  last use (the `raise`) drives release. **Component B — MatchRef**
  (`exception_last*`/`exception_active`/`exception_current`/`exceptiongroup_*` SSA
  result, a fresh runtime-inc'd handler-match ref): release bound to the enclosing
  handler region's **`exception_pop`** op (doc 45 §7 ExceptionPop), reached on
  EVERY exit path (matched fallthrough AND re-raise/propagate).
- Data structures added: `exception_match_release_temps: BTreeSet<String>` +
  `exception_match_release_at_pop: BTreeMap<usize /*pop op idx*/, Vec<String>>`
  (inverse map → the merged post-handler release).
- No-dangle invariant verified: `record_exception`/`exception_context_set` inc
  their OWN slot refs, so releasing the SSA temp can never dangle a slot or
  `sys.exc_info()`.
- Gated to be INERT when the round-13 RC flip activates the TIR drop pass over
  exception CFG (must not double-free).

**What REMAINS (finish on relaunch):** populate the two sets during lowering;
emit the dec_ref at each `exception_pop` from the inverse map; the Component-A
exclusion in all three func_end extensions (mirror #46's `stateful_per_iter_temps`
pattern); the GREEN leak test `tests/differential/memory/exception_raise_catch_loop_leak.py`;
MOLT_ASSERT_NO_LEAK plateau; sys.exception() in/out; differential byte-identical;
peel 9/9. A+B TOGETHER (no asymmetric half-fix). Do NOT touch drop_insertion.rs
(round-13) or alias_analysis.rs (#73 done). See doc 45 + [[project_exception_loop_leak_baton]].

**CallFacts Phase 1 (agent a0fbb379, also stopped):** WIP at `callfacts_wip.patch`
(new `tir/call_facts.rs` + `call_graph.rs` + `inliner.rs` edits; was wiring the
table into `run_module_pipeline`'s returned `ModuleAnalysis`). Resume AFTER
ExceptionRegion (council sequencing — shares the no_throw fact). doc 47 is the spec.
