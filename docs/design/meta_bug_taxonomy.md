# Meta-bug & bug-class taxonomy (verification-machinery adversarial audit)

Output of the recursive adversarial pass against molt's VERIFICATION/AUTHORITY
machinery (audit `w342n3wty`). The operator's framing: the recurring findings are
not instances — they are **bug classes** (patterns that mint instances) and
**meta-bugs** (bugs in the machinery meant to *catch* bugs). A green from a fooled
catcher is worse than no catcher: it is false confidence. Fix = make the class
unexpressible (canonicalize the authority + a CI drift-gate), and fix the
meta-bugs first because an untrustworthy catcher invalidates every green beneath it.

## The master class

**PROXY-MEASUREMENT SUBSTITUTION.** A verifier measures a *cheap proxy* correlated
with the real invariant on the happy path and **decorrelated exactly where the bug
lives.** Every meta-bug below is an instance:
- aggregate-live proxies rate-of-growth (leak ceiling)
- current-RC proxies confirmed-death (weakref `rc<=2`)
- a markdown banner proxies JSON provenance (freshness)
- a dispatch-shape test proxies gate-execution (panic contract)
- **a unit-test-of-the-authority proxies the-authority-having-run** (the unwired perf gate)

Unexpressible-making fix pattern: replace each proxy with a check on the SAME
authority the system already trusts elsewhere, + a drift-gate that fails when a
verdict is computed from anything but that authority.

## Other bug classes
- **Authority single-sourced-in-NAME, not-in-REACH** (a correct guard + an unguarded twin): callers route the happy path through the authority; a fallback/sibling re-derives it unguarded. Fix: delete every direct re-computation; one function is the only path; ripgrep drift-gate.
- **Fact-in-channel-A, check-in-channel-B** (provenance written where the gate can't read). Fix: one emitter writes the fact into every channel; a dual-channel gate.
- **Per-unit guard without a collective ledger** (N x per-unit-limit > global). Fix: a shared budget pool.
- **Finalizer-driven re-escape / resurrection-at-a-distance** (static lifetime invalidated by `__del__`): UAF at a weakref callback / container inner-pointer. Fix: transitive FinalizerSensitive + defer drop to the Python boundary + a finalization-state CAS enum.
- **Tier conflation in the repr lattice** (one carrier tier proves two unrelated facts). Fix: one seed per tier + `repr_audit()` coherence gate.
- **Audit unit too coarse** (opcode-centric, blind to opcode x lane / x backend cells). Fix: a per-(opcode x lane x classifier) coverage audit.
- **Green-on-red via missing prerequisite** (a gate runs without proving its upstream succeeded — pytest atop a NameError-ing compiler; continue-on-error masking a crash). Fix: a blocking build-success tier-1 prerequisite; stderr-aware conftest.

## Fix queue (priority order; meta-bugs first)
1. **[DONE — `e3bf3cfab`] Wire the canonical perf gate to fire on main** + `check_perf_gate_wiring.py` fail-closed audit in `ci_gate.py` tier-1. (The sharpest: an unwired gate hid the ENTIRE perf dimension on every PR/merge.)
2. **[DONE] Ratio-direction canonicalization** —
   `molt.metric_ratios.signed_ratio` / `signed_ratio_value` are the sole
   wall-clock ratio implementation authority (`tools/perf_authority.py`
   re-exports them for benchmark tooling), every serialized ratio carries
   `RatioDirection` metadata, degenerate operands produce `None`, and
   `tools/check_ratio_direction.py` is an AST drift-gate in `ci_gate.py` tier-1
   rejecting raw `<x>_time / <y>_time` outside the source authority.
3. **[META] Leak detector aggregate → rate-of-growth** — `assert_no_leak_at_exit` 200K fixed ceiling (`metrics.rs:28`) launders bounded resurrection leaks. → per-object `alloc_epoch`; a bounded-leaker test MUST fail.
4. **[META] Single-source object-death** — move `DEALLOC_COUNT` to the dealloc TAIL (after weakref_clear + interior free); `weakref.rs:199` `rc<=2` heuristic consults the DEALLOC authority instead.
5. **[META] Dual-channel freshness** — emit the stale/provenance header into BOTH JSON and markdown from one emitter; `check_perf_freshness.py` checks both.
6. **[META] Collective budget pool** for `molt_diff --jobs>1` (kills the parallel-contention FALSE-FAIL corrupting differential verdicts).
7. **[CLASS] Finalizer-aware lifetime on the ownership lattice** (kills the resurrection-at-a-distance UAF class) — part of Spine-4 Outcome 1 (#23).
8. **[CLASS] Repr-authority coherence audit** (`repr_audit()` before `lower_function_to_lir`: RawI64Safe⇒fits_inline_int47, FullDeopt⇒range-outside-inline) — part of int-unification (#20).
9. **[CLASS] Per-lane coverage audit** (`repr_lane_coverage_audit.py`, wired into ci_gate tier-1).
10. **[CLASS] Build-success prerequisite + stderr-aware conftest** (kills green-on-red atop a broken compiler).
11. **[CLASS] Panic-invariant assertion** — replace `isolates.rs` `let _ = catch_unwind(...)` with an asserting form + a shipped-profile=abort gate.

## Feeds the drift layer (#19, active-analysis prevent-drift instruments)
The six static fixes harden into CI drift-gates: (1) perf-gate-wiring-audit [DONE], (2) ratio-direction-scan, (3) dual-channel-freshness, (4) repr-authority-coherence + shift-carrier-coherence, (5) per-lane-coverage-audit, (6) build-success prerequisite + stderr-aware conftest. The runtime self-checks (leak rate-of-growth, DEALLOC-tail, finalizer-aware lifetime) gate under `MOLT_ASSERT_NO_LEAK` in differential CI. **Each instrument's contract: it must FAIL on a synthetic violation injected today** (proving it is not itself a proxy-measurement meta-bug) before it is trusted to keep a lane green. See [[coupled-analysis-infra]], [[numeric-loop-crux-stack]].
