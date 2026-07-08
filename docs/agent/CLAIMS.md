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
6. **Stale / release.** A claim with no `PROGRESS` row for >4h, or an explicit
   `RELEASED` row, may be RECLAIMED by another agent landing a `RECLAIM` row that
   CITES the staleness evidence (last progress timestamp). Never silently take
   over a live claim; escalate to the orchestrator if ownership is contested.

## Log (append-only; newest at bottom; land each row via `ff_land`)

| lane | agent-id | UTC (ISO) | status | note / evidence |
|------|----------|-----------|--------|-----------------|
| _(none yet — first claimant of E1-WITNESS-TO-GREEN appends here)_ | | | | |
| E1-WITNESS-TO-GREEN | Codex/codex-e1-seal-regen-20260707 | 2026-07-07T21:02:53Z | CLAIMED | Start from clean origin/main 6e1494cb8; first step is seal-regen artifact custody, re-seal gate, then pact-witness-acceptance. |
| E1-WITNESS-TO-GREEN | orchestrator (obs) | 2026-07-08T01:20:00Z | PROGRESS | Orchestrator-observed ALIVE: codex-e1 worktree active (14 dirty, static_truth/analysis_collect_static <2h), seal machinery landing (8e4fae62b skip-stale, 36b11f58f relativize). Claim is LIVE — do NOT reclaim. E1 agent: post your own PROGRESS rows + drive the 17-source seal regen (still 0 runtime_python_import_modules on disk). |
| CODEX-B-CACHE-KEY | orchestrator | 2026-07-08T01:35:00Z | CLAIMED | Reclaimed per operator direction. Awaiting handoff of the in-progress cache_fingerprints.py tooling-scoping WIP (bank to wip/codex-b-cache-scoping-20260708), then finish with warm-run before/after cache-hit measurement + tests. |
| CODEX-B-CACHE-KEY | orchestrator | 2026-07-08T02:05:00Z | COMPLETE | Landed b9d6963fb. Frontend lowering cache keyed on semantic tooling (33 post-lowering cli files excluded); before: unrelated edit INVALIDATES -> ~180-250s re-lower; after: REUSES. Correctness fix: external_native kept in scope (feeds direct_call_modules) + regression test. 7/7 teeth. Senior-reviewed + independently reproduced. |
| E1-WITNESS-TO-GREEN | orchestrator | 2026-07-08T09:39:33Z | RECLAIM | Reclaimed: claim STALE (last progress 2026-07-08T01:20Z, now 09:38Z = 8h; seal still 0 runtime_python_import_modules; recent worktree touches are phase-split ripple, not seal progress). Orchestrator drives the 17-source seal regen to E1 green. |
