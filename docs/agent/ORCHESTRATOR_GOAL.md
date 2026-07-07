# Molt pact-collab Orchestrator Goal (v2, 2026-07-05)

This is the durable, full contract behind the streamlined `/goal` condition
(the `/goal` text is capped at 4000 chars and points here). This file is
binding at the same level as the `/goal` args. If they conflict, this file
controls until reconciled. See also `CLAUDE.md`, `docs/agent/CLAUDE.full.md`,
`docs/agent/PROOF_QUEUE.md`, and the auto-memory index.

## Mission

Autonomously drive Molt pact-collab to the real 100-year green end state, acting
as the trusted orchestrator and senior engineer over a live swarm of ~3 Codex
agents (mid-career engineers) plus your own subagents. Start from the current
live tree in `C:\Users\adpen\OneDrive\Documents\molt` at HEAD, preserving all WIP.

## Done criterion (structural — not a smoke test or shim)

`collab/pact/pact_witness_kernel/field_solve.py` (import numpy + scipy) compiles
and runs through Molt's live WASM/browser path, produces `candidate_outputs.npz`,
and `check_parity.py candidate_outputs.npz` passes. Use the one-command witness
cycle and the named acceptance lane `tools/proof_queue.py pact-witness-acceptance`
(producing candidate_outputs.npz + passing check_parity.py) as the acceptance
proof — a bare `molt build field_solve.py` is build evidence, not acceptance.

## Non-negotiables (verbatim, binding)

- No Python reimplementation of NumPy/SciPy behavior.
- No host-CPython/Pyodide fallback.
- No fake `module__function` symbols from known_modules.
- `known_modules` stays import visibility; `direct_call_modules` stays Python
  symbol link authority.
- Native `callable_exports` become executable ABI dispatch — especially
  `scipy.ndimage.distance_transform_edt`.
- Admit upstream package source + extension artifacts only through package/native
  custody. Compile/link only reachable extension objects and symbols.
- Close missing C-API/ABI symbols as reusable primitives, not package-specific
  hacks; fail closed with a precise diagnostic when behavior is missing.
- Land ndarray/storage/dtype/shape/stride/buffer truth where the witness needs it.
- Delete or unify legacy/duplicate lanes touched by the work (no back-compat
  inside Molt internals).
- Keep docs, tests, sidecars, manifests, cache digests, and diagnostics aligned
  with the new authority.
- Performance is correctness: claimed support must be deterministic, portable,
  small, fast-start, and faster than CPython on the claimed target, with honest
  evidence. Decide+execute like Chris Lattner; never end a turn asking the
  operator to pick between fixes (only ask on user-owned calls: subset/API
  semantics, irreversible outward actions, scope).

## Current aperture (top program)

Witness-closure top program: the `scipy.ndimage` witness operation closure in
`field_solve.py`, followed end-to-end through import graph → package admission →
native artifact custody → callable export lowering → backend WASM import tables →
browser embed manifest → runtime ABI → ndarray storage → parity oracle.

Recent landings to build on (do not redo):
- `2bcd613db` typed strided buffer authority
- `aff08f405` ndarray buffer lease custody
- `3076bb67b` numpy-exec CPython C-API primitives (silent-failure tracing)
- `36d4ae5a2` Pact ndimage callable import forms
- `7f35011ba` one-command witness cycle with compact verdicts

Next known gap: `PyDescr_NewGetSet` / `PyDescr_NewMember` + `tp_getset` (see the
`numpy-exec-pytype-ready-closure` memory); make ndimage ABI dispatch executable.

## Orchestration model

- You are orchestrator + senior reviewer. Codex agents (~3, adjustable) execute
  delegated lanes and commit as `adpena`; your subagents architect, verify, and
  take over lanes where Codex churns or stumbles. See what each is on before
  feeding; prefer non-overlapping lanes; never feed a hot lane the swarm owns.
- Delegate ndimage ABI + ndarray depth to Codex; keep witness-closure integration,
  correctness review, and the parity oracle as your own authority.
- Regularly check `origin/main` `collab/pact` for a new pact-collab
  branch/worktree/PR or correspondence from the pact team; treat its contents as
  binding, highest-emphasis operator requirements. Follow the PR trail.
  `origin/main` is the final source of truth for pact-collab.
- Integrate via the worktree pattern (branch off `origin/main`, cherry-pick in a
  clean-index window, ff-push `integ:main`) — the shared checkout is rarely clean
  during swarm work. No force-push, `reset --hard`, checkout-over-WIP, or
  destructive git in the shared checkout (`git_guard` + `refs/wip-guard` net).
  Commit with an explicit pathspec so you don't sweep partner WIP. Push over SSH.

## Integration custody — YOU own drift, signal-loss, orphan, trample prevention (binding)

A first-class standing deliverable, not a background chore. The swarm lands PRs
fast; `origin/main` moves under you every few minutes. Keep the system coherent
while it moves:

- **Drift prevention — SWEEP PROACTIVELY, don't wait for a surprise.** Drift
  checking is a standing reflex, not a reaction. Cultivate the instinct: the
  swarm lands PRs continuously, so treat `origin/main` as moving under you at all
  times and *go look* before you assume anything is current. Instrument:
  `python tools/tree_drift_check.py --witness --fetch` gives a one-line
  fail-closed verdict (exit non-zero) on whether the current tree is
  stale/masking vs `origin/main`, per-file for the witness-frontier set. Run the
  DRIFT SWEEP:
  - **At the START of every arc** — `git fetch origin`; scan
    `git log --oneline <last-known>..origin/main`; for each active/assigned lane
    diff its target against current `origin/main`; re-sync the board's
    frontier/lane lines to whatever just landed.
  - **Before ASSIGNING or accepting any lane** — a task naming a branch is valid
    only if that branch still exists AND is unlanded: verify `git rev-parse
    origin/<b>` + `git merge-base --is-ancestor origin/<b> origin/main`. If the
    branch is deleted or the logic already sits on main, that's a merged lane —
    DROP it and say why (re-landing it is trampling).
  - **Before every LAND** — re-fetch, confirm fast-forward, and confirm no
    parallel PR superseded your work by content (same-logic under a new hash).
  - **On a REGULAR cadence during long work** — periodically re-fetch so a
    fast-moving swarm landing can't silently invalidate an in-flight assumption,
    a stale frontier note, or a diagnosis you're mid-way through.
  Any diagnosis or file you read from the stale shared checkout is suspect until
  re-verified against a clean `origin/main` worktree — the shared checkout lags
  main and yields false regressions and phantom frontiers. When a sweep finds
  drift, fix it in the same arc: re-sync the board, drop/refile the affected
  lane, and note what moved. A stale board is itself drift you own.
- **Signal-loss prevention.** Every unlanded piece lives in two durable places
  before you touch anything: a committed worktree branch AND an `origin` ref
  (`wip/*`). Local-only worktree commits are one prune from loss — push first.
  The board and coordination notes live in-repo, never only in an ephemeral
  session scratchpad.
- **Orphan prevention.** No branch/worktree with unique commits is pruned until
  its signal is landed on `origin/main` or banked to `origin/wip/*`. Track the
  reconcile state of every `wip/*` ref; an unreconciled ref no lane owns is an
  orphan to land or explicitly retire with a note.
- **Trample prevention.** Cherry-pick LOGIC, never merge stale-base branches
  (their tree-diff reverts upstream's parallel evolution). Never land docs/code
  from the stale shared checkout — re-apply onto a clean `origin/main` worktree.
  Commit by exact pathspec. Never touch another lane's dirty files. Enforce board
  lane exclusivity + stand-down orders under operator-delegated authority.
- **Cherry-pick then prune (the custody loop).** Per banked `wip/*` ref and live
  worktree: (1) isolate real per-commit signal vs current main, skipping what
  already landed by content (same-hash or same-logic — check, don't assume);
  (2) reconcile that logic onto a fresh `origin/main` worktree; (3) land via
  ff-push after a queue-owned proof; (4) only then prune. Bulk worktree pruning
  happens in a swarm-quiescent window (build procs 0), ref-by-ref, never before
  the signal is proven banked or landed.

## Process / resource custody (binding, hard-won)

- APDataStore = `D:` (exFAT, ~2TB) is THE build volume. Route fresh DX/proof
  builds through RunContext (`tools/run_context_env.py --prefer-external-artifacts
  --dx`, `tools/throughput_env.sh`, `tools/dev.py`, or the proof queue) so
  build/cache/temp roots resolve to `D:\Molt` and `MOLT_TARGET_ROOT` resolves
  to `D:\Molt\target-root`. exFAT has no hard-links: the backend cache owns the
  lock+rename/copy fallback — a "Failed to
  publish backend cache output" under `D:\Molt` is a DX defect to diagnose through
  the cache authority, never a reason to reroute to `E:` or hand-copy. Treat
  `E:\Molt` / `E:\molt-target` as legacy evidence only; preserve inherited
  legacy roots only with `MOLT_PRESERVE_LEGACY_ARTIFACT_ROOTS=1` or an explicit
  `MOLT_EXTERNAL_ARTIFACT_ROOTS` override. The daily `MoltSSDJanitor` keeps
  `D:` clean.
- HARD build cap: ≤2-3 build agents at once (rustc/cargo are NOT RSS-guarded; 5
  concurrent OOM'd a 32GB host at 97GB and got Codex killed). When builds stall,
  STOP feeding and let in-flight drain — don't cram. Use fast feedback loops
  (`cargo test -p <crate>` via the queue cargo lane, differential E2E,
  `MOLT_WASM_TRACE`), not 30-min cold E2E builds, as the inner loop.
- All contention-heavy work (Cargo, WASM/browser proofs, benches, conformance,
  stress) goes through `tools/proof_queue.py` with
  `--reason/--resource-family/--contention-key/--scope`; cargo via the
  queue-native cargo lane; long work via queue-owned `--detach`. Run
  `proof_queue.py status` before queueing; cite run IDs/log paths as evidence;
  `diagnose RUN_ID` before manual log archaeology.
- NEVER kill Codex/Claude/control-plane/parent/watcher/host processes; cleanup
  targets only live-proved Molt-owned build/test/bench/daemon/runtime/guard PIDs
  with an incident record. Never send Ctrl-C/SIGINT/ESC/interrupt bytes as process
  control (crashes the Codex exec backend, `code=3221225786`). The instant the
  operator reports host/Codex distress: STOP all agents, PAUSE feeding, diagnose
  from incident records before resuming. Crash recovery constrains fanout, not
  ambition: one active structural arc, one bounded proof lane, no retry storms.

## Proof (smallest high-signal lanes that prove the real contract)

1. structural tests for import/link authority + callable export lowering,
2. extension/native custody tests for reachable artifacts + C-API symbol boards,
3. browser/WASM native callable ABI tests,
4. real pact `candidate_outputs.npz` generation through Molt WASM/browser
   (`pact-witness-acceptance` lane),
5. `check_parity.py candidate_outputs.npz` PASS.

Independently verify every deliverable (including subagents' and Codex's): re-run
the gate, read the test body, prove it fails on a synthetic violation. A PASS is a
hypothesis until reproduced. Verify against a clean origin worktree — shared-
checkout HEAD stays at the session base and yields false regressions.

## Final response must include

- exact files changed,
- exact commands run + results (queue run IDs / log paths),
- whether `candidate_outputs.npz` was produced by Molt WASM/browser,
- remaining blockers only if truly external/impossible in this environment,
- senior-engineer review of correctness, performance, compatibility, fidelity.

## Standing parallel tracks (keep alive as slots allow, subordinate to witness)

- Harden/polish DX + queue + throughput as you go (recursive adversarial review).
- Keep the 100-year outcome portfolio moving where it doesn't collide with the
  witness lane or hot swarm lanes: perf dominance > CPython, scalar-repr
  canonicalization, memory-safety floor, CPython `>=3.12` parity,
  `docs/design/foundation/` 54-67. Cash every instrumental gate/tool into a
  measured outcome.
