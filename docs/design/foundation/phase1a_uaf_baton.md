# Phase 1a (§2.7 back-edge drop-old) — baton-pass: UAF on conditional-wrapped loops

Status: **verified-correct for flat loops, INCOMPLETE (UAF) for conditional-wrapped loops.
NOT on main, NOT committed.** Do not land until the structural fix below is in and the full
binary gate is green.

## What is done (and verified)
- The §2.7 back-edge drop-old (the Perceus mutable-slot `drop-old`, dual to §5) is implemented in
  worktree `.claude/worktrees/agent-ab6ad8bdf6890742b`, file
  `runtime/molt-passes/src/tir/passes/drop_insertion.rs` (the block at ~3029-3181 replacing the old
  §2.7 no-op).
- FLAT loops are fixed and measured: `for i in range(1_000_000): d = {...}` goes **513 MB → 15 MB**,
  correct output. 1109/1109 `cargo test -p molt-tir` pass (incl. the accumulator and §5 borrowed-phi
  guards, plus two new RED-confirmed regressions).

## The bug (definitive, measured)
- `if c: for i in range(n): d = {...}` aborts with `molt fatal: invalid object header before dec_ref`
  (a use-after-free; molt's runtime guard catches the bad dec_ref and aborts — it does NOT silently
  corrupt). The flat form of the SAME loop does not crash.
- Reproducers (in the worktree `tmp/`): `repro_flat.py` (clean, 513→15 MB), `repro_min.py` (crashes),
  `repro_min_init.py` (adds `d = None` before the loop → **clean**), `mem_probes.py` (the matrix;
  dict/list/tuple/set/comp/nested_loop crash, the rest are clean).
- **Root cause:** the §2.7 drop `dec_ref`s the iteration-1 phi value = the preheader's INITIAL. When a
  loop-local is first-bound INSIDE a conditional-wrapped loop, the preheader delivers an **undef /
  garbage** value (not `None`); `DecRef(undef)` reads a garbage object header → abort. The FLAT path
  initializes the slot to a safe value; the CONDITIONAL path does not. `d = None` pre-init makes the
  preheader deliver a valid immortal and the crash vanishes — confirming the cause.

## Why cargo tests passed but it crashed
The 1109 unit tests construct hand-built CFGs with valid initial phi values; none built the
uninitialized-initial-under-a-conditional shape. **The E2E compiled-binary battery is the real gate
for any drop-insertion change; unit-tests-green is necessary but NOT sufficient.**

## The structural fix (do THIS, not a drop_insertion guard)
Per CLAUDE.md ("fix the abstraction, not the edge case"; "never re-patch DropInsertion with a
special-case") and the §5 invariant ("the phi drop is sound ONLY when EVERY incoming edge delivers an
owned +1"):

The fix is in the **frontend lowering / SSA construction**: an uninitialized loop-carried local slot
MUST enter its header phi as `None` at EVERY preheader edge, **uniformly** — identically whether or
not a conditional wraps the loop. The conditional-wrapped path currently passes `undef`; make it pass
`None` like the flat path. With that invariant restored, the §2.7 drop is correct everywhere
(DecRef(None) is a no-op on iter 1; real drops on iter 2+), the conditional case is **leak-fixed** (not
merely made safe), and no drop_insertion guard is needed.

- DO NOT add a fail-closed "skip the drop if the preheader is undef" guard to §2.7. It would be a
  localized hack AND would leave the conditional case LEAKING (a partial fix the policy rejects).
- Investigation start: grep the frontend SSA/loop lowering (`src/molt/frontend/`, incl. the local-
  binding / phi-placement path — the agent noted `local_bindings.py` gates on
  `current_func_name != "molt_main"`) for where a loop-carried local's initial/preheader value is
  produced; find why the conditional path yields undef vs the flat path's safe init. Add a native
  bootstrap/differential regression in the same change.

## Frontend investigation (this session) — fix site narrowed
- Local-binding init lives in `src/molt/frontend/lowering/local_bindings.py`. `_box_local`
  (lines 104-137) seeds a BOXED local to `_emit_missing_value()` (the MISSING unbound-sentinel)
  iff it is in `scope_assigned`/`del_targets`, else to `CONST_NONE` (lines 133-137). The comment at
  lines 49-51 states the intended mechanism: the loop-header phi merges the cell SSA with the
  **entry-block default `None`** on the first iteration.
- CRASH EVIDENCE rules out both seeds: the value §2.7 dec_refs has VARYING garbage `type_id`s across
  runs (3929759456 / 876306096 / 40 / 0) — that is POISON (unseeded memory), NOT the consistent
  MISSING sentinel and NOT `None`. So the conditional-wrapped loop-carried local's preheader phi-arg
  is genuinely UNSEEDED.
- LEAD (where to look next, in order): (1) determine whether the loop-local is BOXED or a plain SSA
  local in the flat vs conditional case — a differing boxing/scope decision under the conditional
  (check `src/molt/frontend/lowering/analysis_collect_static.py` and the boxing predicate) would
  explain why the seed is skipped; (2) find where plain SSA-locals (not `_box_local`) get their
  entry-default `None` seed and why a conditional preheader misses it; (3) if the frontend emits the
  seed correctly, the gap is the molt-tir SSA construction (`runtime/molt-ir/src/tir/ssa.rs`)
  building the loop-header-phi preheader arg through the conditional. The invariant to restore:
  EVERY loop-carried local's header phi receives a valid `None` (or MISSING) seed on EVERY preheader
  edge, conditional-wrapped or not — then §2.7 is correct unchanged.

### SSA construction analysis (deeper — the seed mechanism)
The seed lives in `runtime/molt-ir/src/tir/ssa.rs` `rename_and_emit`:
- `undef_value` (lines 696-697) is a `ConstNone` emitted ONLY in the ENTRY block (lines 774-783),
  which dominates every block — so a branch arg equal to `undef_vid` is a VALID `None` everywhere.
- A branch arg for a variable with no reaching def resolves to `undef_value` (line 803,
  `.or(self.undef_value)`).
- The second pass (lines 908-963) re-resolves `undef_vid` branch args by walking the dominator
  chain for a real reaching def; for a loop-local first-bound INSIDE the loop, no dominator of the
  preheader defines it, so the walk KEEPS `undef_vid` (None).
- BY THIS MODEL the conditional preheader's loop-phi arg should be `undef_vid` = None (DecRef-safe).
  But the crash drops POISON (varying garbage `type_id`s), so the ACTUAL conditional CFG/SSA diverges
  from this model — a use-before-def branch arg, or a second-pass mis-resolution under the
  conditional CFG. This MUST be inspected directly to fix correctly.

**TO PIN + FIX without the build daemon:** write a `ssa.rs` unit test that constructs the
`if c: for i: d = {...}` CFG by hand — entry → CondBranch → { preheader → Branch header(d_seed,i_seed) ;
after } ; header(d_phi,i_phi) → CondBranch → { body → Branch header(d_new,i_next) ; loop_exit } — run
`rename_and_emit`, and assert the header block's PREHEADER-edge arg for `d` equals `undef_vid` (the
ConstNone). It will likely be a use-before-def poison value, reproducing the UAF at unit-test level,
where the branch-arg computation / second-pass resolution can be fixed and verified with `cargo test`
alone. (TOOLING NOTE: `TIR_DUMP=1` / `MOLT_TIR_DUMP` execute inside the build daemon and are NOT
relayed to the foreground build's stderr — only "Compiling/Successfully built" is — so an IR dump
needs a daemon-bypass or the daemon's own output channel; the unit-test route sidesteps this.)

## Verification gate (ALL on the compiled binary, not cargo test)
1. `repro_flat` 513→15 MB, no crash; `repro_min` no crash, correct output.
2. EVERY `mem_probes.py` pattern (baseline,dict,list,tuple,set,str,obj_slots,obj_dict,nested,cond,
   comp,nested_loop,funcret) runs crash-free with correct output, and peak RSS is FLAT from 1M→10M
   (bounded; `str_accum_On` is the O(n) control that should grow).
3. Differential memory battery: `tests/differential/memory/{alias_reassign_*, resurrect_*,
   finalizer_resurrection_leak_gauge, custom_object_loop_phi_retain, string_concat}.py` via
   `python tests/molt_diff.py <files> --jobs 1` — no UAF, output matches CPython.
4. 1109 cargo tests still pass + a NEW cargo regression for the conditional-wrapped-loop CFG.
5. molt-vs-CPython peak RSS table (every pattern ≤ CPython or the gap optimized). NOTE: the
   measurement harness `tmp/cpython_rss.py` self-report currently returns 0 — fix it (the launcher
   broke the external poll; self-report via Win32 GetProcessMemoryInfo is the reliable path) before
   trusting the comparison.

## Pointers
- Design: `docs/design/foundation/rc_gc_redesign.md` (adversarially-reviewed two-tier RC+GC).
- Stub queue: `docs/design/foundation/noop_stub_inventory.md` (54 stubs, P0s first).
- The §2.7 implementation diff: worktree above; reviewed sound for the flat case in this session.
