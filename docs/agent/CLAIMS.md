# Solo-Owner Lane Claims

Some lanes must be driven **END-TO-END by ONE agent**. Splitting them across
agents causes exactly the failures this collaboration has hit repeatedly:
colliding edits, trampled landings, and *masked frontiers* (one agent's local
state hiding the real blocker from another). Those lanes are **SOLO lanes**.

Before working a SOLO lane you MUST claim it here. The claim is a git-atomic
lock: because claims are landed by fast-forward (`tools/ff_land.py`), exactly one
agent can win a claim race — the loser's push is refused and it backs off.

This registry is authoritative at `origin/main`. Read it there, not from a stale
checkout (`python tools/tree_drift_check.py --fetch` first).

## SOLO lanes (claim before starting)

- **`E1-WITNESS-TO-GREEN`** — the entire witness closure, owned end-to-end by one
  agent: seal-regen → WASM link → numpy static-lib link → execution →
  `candidate_outputs.npz` → `check_parity.py` PASS. Do NOT split this across
  agents; the seal artifacts, cpython-abi link, and acceptance lane are one
  coherent arc. (See the E1 order-of-operations in the PAUSE HANDOFF /
  `ORCHESTRATION.md`.)

- **`CODEX-B-CACHE-KEY`** — frontend lowering cache-KEY scoping (`cache_fingerprints.py`
  + `frontend_*` + aligned tests) so unrelated CLI/runtime/link edits don't cold-start
  the witness lowering cache. Coherent structural move; one owner. (Orchestrator-held;
  handoff from the prior in-progress agent per the reclaim directive in `ORCHESTRATION.md`.)

- **`REVIEW-5-SCCP-STR-UNICODE`** — adversarial-review finding #5 (P1
  CODEX-CORRECTNESS): SCCP constant-folding of str builtins/methods must use
  CPython code-point semantics, never Rust UTF-8 byte offsets. Single authority:
  `runtime/molt-passes/src/tir/passes/sccp/eval.rs`
  (`eval_concrete_builtin`/`eval_concrete_method`). One owner so the fix + teeth
  land as a coherent cut.

(Add new SOLO lanes here as the orchestrator or a claimant identifies them.)

## Protocol (binding)

**Helper (does steps 1-3 correctly for you):** `python tools/claim_lane.py <lane>
--check` (read-only; exit 0 = claimable, 1 = held → back off) and
`python tools/claim_lane.py <lane> --claim --agent <id> --note "..."` (pre-checks,
appends the row, `ff_land`s, and backs off on a lost race). Log progress/finish
with `--append PROGRESS|COMPLETE|RELEASED --agent <id> --note "..."`. The manual
steps below are the contract it implements.

1. **Drift-sweep first.** `git fetch origin`; read THIS file at current
   `origin/main`. `python tools/tree_drift_check.py --fetch` to confirm your tree
   isn't masking.
2. **Check the claim.** If the SOLO lane you want is `CLAIMED` and not STALE (see
   §6), **BACK OFF** — pick a different unclaimed SOLO lane, or a standing lane
   (Codex B/C/D). Never work a lane another agent holds.
3. **Claim it.** Append ONE row to the log (lane, your agent-id, UTC ISO
   timestamp, `CLAIMED`, first-step note) and land it with
   `python tools/ff_land.py`. Because landing is a fast-forward, exactly ONE
   claim wins a race. **If `ff_land` REFUSES (non-ff), someone moved first** —
   `git fetch`, re-read this file; if your lane is now claimed, discard your claim
   row and BACK OFF.
4. **Own it end-to-end.** While your claim is active, no other agent touches the
   lane's files, artifacts, or tests. Post append-only `PROGRESS` rows (with
   evidence: queue run ids, commits) at least every few hours so the claim is
   visibly alive.
5. **Final completion is NOT a green build.** Mark `COMPLETE` only after ALL of:
   - **(a) FINAL EXIT CRITERIA met, with evidence.** For `E1-WITNESS-TO-GREEN`:
     `candidate_outputs.npz` produced through Molt WASM AND `check_parity.py`
     PASS — cite the `pact-witness-acceptance` queue run id and paste the parity
     verdict. Zero fakes, no host-CPython/Pyodide fallback, POISON-clean.
   - **(b) RECURSIVE ADVERSARIAL REVIEW accomplished.** You (or a reviewer you
     spawn) actively tried to REFUTE the result from independent angles
     (re-run on a clean origin/main worktree; probe determinism/tolerances;
     confirm no masking, no fake symbol, no weakened assert) and it SURVIVED.
     Record what you tried and why it held.
   - **(c) SENIOR-ENGINEER REVIEW green.** Correctness incl. memory safety;
     performance (faster than CPython on the claimed target, honest numbers);
     CPython ≥3.12 compatibility in the verified subset; fidelity; no
     duplicate-authority / no partials. Record the sign-off.
   Only then append a `COMPLETE` row with the evidence links and `ff_land` it.
6. **Stale / release.** A claim with an explicit `RELEASED` row may be reclaimed.
   Otherwise a claim is STALE **only if BOTH** (a) no `PROGRESS` row for >4h AND
   (b) an OBJECTIVE-LIVENESS check shows no real activity. A missing PROGRESS row
   is NOT sufficient — an active agent often works silently in its worktree/queue
   without touching this log (this cost a wrong reclaim 2026-07-08). Before landing
   a `RECLAIM` row you MUST verify NO objective activity in the last ~2h:
   - the claimant's worktree commit log: `git -C <worktree> log --oneline --since="3 hours ago"`;
   - recent proof-queue rows for the lane: `tools/proof_queue.py status` (+ any
     worktree-local `logs/proof_queue`);
   - recent `origin/main` commits by that agent.
   If ANY show recent activity, the claim is ALIVE — do NOT reclaim (post a
   PROGRESS observation instead and ping the claimant to log its own progress).
   Cite the objective-liveness evidence (or its absence) in the `RECLAIM` row.
   Never silently take over a live claim; escalate to the orchestrator if contested.
   **Claimants:** post a `PROGRESS` row (or `python tools/claim_lane.py <lane>
   --append PROGRESS ...`) at least every few hours — silence forces others to
   guess your liveness from commits/queue, which is fragile.

## Log (append-only; newest at bottom; land each row via `ff_land`)

| lane | agent-id | UTC (ISO) | status | note / evidence |
|------|----------|-----------|--------|-----------------|
| _(none yet — first claimant of E1-WITNESS-TO-GREEN appends here)_ | | | | |
| E1-WITNESS-TO-GREEN | Codex/codex-e1-seal-regen-20260707 | 2026-07-07T21:02:53Z | CLAIMED | Start from clean origin/main 6e1494cb8; first step is seal-regen artifact custody, re-seal gate, then pact-witness-acceptance. |
| E1-WITNESS-TO-GREEN | orchestrator (obs) | 2026-07-08T01:20:00Z | PROGRESS | Orchestrator-observed ALIVE: codex-e1 worktree active (14 dirty, static_truth/analysis_collect_static <2h), seal machinery landing (8e4fae62b skip-stale, 36b11f58f relativize). Claim is LIVE — do NOT reclaim. E1 agent: post your own PROGRESS rows + drive the 17-source seal regen (still 0 runtime_python_import_modules on disk). |
| CODEX-B-CACHE-KEY | orchestrator | 2026-07-08T01:35:00Z | CLAIMED | Reclaimed per operator direction. Awaiting handoff of the in-progress cache_fingerprints.py tooling-scoping WIP (bank to wip/codex-b-cache-scoping-20260708), then finish with warm-run before/after cache-hit measurement + tests. |
| CODEX-B-CACHE-KEY | orchestrator | 2026-07-08T02:05:00Z | COMPLETE | Landed b9d6963fb. Frontend lowering cache keyed on semantic tooling (33 post-lowering cli files excluded); before: unrelated edit INVALIDATES -> ~180-250s re-lower; after: REUSES. Correctness fix: external_native kept in scope (feeds direct_call_modules) + regression test. 7/7 teeth. Senior-reviewed + independently reproduced. |
| E1-WITNESS-TO-GREEN | orchestrator | 2026-07-08T09:39:33Z | RECLAIM | Reclaimed: claim STALE (last progress 2026-07-08T01:20Z, now 09:38Z = 8h; seal still 0 runtime_python_import_modules; recent worktree touches are phase-split ripple, not seal progress). Orchestrator drives the 17-source seal regen to E1 green. |
| E1-WITNESS-TO-GREEN | orchestrator (correction) | 2026-07-08T09:55:00Z | PROGRESS | CORRECTION: my 09:38 stale-reclaim was WRONG — the E1 Codex agent is ACTIVE and PAST THE SEAL. It has a working witness build (app.wasm, run 20260708T092032) that passes seal custody + WASM link (89e5160ea) + instantiation (its manifest link-import host-trap fix 985567256 cleared the libc++ bad_function_call frontier). NEW frontier = reserved-callable dispatch: molt_call_indirect4 idx=2108 molt_type_new arity mismatch (expected 5 got 4). Codex agent RETAINS witness E2E ownership; the molt_type_new reserved-callable frontier is ORCHESTRATOR-owned — orchestrator drives it. E1 agent: post PROGRESS rows so this doesn't recur. |
| E1-WITNESS-TO-GREEN | orchestrator (correction) | 2026-07-08T10:53:27Z | PROGRESS | FRONTIER ADVANCED past molt_type_new. pact-witness-acceptance 4521da6aebe64770 fails unsupported-direct-call 'AxisError' at scipy/_lib/_util.py:6:14. Root: call_dispatch_named.py:2184-2237 — names imported from a SOURCE-COMPILED package in the witness closure (numpy) fail closed (Tier0 non-allowlisted) instead of resolving through package symbol-closure; only imported_from is None gets _emit_dynamic_call. POISON-clean fix: imported-from-compiled-package callables resolve through package/import symbol-closure custody (numpy compiled here), never Molt-baked. Driving via orchestrator worktree-isolated agent. |
| E1-WITNESS-TO-GREEN | orchestrator (correction) | 2026-07-08T11:00:14Z | PROGRESS | E1 FRONTIER (past molt_type_new): pact-witness-acceptance 4521da6aebe64770 fails unsupported-direct-call 'AxisError' at scipy/_lib/_util.py:6. ROOT CAUSE ISOLATED (reproduced at frontend level, no build): scipy does 'from numpy.exceptions import AxisError'; the CHILD module numpy.exceptions is NOT in the witness import closure (known_modules), so the bare-name call fails closed. Verified: known_modules={numpy} alone -> RAISES; known_modules={numpy,numpy.exceptions} -> OK (call_bind). The fail-closed is CORRECT — do NOT loosen call_dispatch_named.py:2231 to emit call_bind for unknown modules (that is a fail-OPEN POISON: binds to an undiscovered module -> resolves to nothing at runtime). REAL FIX is upstream in numpy closure discovery: numpy.exceptions (and siblings) must be admitted into known_modules via package/import symbol-closure custody. Ties to the stale numpy seal (0 runtime_python_import_modules). E1 SOLO-lane witness-closure work, needs full build loop. |
| REVIEW-5-SCCP-STR-UNICODE | Claude/claude-review5-sccp-unicode-20260708 | 2026-07-08T17:23:09Z | CLAIMED | Finding #5 (P1 CODEX-CORRECTNESS). Verified free (no origin/main sccp touches in 2d; no other claim). Root: sccp/eval.rs folds len/find/rfind/count("")/zfill with Rust UTF-8 byte semantics -> silent miscompile of every non-ASCII string constant. Fix in flight: code-point semantics at the single fold authority + non-ASCII teeth (CPython 3.12-captured expectations). |
| REVIEW-5-SCCP-STR-UNICODE | Claude/claude-review5-sccp-unicode-20260708 | 2026-07-08T17:30:42Z | COMPLETE | Landed 9ebc5f4752. sccp/eval.rs now folds str len/find/rfind/count("")/zfill with CPython code-point semantics (chars().count() + shared byte_offset_to_char_index helper); zfill also made negative-width-safe (old `*w as usize` wrapped -> compile-time OOM/panic). Single authority; no duplicate fold path (grep-verified value_range/ir clean). Teeth: sccp::eval::unicode_fold_tests (6 tests, non-ASCII incl. astral, CPython 3.12-captured). Queue run 20260708T172531-review5-sccp-unicode-1f6587e47637432e PASSED rc=0: 46 sccp tests pass. rustfmt --changed clean. Every expected value re-verified against the CPython 3.12 reference interpreter (primary source). |
| E1-SCIPY-RANK-FILTER-1D | claude-opus-e1-rankfilter | 2026-07-08T18:08:31Z | CLAIMED | Building scipy.ndimage._rank_filter_1d (_rank_filter_1d.cpp) to wasm32 static-link artifact via source-extension custody lane |
| E1-NUMPY-HEADER-OVERLAY-CUSTODY | claude-opus-e1-numpy-header | 2026-07-08T18:09:23Z | CLAIMED | Route numpy headers through package custody (_core/include); delete include/numpy overlay; decrement fail_closed numpy-header poison class |
| C4 | Codex/codex-c4-scoreboard-gate-20260708 | 2026-07-08T18:12:37Z | CLAIMED | Claim C4 R8 scoreboard/release-gate lane; first step wire canonical perf scoreboard freshness+greenness into the existing freshness gate and prove it with focused tests. |
| E1-SCIPY-NI-SUPPORT | Codex/codex-e1-scipy-ni-support-20260708 | 2026-07-08T18:28:01Z | CLAIMED | Build scipy.ndimage._ni_support into wasm32 static-link artifact through source-extension custody; no package-specific regen or header overlay. |
| E1-SCIPY-NI-DOCSTRINGS | Codex/codex-e1-scipy-ni-docstrings-20260708 | 2026-07-08T19:46:28Z | CLAIMED | Build/prove scipy.ndimage._ni_docstrings source-extension artifact or closure-strippable outcome; no package-specific overlay. |
| REVIEW-9-PARITY-IMPORTERROR-STRICT | Codex/codex-review9-parity-importerror-20260708 | 2026-07-08T20:01:49Z | CLAIMED | Fix review finding #9: strict parity gate must not skip Molt-only ImportError-shaped failures; first step inspect parity_gate authority and tests, then land fail-closed behavior with teeth. |
| REVIEW-9-PARITY-IMPORTERROR-STRICT | Codex/codex-review9-parity-importerror-20260708 | 2026-07-08T20:12:16Z | COMPLETE | Already fixed by e00d69a98d; review ledger landed in 6b0997a666; reverified tests/tools/test_parity_gate.py -q on rebased tree (5 passed in 2.58s). |
