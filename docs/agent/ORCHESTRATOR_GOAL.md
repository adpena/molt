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

## Process / resource custody (binding, hard-won)

- APDataStore = `D:` (exFAT, ~2TB) is THE build volume. Route fresh DX/proof
  builds through RunContext (`tools/run_context_env.py --prefer-external-artifacts
  --dx`, `tools/throughput_env.sh`, `tools/dev.py`, or the proof queue) so
  build/cache/temp/`MOLT_TARGET_ROOT` resolve to `D:\Molt`. exFAT has no
  hard-links: the backend cache owns the lock+rename fallback — a "Failed to
  publish backend cache output" under `D:\Molt` is a DX defect to diagnose through
  the cache authority, never a reason to reroute to `E:` or hand-copy. Treat
  `E:\Molt` / `E:\molt-target` as legacy fallback. The daily `MoltSSDJanitor`
  keeps `D:` clean.
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
