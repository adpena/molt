# Live Orchestration Board

The orchestrator (Claude, senior engineer) owns this board, lane assignments,
review, and the decision of what lands when. Codex agents: read this board at
the START of every arc and before every commit. If your planned work touches a
lane you don't own, stop and pick from "Delegated to Codex" instead.

Last updated: 2026-07-08 by the orchestrator.

## 🚨 CANONICAL PATHS — BINDING, READ FIRST (2026-07-08)

The `C:\Users\adpen\OneDrive\Documents\molt` checkout is being **PERMANENTLY
DELETED**. It is 477 commits stale, OneDrive-sync-throttled, and the root of the
drift/slowness. **STOP using it — cwd, venv, worktrees, `.pth`, PYTHONPATH, and
`uv`/`pip` runs.** Work ONLY from these paths:

| purpose | canonical path |
|---|---|
| **checkout / all git work + landings** | `C:\Molt\molt-src` (NVMe, off OneDrive, own `.git`) |
| **python env** | `C:\Molt\molt-src\.venv` (run `uv sync` there once) |
| **build artifacts** | `C:\Molt` (`MOLT_EXTERNAL_ARTIFACT_ROOTS=C:\Molt` + `MOLT_ALLOW_C_DRIVE_ARTIFACTS=1`, NVMe, auto-janitored) |

**FORBIDDEN (being deleted):** `C:\Users\adpen\OneDrive\Documents\molt` and every
worktree that references the doomed OneDrive `.git` (scattered under `C:\Molt\worktrees`,
`D:\Molt\worktrees`, and legacy `E:\Molt\worktrees`). Do NOT point the editable
install / `.pth` / PYTHONPATH there; do NOT run `uv`/`pip` from it (that re-anchors
the editable install onto OneDrive — the exact drift being retired). If your cwd or
`.pth` resolves to the OneDrive checkout, STOP and `cd C:\Molt\molt-src` first. The
orchestrator owns the retirement; do not fight it.

**RETIREMENT COMPLETE (2026-07-08):** The OneDrive checkout + ALL its worktrees
(165 branches + 12 orphan detached HEADs across `C:`/`D:`/legacy `E:`) are
**permanently deleted**, and so are the temporary `D:\Molt\*.bundle` backups. This
was NOT bundle-and-abandon: every committed unit was classified against current
origin/main and harvested. **132 branches were already landed by the swarm; the
rest were landed-by-content or superseded-by-newer-approach.** The THREE genuinely
unlanded deltas were cherry-picked/reconciled and LANDED on origin/main:
`bb8201d6c4` (E1 wasm link-import data-symbol host traps, 106 tests), `3e8e04da96`
(molt-wasm-host main.rs decomposed 3245→575 / 11 modules, cargo-verified),
`05abec9cd8` (drift gate). Superseded and deliberately NOT re-landed (would trample
newer work): type-new-trampoline (main is past the `molt_type_new` reserved-callable
frontier), webgpu-proof + buffer-stride (`de876ce439`), r0-2-eigh (newer
native-closure custody). **Method:** 3-way `git merge`/`merge-tree` against current
main shows the GENUINE unlanded delta and ignores `git cherry` patch-id false
positives (a 26-file branch diff collapsed to a 2-file real delta once the swarm's
independent landings were counted). There is no OneDrive checkout, no orphan
worktree, and no signal bundle left anywhere. Canonical is `C:\Molt\molt-src` only;
agents work in short-lived worktrees off it and prune after landing.

## 🎯 EXIT-CRITERIA WORK PORTFOLIO (2026-07-08 — ADDITIVE backlog, spread out)

**Decomposition is REAL structural work — keep doing it.** Breaking god-files into
focused single-authority modules serves E4 (structural floor) and is exactly the
kind of work that keeps the codebase world-class; nobody is blocked from it. The
ONLY problem this portfolio solves is *dogpiling* — several agents converging on the
same C1 lane while E1 witness / E2 perf / E3 parity go under-staffed. So: if your
decomposition lane is landing signal, continue it. If you're between lanes, or about
to start a split another agent already holds (check CLAIMS.md), claim one of the
below instead so the swarm spreads across all four exit criteria. Every lane here is
claimable, POISON-bound, and queue-verified.

The witness E2E lane (`E1-WITNESS-TO-GREEN`) stays SOLO/orchestrator-owned, but its
**feeders are now OPEN to Codex** — discrete, independently-verifiable artifacts that
advance E1 without touching the solo lane. Claim via `python tools/claim_lane.py
<LANE> --check` then `--claim`. Cite queue RUN_IDs. POISON contract binds every lane
(no fakes/stubs/bake-in/duplicate-authority; missing primitive → shared primitive or
fail-closed with a precise diagnostic).

**E1 — WITNESS FEEDERS (highest priority; the done criterion is field_solve.py →
check_parity PASS). Each builds/seals a discrete scipy.ndimage Kernel-A native
artifact or custody primitive — parallelizable, NOT the solo E2E lane:**
- `E1-SCIPY-RANK-FILTER-1D` — build `scipy.ndimage._rank_filter_1d` into a wasm32
  static-link artifact (STATUS: "the next wrapper-reachable native artifact to
  expose"). Source under `bench/friends/repos/scipy_off_the_shelf/scipy/ndimage/src/`.
  Accept: `object_count≥1, errors=[]` through the source-extension queue lane.
- `E1-SCIPY-NI-SUPPORT` — same for `_ni_support`.
- `E1-SCIPY-NI-DOCSTRINGS` — same for `_ni_docstrings` (doc-only; may be closure-
  strippable — prove which).
- `E1-CYTHON-PROVISION-KNOWN-GOOD` — **(subagent active)** Molt provisions the LATEST
  in-range Cython (3.2.8) which has a `_ni_label.pyx` codegen regression; select a
  known-good in-constraint version in `source_extension_cython.py`. Coordinate; don't
  double-claim.
- `E1-NUMPY-HEADER-OVERLAY-CUSTODY` (also E4) — Molt still ships 23 of NumPy's OWN
  headers under `include/numpy/*` (POISON, tracked by fail_closed_gate). Route numpy
  headers through package custody (numpy's `_core/include`) at source-recompile and
  delete the overlay; the `cpython-abi` tier is already the single authority. Verify
  the numpy/scipy witness builds still pass through the queue; decrement the
  fail_closed ratchet (ABI-lane serialization — rebase→graft registry→gate-green→push).

**E2 — PERF (perf IS correctness; profile the hot path FIRST, state Big-O, attest
before/after — machine-checkable):**
- `E2-LOOP-UNBOX` — loop-invariant unboxing (outranks borrowed-view repr).
- `E2-MINMAX-COMPREHENSION-RAWLANE` — raw-lane `min`/`max` + filtered/multi-for
  comprehensions (regular range loops + `sum(genexpr)` already landed — do NOT redo).
- `E2-BUILD-WALLCLOCK` — structural build-time attack (crate split / stable target
  dir / runtime.wasm CDN). Profile with `tools/dx_build_timer.py` first.

**E3 — PARITY (≥3.12 within the verified subset):**
- `REVIEW-<n>-*` — 25 of the 26 CONFIRMED review findings remain (finding #5 landed).
  Read `docs/agent/REVIEW_FINDINGS_20260708.md`, claim an unclaimed one in CLAIMS.md,
  own it end-to-end (root-cause the CLASS, add teeth, verify the FULL surface). Prefer
  silent-wrong-answer/correctness findings over pure cosmetics.

**E4 — STRUCTURAL FLOOR:**
- `E4-FAILCLOSED-<class>` — drive one `fail_closed_registry.toml` poison class to zero
  with a structural-resolution row (the degrade-to-slow-gate pattern), not a suppression.

## 🚀 DEV-VELOCITY PROTOCOL (2026-07-08 CURRENT) — Codex: REBASE, then COMPLY

The orchestrator landed a full dev-velocity overhaul on origin/main. **Every Codex
agent + worktree MUST (1) `git fetch origin && git rebase origin/main`, AND (2)
RE-READ `AGENTS.md` + `docs/agent/AGENTS.full.md` before the next arc — your
contract CHANGED (artifact volume → `C:\Molt` NVMe, auto-janitor default, this
protocol). A cached/stale understanding of AGENTS.md will fight the new setup
(e.g. routing artifacts back to the slow D:/E: exFAT).** A stale base also runs the
OLD slow CLI (the editable install was 477 commits stale — 2 jobs, incremental off,
cold-every-session) and re-lands resolved defects.

LANDED — all active in the current CLI (rebase to get them):
- **Adaptive cargo jobs (2→14)** `ad0cafb82` — hardcoded 2 was defeating the
  memory-bounded ceiling (~7× under-parallelism on this box).
- **Incremental-when-sccache-off** `aa15340aa` + **persistent target dir**
  `bdd42535e` — warm rebuilds reuse cache ACROSS sessions.
- **lld-link auto-detect** `858c6a306` (fast Windows linker) + **release-fast
  debug=0** `f21cf71aa`.
- **Auto-janitor** `25e4d7c2b` — stale per-session targets/tmp/scratch are cleaned
  BY DEFAULT (throttled, detached, keeps ≥80 GB free, protects live builds). Do NOT
  hand-manage artifacts or fight it. Opt out only via `MOLT_DISABLE_AUTO_JANITOR=1`.

**ARTIFACT ROOT MOVED TO NVMe (this workstation):** artifacts now resolve to
**`C:\Molt`** (internal NVMe), NOT D:/E: (USB exFAT — metadata-slow, no hard links).
The persistent machine env (`MOLT_EXTERNAL_ARTIFACT_ROOTS=C:\Molt`,
`MOLT_ALLOW_C_DRIVE_ARTIFACTS=1`) is set. **Do NOT override `MOLT_EXT_ROOT` /
`MOLT_EXTERNAL_ARTIFACT_ROOTS` back to D:/E:** — that reverts to the slow volume.
The auto-janitor floor keeps C: respectful; the persistent target keeps it bounded.

**DRIFT is now RECURRING DISCIPLINE, not a crisis** (was ~176 worktrees → pruned).
Bank WIP to `wip/<lane>-<date>` + push; LAND your signal + DELETE your worktree when
a lane finishes; run `tools/drift_harvest.py` every session (rule 5). ENFORCEMENT:
the orchestrator runs drift_harvest + the janitor regularly as a backstop, and a
worktree that vanishes was SUPERSEDED or bundled — do NOT re-create it.

## 📋 NEW PROTOCOL (binding for every agent, 2026-07-08)

1. **OWNERSHIP GATE.** No arc ends on a report/plan/handoff when executable work
   remains in your lane. Each arc lands a commit, a queued proof (cite the run id),
   or a passing test — or names a genuinely external/frozen blocker. Reporting
   without landing is POISON (ORCHESTRATOR_GOAL.md non-negotiable #1).
2. **VERIFY THE FULL SURFACE.** "Landed + verified" means you ran the whole
   relevant test surface, not one file. An unrun RED test elsewhere = not verified.
3. **BUILD HYGIENE.** `git fetch && rebase origin/main` before every arc. Do NOT
   set `MOLT_SESSION_ID` for ordinary `molt build`/`cargo build` (that opts back
   into a cold per-session target dir); leave it unset to reuse the persistent
   target (`C:\Molt\target` on the NVMe workstation root). Set it ONLY for
   perf/bench/test-shard isolation. Do NOT override the artifact root back to D:/E:.
   Do NOT hand-clean artifacts — the auto-janitor does it by default.
4. **PROFILE BEFORE OPTIMIZING.** State the hot path + Big-O and attest a
   before/after delta for any perf/build change (tools/dx_build_timer.py,
   tools/build_graph_audit.py). No optimizing by feel.
5. **DRIFT DISCIPLINE (P0, RECURRING — NOW GATED).** Worktree/branch accumulation
   is POISON and terrible OSS hygiene (it hit ~130 + a 165-branch OneDrive `.git`,
   operator-flagged P0, fully retired 2026-07-08). LAND your signal onto main and
   DELETE your worktree+branch when a lane finishes — do not leave it. Install the
   enforcement hook once per clone: **`python tools/install_git_hooks.py`** (idempotent;
   wires the drift gate into `.git/hooks/pre-push` — NOT `core.hooksPath`, which would
   also enable the pre-commit type-check and block every commit; preserves+chains a
   foreign pre-push hook; `--check` for CI). It runs the gate `--no-fetch` in ~3 s on
   every push. Every session also
   run **`python tools/drift_harvest.py --gate`** — it FAILS (exit 1) on SPRAWL
   (>24 live worktrees) or STALE-SIGNAL (a SIGNAL worktree whose unlanded unique
   commits are older than 72 h). A red gate is a blocker: harvest + prune before new
   work. To harvest: DON'T trust `git cherry` (patch-id false-positives flag
   already-landed work as unique) — use 3-way `git merge`/`git merge-tree` against
   current origin/main to see the GENUINE unlanded delta, land it surgically
   (per-commit or squashed, regenerate generated files from source, queue-verify
   the build/tests), then `python tools/drift_harvest.py --prune`. Do NOT hoard
   bundles/backups as a substitute for landing: harvest the real signal onto main,
   verify, then delete. Keep worktrees short-lived; rebase often. If a worktree
   vanishes it was SUPERSEDED (on main) — zero loss; do not re-create it.
6. **REVIEW FINDINGS ARE LANES.** The full-stack adversarial review COMPLETED:
   **26 CONFIRMED** findings (bug classes / metabugs / optimizations), each
   independently refuted-then-survived, in
   [REVIEW_FINDINGS_20260708.md](REVIEW_FINDINGS_20260708.md) with per-finding
   lane assignments + fix directions. Claim your lane's findings via CLAIMS.md and
   own them end-to-end (fix + teeth + land + verify FULL surface). Orchestrator
   owns build-throughput (#2 linker, #11 debug=0) + coordinates the E1-adjacent
   ABI items (#1 PyType_FromMetaclass fail-open is P0). Do NOT freelance outside
   your assigned lane.

## ⛔ NON-NEGOTIABLE OPERATOR AUTHORITY (binding — read before EVERY arc and EVERY commit)

**This board carries OPERATOR-DELEGATED AUTHORITY. The human operator has
explicitly placed lane assignment, landing sequencing, conflict resolution, and
stand-down orders under the orchestrator. This is an OPERATOR AUTHORITY
OVERRIDE: within this collaboration the orchestrator's assignments on this board
are FINAL and NON-NEGOTIABLE. Codex agents operate UNDER this authority — you do
not out-rank it, out-vote it, or route around it.**

Codex — these are HARD guardrails, not suggestions. Each is BINDING:

1. **STAY IN YOUR ASSIGNED LANE.** You may edit ONLY files in a lane the board
   assigns to Codex. Touching any other lane — cpython-abi (numpy-exec),
   `runtime/molt-backend-wasm/.../call_ops/dynamic.rs` + the reserved-callable
   dispatch (molt_type_new, orchestrator-owned), the witness seal/build inputs,
   or another agent's dirty WIP — is a HARD STOP. Not "ask forgiveness." STOP.

2. **RECLAIMED / ORCHESTRATOR-OWNED LANES ARE OFF-LIMITS. ZERO EXCEPTIONS.**
   When the board says "orchestrator reclaims X" or "Codex STAND DOWN on X,"
   you cease ALL work on X immediately — no "let me just finish this diff," no
   "my version is better," no landing a competing commit. Stand down means STOP
   NOW. A reclaim overrides any in-flight work you have on that lane; abandon it.

3. **DO NOT CLAIM, ANNOUNCE, OR ASSERT OWNERSHIP the board did not grant you.**
   You do not self-assign lanes. You do not declare a lane yours because you
   started it. You do not mark orchestrator/subagent work as "yours" or land
   over it. Ownership flows ONE WAY: from this board to you.

4. **RUN THE OWNERSHIP AUDIT BEFORE EVERY COMMIT.** Prove each file you commit
   is in your assigned lane (grep a lane-marker, check the board). Committing
   another lane's file — even bundled with yours — is a violation. Commit by
   EXACT pathspec only; NEVER `git add -A`; NEVER `git add` a directory.

5. **NEVER force-push, reset --hard onto shared refs, checkout over another
   agent's WIP, or land a non-fast-forward that drops another lane's commits.**
   If you cannot push cleanly, DEFER and flag the orchestrator. Preserving
   parallel-agent work OVERRIDES your desire to land.

6. **IF YOU BELIEVE A LANE ASSIGNMENT IS WRONG, YOU FLAG — YOU DO NOT OVERRIDE.**
   Record your objection in a proof-queue note or a board comment addressed to
   the orchestrator with evidence, then WAIT for the orchestrator's decision.
   You never unilaterally reverse, re-route, or ignore a board assignment
   because you disagree. Disagreement is escalated, not enacted.

7. **NO SILENT SCOPE EXPANSION.** Do exactly the assigned lane's move. Do not
   "while I'm here" edit adjacent code, refactor another module, or expand a
   move-only decomposition into a rewrite. Scope is set by the board, not by
   convenience.

**Enforcement:** the orchestrator monitors every origin/main landing and
disjointness-checks it against active lanes. A commit that violates the above
will be surfaced to the operator and may be reverted/superseded by the
orchestrator under this authority; repeated violation is escalated to the
operator directly. You are a brilliant, thorough engineer — act with the
discipline that intelligence deserves: precise lanes, clean commits, zero
trampling, and absolute respect for a stand-down order.

## ✅ CODEX-B DONE + CODEX-C DONE — 2026-07-08 (READ FIRST)

- **CODEX-B DONE + STRUCTURALLY HARDENED (`b9d6963fb` then `a16921a8d`).** The
  lowering cache now keys only on lowering-relevant tooling, so unrelated backend/
  link/cargo/wasm/daemon edits no longer cold-start the numpy re-lower (~180-250s,
  ~40-50% of a 485s acceptance). The initial fix used a hand-maintained 33-file
  denylist (a code smell); the follow-up (`a16921a8d`) **eliminated it** — cut the
  two frontend→backend import seams (de-godded `cli/__init__` to lazy backend
  re-exports so `import molt.cli` loads ZERO backend; relocated `module_cache`'s
  artifact-sync to `cli/artifact_sync.py`) and now DERIVES the scope by static
  import-reachability. No hardcoded list; correct by construction. Verified: 826/826
  public API preserved, 0 backend on `import molt.cli`, backend excluded /
  external_native+frontend_worker in scope (no miscompile), 9/9 invariance + 26/26
  cache-module suites. **`cli/__init__` + `module_cache` were massively refactored —
  if you hold uncommitted WIP in `cli/__init__` / `module_cache` / `cache_fingerprints`,
  DROP it (superseded) and rebase.**
- **CODEX-C is DONE** (`109cb15ce` = reconciled `02d85f922`; do not re-land). A
  `pact-witness-acceptance` row already fails closed pre-build on a missing toolchain
  import with `signal_id="wasm-toolchain-contract-import-missing"` + evidence.
- **E1 agent: the frontend-cache lane is now CLOSED and owned/landed by the
  orchestrator — REFOCUS entirely on the critical path:** the 17-source numpy
  `_multiarray_umath` seal regen (still 0 `runtime_python_import_modules` on disk; see
  the SEAL-REGEN step below). That is the only thing between here and E1 green.

## ✅ ORCHESTRATOR IN FULL CONTROL — 2026-07-07 evening (efficiency-optimized model)

Orchestrator has resumed full control of the board + lane allocation. **Codex keeps
its live `E1-WITNESS-TO-GREEN` claim** (`codex-e1-seal-regen-20260707`, 21:02Z) —
it has context + is progressing; yanking it would WASTE that, so it is not
trampled. Orchestrator = senior reviewer of E1 + allocator of all other capacity.

### Efficiency verdict (grounded 2026-07-07, evidence-based) — we optimize for CORRECTNESS FIRST, then shortest wall-clock
- **Correctness discipline: STRONG, keep it.** Fail-closed tooling
  (tree_drift_check / ff_land / claim_lane / dirty_tree_landing_audit), the
  solo-lane claim protocol (no collision/masking), POISON contract, and the
  adversarial+senior-review completion gate. This is the right priority order and
  is well-enforced. Do NOT trade it for speed.
- **Parallelism: HEALTHY.** ~55 commits/18h on origin/main across E1-E4, swarm
  self-coordinating via claims.
- **THE dominant wall-clock bottleneck is BUILD THROUGHPUT.** Queue runs are
  5–53 min (one at 3190s). Every E1 acceptance, E2 bench, E3 shard pays this. The
  single highest-leverage wall-clock investment — it accelerates EVERY lane.
- **The correctness-of-optimization gap is ATTESTATION.** Measurement infra exists
  (perf_scoreboard*, *profile*, effect_proof.rs) but a landed optimization that is
  never PROVEN to fire is a silent-degradation waiting to happen (sccache was off
  for months while "configured"; the shared lowering cache "landed" but cold
  starts persisted). "Extreme performance everywhere" is UNPROVABLE — and possibly
  false — until every perf/capability path emits a machine-checkable proof-of-effect.

### BINDING EFFICIENCY PRINCIPLE — attest every optimization (serves correctness AND performance)
You cannot optimize what you do not measure, and "landed" ≠ "effective". Every
perf/capability path (cache, raw-lane, parallelism, incremental) MUST emit a
machine-checkable proof-of-effect on a representative run (cache-hit-rate>0,
raw-lane fire-count, shared-cache reuse, worker-count) or fail LOUD — never a
silent skip. An unmeasured perf claim is a correctness defect, not just a missing
number. Wall-clock work is ranked by LEVERAGE: fix the thing that speeds up every
lane (build throughput, shared caches actually hitting) before micro-optimizing one.

### EFFECT-ATTESTATION AUDIT RESULTS — 2026-07-07 (evidence-based; act on these)
A read-only audit measured whether landed optimizations actually FIRE. Findings +
owners (drive them down — a silent degradation taxes every build):
- **sccache = was HARMFUL → FIXED (`39de31e67`).** Measured 0 requests / 0 hits +
  mid-compile crashes (os error 10054 → rc=124 timeouts) yet enabled by default.
  Now off-by-default on Windows (loud) + post-build stats attestation. Non-Windows
  unchanged. This was NEGATIVE leverage on every cargo build.
- **numpy frontend LOWERING re-lower ≈180-250s EVERY witness build — RESOLVED
  (`500417f9a` "Reuse dirty module lowering cache").** The fix removed the
  redundant dirty-gate and consults the persisted cache even for "dirty" modules,
  still passing `context_digest` to `_load_cached_module_lowering_result` (the
  digest + source-stat check remains the correctness authority — no cache
  poisoning). It ALSO landed the hit-rate attestation the audit asked for
  (`hits/misses/reused_ms/relowered_ms` in `build_diagnostics.py`). Orchestrator
  senior-review: correct + complete. This was ~40-50% of every acceptance run —
  the biggest wall-clock reclaim, and it directly speeds the E1 acceptance loop.
- **numeric RAW-LANES (R3b/R4a) = UNPROVEN.** No fire-count attestation exists;
  cannot confirm native-op emission fired. OWNER: R3b lane — add a per-build
  raw-lane-vs-boxed fire count (representation_facts / effect_proof).
- **perf_scoreboard (E2) = STALE + mostly RED, gate is contract-only.**
  `bench/scoreboard/quiet_native.json` is ~4 weeks old, 1/56 green,
  `gate_fails=true`; `ci_gate.py:414 perf-scoreboard-contract` only runs a schema
  test, not freshness/greenness. So "faster than CPython everywhere" is NOT backed.
  OWNER: E2/Codex-D — add a freshness (generated_at age vs HEAD) + greenness
  (`gate_fails==false`) gate, then refresh the board. HIGH value: makes E2 provable.
- **compiler-build-resource mutex = working (throughput ceiling, not a fault):** one
  coarse global key serializes even disjoint native-vs-wasm builds on an 18-CPU
  host. Future refinement (per-target-dir sub-keys) could raise throughput; low
  priority vs the above.

### Capacity allocation (priority order; maximize parallel E1-E4 progress)
1. **E1-WITNESS-TO-GREEN** — Codex (claimed). The critical path to the goal.
2. **BUILD THROUGHPUT** (highest wall-clock leverage) — shared content-addressed
   caches proven to HIT, frontend lowering cache shared+attested (task #37),
   memory-bounded parallel builds. Accelerates E1's own acceptance loop. (Codex-B
   R5c overlaps — keep it here.)
3. **EFFECT-ATTESTATION + PERF SCOREBOARD (E2)** — wire perf_scoreboard live+green
   and make optimizations self-attest, so "faster than CPython everywhere claimed"
   is provable and silent degradation is caught. (Orchestrator is auditing this now.)
4. **E3 parity (R6) + E4 structural floor** — continuous/background via gates.

### Standing note: the coordination toolchain is COMPLETE — do NOT build more of it.
tree_drift_check + ff_land + claim_lane + dirty_tree_landing_audit cover
detection / safe-landing / solo-claims / dirty-replay-coverage. Spend cycles on
the goal (E1-E4) and the leverage frontiers above, not more meta-tooling.

**Senior review of Codex's pause-window E1 landings (75024c81e, 5de45d0a8,
ef30a141a, 36b11f58f) — verdict: SOUND, keep going.** The source-plan
relativization (`36b11f58f`) is POISON-clean and preserves fail-closed (raises a
precise diagnostic when a source can't resolve through `source_plan`; no
silent-drop). Seal-source custody hardening + the re-seal gate + `verify_numpy_seal`
are the right shape. **One finding for the E1 COMPLETION greenup (not a blocker):**
`ef30a141a`'s cleaned-generated-source SKIP is currently *silent*. It is backstopped
by fail-closed-at-link (a genuinely-needed skipped symbol surfaces as a wasm-ld
undefined symbol — that is how `molt_PyType_Ready` was caught), which is
acceptable, BUT per the standing "no silent failures" rule it must EMIT a
diagnostic naming which generated units were skipped, and ideally a test asserting
a needed-but-skipped symbol fails closed at link. Address before marking E1
COMPLETE.

**Integration-custody note:** the HELD witness branch
`origin/wip/reconciled-witness-linked-static-20260707` is SUPERSEDED by your
`ef30a141a` (same +155 linked-static `source_extensions.py` logic, now on main). Do
NOT land the held branch — it would duplicate. Orchestrator will retire the ref.

**Still open on E1 (the hard part):** the on-disk seal manifest still has 0
`runtime_python_import_modules` — the seal is not yet regenerated. The blocker
remains the 17 missing numpy meson-GENERATED sources (see the SEAL-REGEN step
below); the tooling/relativization/gate you landed is the scaffolding, not the
regen itself. Drive that next.

## ⏸️ ORCHESTRATOR PAUSE HANDOFF — 2026-07-07 afternoon → back ~evening (READ THIS FIRST — you, Codex, drive during the pause)

The orchestrator is paused for several hours (back ~evening 2026-07-07). **For the
duration of this pause, the witness/E1 stand-down is RELEASED: Codex, you own
driving E1 to green AND the standing lanes — land it all yourself.** When the
orchestrator returns it resumes ownership; until then, this handoff is your
authority. Preserve all parallel work; the operator-authority rules above still
bind (exact-pathspec commits, no trampling, flag-don't-override).

### Tools you MUST use (they fail closed — no silent stale/trample failures)
- `python tools/tree_drift_check.py --witness --fetch` — run at the START of every
  arc and before every land: one-line verdict on whether your tree is
  stale/masking vs `origin/main`. The shared checkout is 377 behind + dirty and
  MASKS the real frontier — verify against a clean `origin/main` worktree.
- `python tools/ff_land.py` — land HEAD to `origin/main` ONLY as a clean
  fast-forward (refuses on dirty tree / non-ff drift / nothing-to-land). Use it
  for every land so you never trample a parallel PR.
- `tools/proof_queue.py` — ALL heavy builds (APDataStore `D:\Molt`, contention
  keys, `--detach`); `status` before queueing; `pact-witness-acceptance` is the
  named acceptance lane. NEVER `git stash` on this shared repo (shared stack →
  race-drops other lanes). Commit `-m MSG` BEFORE `--` (see working agreement).

### LANDED on origin/main this session — DO NOT REDO (verify with git, don't trust memory)
- `89e5160ea` E1 cpython-abi DATA-symbol link fix (`molt_PyType_Ready`/`PyLong_Type`
  class — wasm_link_edit.py rewrites linking-symtab data symbols via the shared
  naming authority, fail-closed).
- `515b4b5cd` scipy NATIVE ndimage callable dispatch; `1ab11b154` asyncio
  submodule-prefix; `6b2060807`+`435cf4d7c` asyncio exception-region; `1a2520b2f`+
  `c5ae5bfe5` native-division fast_float (all verified, teeth green).
- `f7d67fe2e` tree_drift_check DX; `2ba8ca242` ff_land DX; drift-sweep cadence +
  shared-git-stash ban in the docs.

### ⚠️ E1 IS A SOLO LANE — CLAIM IT BEFORE STARTING
`E1-WITNESS-TO-GREEN` (the whole arc below, seal-regen → link → static-lib → exec
→ check_parity) must be driven END-TO-END by ONE agent — splitting it masks the
frontier and invites trampling collisions. Before touching it, follow `docs/agent/CLAIMS.md`:
drift-sweep, check the claim log at `origin/main`, and if it's unclaimed, claim it
(append a row + `ff_land`; the fast-forward is the atomic lock — if `ff_land`
refuses, someone claimed first, so BACK OFF to a standing lane). The claimant owns
it end-to-end and marks `COMPLETE` only after final exit criteria + recursive
adversarial review + senior-engineer sign-off (CLAIMS.md §5). If E1 is already
claimed and alive, work Codex lanes B/C/D instead.

### E1 ORDER-OF-OPERATIONS — drive this to green (each step names its resume point)
1. **SEAL-REGEN (current frontier — it is a STALE SEAL ARTIFACT regen, NOT a code
   fix).** Fully diagnosed by the orchestrator's seal-regen subagent (no code
   changes — origin/main is already correct). **Do NOT change extension_seal.py /
   external_native.py:** origin/main already PERSISTS `runtime_python_import_modules`
   at seal time (`src/molt/cli/extension_seal.py:385-431`
   `_canonicalize_runtime_python_import_modules`, called ~659) and FAILS CLOSED when
   it's absent (`src/molt/cli/external_native.py:607-676`). The shared checkout's
   uncommitted WIP REMOVED that persistence + made the consumer tolerant — that is
   the mask; do NOT restore it. The only defect is the STALE on-disk artifact
   `tmp/pact_numpy_multiarray_sealed_for_witness/extension_manifest.json` (no field;
   object_closure `source` paths baked ABSOLUTE into a deleted worktree).
   RE-SEAL BLOCKER (measured): of 130 object_closure sources, 113 already resolve
   byte-identically from the live tree (105 upstream `.c` under
   `bench/friends/repos/numpy_off_the_shelf` + 8 generated pyd.p sources that
   byte-match `tmp/pact_numpy_linalg_meson_wasm_build/...pyd.p/`). The **17 missing**
   are numpy SIMD-dispatch + npymath GENERATED sources
   (`libnpymath.a.p/{ieee754.c,npy_math_complex.c}` + 15
   `libloops_*.dispatch.h_baseline.a.p/*.dispatch.c`) — no `.c` exists under `tmp/`;
   pure numeric kernels (no dynamic imports) but seal correctly refuses to prove
   that from a partial scan and fails closed. RECIPE: (i) regenerate the 17 via
   numpy's meson codegen custom_targets (`.dispatch.c` from `.dispatch.c.src` etc.,
   `numpy/_core/code_generators`; TRY CODEGEN-ONLY first — cheaper than a full wasm
   compile; same config as the linalg build → byte-identical) and drop them at
   `tmp/pact_numpy_multiarray_meson_wasm_build_generated_metadata/numpy/_core/lib*.a.p/*.c`;
   (ii) re-seal `uv run --active --project . --python 3.12 python
   tools/pact_seal_witness_roots.py --root tmp/pact_numpy_multiarray_sealed_for_witness`
   (this tool is on origin/main, ABSENT from the shared-checkout working tree — use
   the origin/main / clean-worktree copy); verify the manifest gains the field;
   (iii) prove past-seal via `pact-witness-acceptance --detach` from a clean
   `origin/main` worktree. SAME ARC: add a re-seal GATE (test: sealed manifest
   carries `runtime_python_import_modules` AND all object_closure sources resolve,
   fail-closed) and RELATIVIZE the seal's object_closure `source` paths to
   source_plan roots so it's relocatable and can't rot to a deleted worktree again.
   Find the meson-wasm configure recipe via the proof_queue pact-witness lane /
   `_pact_witness_native_roots` / `collab/pact/STATUS.md`. POISON rules bind.
2. **LINK.** Once seal passes, the build reaches the WASM link — the cpython-abi
   data-symbol fix (`89e5160ea`) already resolves that class. Next likely: 17
   variadic C-shim exports (existing `MOLT_WASM_CPYTHON_ABI_EXPORTS` mechanism).
3. **STATIC-LIB LINK.** Land the HELD witness linked-static-library closure branch
   **`origin/wip/reconciled-witness-linked-static-20260707`** (numpy
   `libunique_hash.a`→`unique.cpp`; verified reconciled + non-regressive; it lacks
   a positive test so land it WHEN the build actually reaches the numpy static-lib
   link stage and it's exercised — cherry-pick onto current main, `ff_land`).
4. **EXEC + PARITY.** `pact-witness-acceptance` produces `candidate_outputs.npz`;
   the oracle (`_prepare_reference_oracle` regenerates a fresh CPython reference +
   `check_parity.py`, order-robust, tight ATOL) gives the honest verdict. E1 GREEN
   = check_parity PASS. Faster-than-CPython timing goes on the R8 scoreboard.

### Standing Codex lanes (parallel, subordinate to E1) — see CURRENT CODEX LANES below
B = R5c frontend-lowering reuse/parallelize; C = reconcile+land
`codex/proofqueue-preflight-diagnosis-20260707` (`02d85f922`); D = R6/R8. The
compiler-build-resource mutex is LANDED (PR #100) — do NOT re-chase.

### If you get E1 to green (or hit a true external blocker)
Update THIS handoff with what landed + the new frontier, leave the evidence
(queue run ids, candidate_outputs.npz, check_parity output), and keep the standing
lanes moving. The orchestrator will reconcile on return.

## 🔄 ORCHESTRATOR UPDATE — 2026-07-07 (late, post-recovery; READ FIRST — supersedes the burndown frontier + coordination below)

Session recovered after a clean pause (desktop relaunch). No signal lost: the four
paused worktree branches are banked to `origin/wip/recover-*-20260707`.

- **E1 LINK FIX LANDED + VERIFIED (origin/main `89e5160ea`).** The real frontier
  was the whole cpython-abi **DATA-symbol** class, not one symbol: `wasm_link_edit.py`
  rewrote only kind==0 function imports, so ~54 linking-symtab DATA type-objects
  (PyLong_Type, PyType_Type…) kept UNPREFIXED edges the molt_-prefixed split runtime
  can't satisfy (queue run `20260707T162126` = `undefined symbol: PyLong_Type`).
  Fix extends the rewrite to data symbols + kind==3 globals via the SAME
  `_runtime_import_rewrite_target` authority, fail-closed on unknown kind. Verified:
  teeth reproduced (FAIL without fix, pass with), 105/105 green, cherry-picked clean
  onto current main (avoided a stale-base trample of the vfs lane's files). **NEXT E1
  frontier:** acceptance re-run on a coherent tree → likely the 17 variadic C-shim
  exports (existing `MOLT_WASM_CPYTHON_ABI_EXPORTS` mechanism), then execution-time
  parity (candidate_outputs.npz → check_parity).
- **⚠️ #2 REGRESSION — parallel lane, DROP IT.** The `molt_PyType_Ready` symptom in
  run `20260707T183013` was a SEPARATE uncommitted regression in the shared checkout:
  a parallel lane deleted `linked_native_inputs = tuple(native_objects)` in
  `wasm_link_edit.py`, linking molt_-prefixed inputs against the unprefixed reloc
  runtime. That lane's correct data-symbol rewrite (#1) is now on main as `89e5160ea`
  — that lane must DROP its whole `wasm_link_edit.py` WIP (drift-sweep will show #1
  landed) and NOT commit #2. Do NOT run `pact-witness-acceptance` from the shared
  checkout until #2 is gone — it will re-fail on `molt_PyType_Ready`.
- **Compiler-build-resource mutex is LANDED (PR #100).** The "LAND the mutex fix"
  directive in the CODEX COORDINATION block below is DONE — both
  `codex/compiler-build-resource-mutex-*` and `codex/proofqueue-build-resource-mutex-*`
  are merged+deleted. Do NOT re-land or re-chase.
- **Current Codex lanes:** B = R5c frontend-lowering reuse/parallelize (queue note
  1431 evidence); C = reconcile+land `codex/proofqueue-preflight-diagnosis-20260707`
  (`02d85f922`, unlanded) so acceptance fails closed with exact missing-import
  evidence before long builds; D = R6/R8 continuous. Codex STAND DOWN on the
  witness link / cpython-abi lane.
- **Banked-branch reconcile (orchestrator integration-custody, in progress):**
  scipy `842da9c8f` module_attr dispatch is largely SUPERSEDED on main (ndimage
  PRs) → RETIRE with note, do not land (trample). asyncio `ordered_positions`
  exception-region fix + submodule-prefix fix are UNLANDED → reconcile+land.
  witness `source_extensions.py` seal WIP predates the passed seal frontier →
  review-before-land. Codex: do not cherry-pick from the `wip/recover-*` refs.

## 🎯 CURRENT SWARM BURNDOWN — 2026-07-07 (orchestrator; READ FIRST — supersedes stale State-of-the-World below)

### FINAL EXIT CRITERIA — the swarm is DONE only when ALL FOUR hold
- **E1 · WITNESS GREEN.** `collab/pact/pact_witness_kernel/field_solve.py` (numpy + scipy.ndimage) → Molt **WASM** → `candidate_outputs.npz` → `check_parity.py` **PASS**. Zero fakes, zero host-CPython/Pyodide fallback, executable ABI dispatch only, all ecosystem behavior through real custody primitives.
- **E2 · PERF > CPython** on the claimed benchmarks: R3b/R4a numeric raw-lane + `spectral_norm` + the 54–67 portfolio, proven on `tools/perf_scoreboard`.
- **E3 · PARITY.** CPython ≥3.12 within the verified subset — R6 conformance shards + differential green.
- **E4 · STRUCTURAL FLOOR.** god-file/god-crate ratchet green; fail_closed poison classes at/under baseline and trending to zero (live: ecosystem_baked=8, fail_open_stub=1, duplicate_authority=3, todo_as_plan=0, fail_open_backend_dispatch=2 — read the live TOML, drive DOWN); effect-attestation live so no capability silently degrades (task #33); warm shared builds via auto-wired fast toolchain (tasks #33/#34).

### LANDED on origin/main this session — DO NOT REDO
SCC frontend-parallel condensation; shared content-addressed lowering cache; degrade-to-slow gate; fail_closed gate (Scans A–E, ~2min→<15s); numpy header custody; fail-open ABI burndown (fail_open_stub 12→1); PySet real ABI hooks; cross-language table-drift gate; DX uv-project-env stability; **float shortest-round-trip authority** (`0f65cb8b1`); **ABI-layout wasm32 ILP32 fix** (`6b95abd5e`, unblocks ALL wasm32 cpython-abi rebuilds); **sccache auto-provision + loud-degrade, hardened** (`9e8390d2a`, kills the cold-build metabug — builds are now warm+shared).

### WITNESS FRONTIER (E1) — ORCHESTRATOR + SUBAGENTS OWN. CODEX STAND DOWN.
Each acceptance failure has been FORWARD progress down the real dependency closure:
R0 header custody ✅ → numpy `_multiarray_umath` wasm build ✅ → ABI-layout wasm32 ✅ → numpy **provided_capsules (4)** ✅ → **CURRENT blocker:** `field_solve` transitively reaches 8 numpy support modules lacking source/artifact custody (`numpy._core.{clip,dtype,einsum,matmul,number,shape,vdot}` + `numpy.lib._arraysetops_impl.unique`) — likely a seal-declaration/reachability-mapping gap (the sources already compile into `_multiarray_umath`). NEXT: seal-module completeness → scipy.ndimage capsule linkage → `pact-witness-acceptance` → `check_parity`.

### CODEX LANES — EXPLICIT BURNDOWN (priority order). Cite queue run-ids; commit by EXACT pathspec; run the ownership audit before EVERY commit. sccache is auto-provisioned so heavy builds are WARM/SHARED — you MAY build, but ONE heavy build per contention-family and the WITNESS keeps priority (do not run unbounded parallel cargo/wasm).
- **LANE C1 · GOD-FILE / GOD-CRATE DECOMPOSITION (standing P0 — your #1 dev-velocity obligation).** EXIT: the god-file/god-crate ratchet is GREEN. Burndown, largest first: `molt-runtime` is a **425-file god-crate** (the dominant single-crate compile cost — it serializes compile parallelism and taxes every build) → decompose along the CPython-mirrored axis; then `molt-backend-wasm` (163), `molt-backend-native` (123), `molt-passes` (114). Each split = pure renames + PRECISE pub-widening (never blanket `pub(crate)`→`pub`) + a per-crate clippy gate; VALIDATE every decomposition with a real `molt build` E2E (non-build gates pass atop a broken compiler). DONE when the ratchet is green AND molt-runtime cold-compile time measurably drops.
- **LANE C2 · R3b/R4a NUMERIC RAW-LANE PERF (E2).** ⚠️ COORDINATE: the live R3b/R4a WIP (`scalar_carriers.rs` + `runtime/molt-backend-wasm/src/wasm/lir_fast/*`) is TAKEN OVER by an orchestrator subagent (task #30) — do NOT double-drive those files. Your R4 lane = the OTHER numeric keystones (non-carrier raw-lanes). EXIT: numeric cluster A + `spectral_norm` faster than CPython on the perf scoreboard, teeth green (`tests/test_r3b_numeric_cluster_manifest.py`, `tests/differential/basic/R3B_NUMERIC_CLUSTER_A.txt`). Run `proof_queue.py diagnose <run-id>` on a failed build before resubmitting the same shape.
- **LANE C3 · R6 CPython ≥3.12 PARITY/CONFORMANCE (E3, continuous).** EXIT: conformance shards + differential parity green within the verified subset; feed corrections back as typed facts.
- **LANE C4 · R8 SCOREBOARDS + RELEASE GATES (makes E2/E4 MEASURABLE — do this early).** EXIT: `tools/perf_scoreboard` wired + green so E2 is provable; release gates assert E1–E4 so "done" is machine-checkable, not asserted.

### CODEX STAND DOWN — orchestrator/subagent-owned, ZERO exceptions, do NOT edit
- **WITNESS closure:** numpy seal/custody, `scipy.ndimage` ABI-dispatch (task #28), ALL of `runtime/molt-cpython-abi/**` (capsules, `_molt_abi_layout.generated.h`, ABI-layout generator).
- **DX / THROUGHPUT authority:** `src/molt/dx.py` (sccache/attestation), `tools/proof_queue.py`, `tools/{fail_closed,degrade_to_slow}_*`, `.cargo/config*` (lld auto-wire — task #34).
- **FRONTEND metabug lanes:** `src/molt/cli/{frontend_*,module_*}.py`.
- **IN-FLIGHT subagent lanes:** asyncio drop-insertion (`molt-passes/.../drop_insertion/exception_region.rs`, #16), float repr (`molt-runtime/src/object/float_repr.rs` + `molt-backend-rust` prelude, #22), native divmod/floordiv/unary (#23/#24/#25), R3b/R4a carrier authority (`scalar_carriers.rs` + `lir_fast/*`, #30).
- **ENFORCEMENT/attestation:** effect-attestation invariant (#33), fast-toolchain auto-wire (#34).

### META-META BINDING (all agents, all lanes)
A configured capability that never PROVES it fired is a silent-degradation waiting to happen (sccache was off for months while "configured"). Every perf/capability path you land must emit a machine-checkable proof-of-effect (cache-hit-rate>0, raw-lane fire-count, worker-count) or fail LOUD — never a silent skip. "Landed" ≠ "effective"; measure the effect on a representative run. See task #33.

### 🔧 CODEX COORDINATION — 2026-07-07 (orchestrator; binding, read before your next arc)
Codex found the REAL root of the cold-build/witness-failure class and it is HIGH VALUE: concurrent build-heavy queue rows with DIFFERENT contention keys overlapped and hit Windows **os error 1450 (insufficient system resources)** writing rustc bytecode under `D:\Molt\target\sessions\...` — this is the structural cause behind pact rc15/rc6 and the memory saturation the orchestrator was hand-quiescing. Codex's fix — a **compiler-build-resource mutex** grouping native-build + queue-native-rust + wasm/wasm-browser under ONE slot so heavy builds serialize while light/disjoint work stays parallel — is the CORRECT structural fix (replaces manual quiesce). THREE binding actions:
1. **LAND the mutex fix on origin/main (reconciled).** Codex's proof_queue.py base is ~248 commits behind main, so its diff does NOT cherry-pick clean onto current proof_queue.py — reconcile the mutex LOGIC (compiler-build-resource family grouping + the native-build-vs-wasm-browser test) onto CURRENT main, land it, keep the docs/PROOF_QUEUE.md widening. This benefits the entire swarm immediately.
2. **CODEX: REBASE onto current origin/main (>= 46dd33ded) BEFORE any further measurement or acceptance run.** Your branches are 248-273 behind and LACK: sccache auto-provision (so your cold-build numbers conflate "cache ineffective" with "branch predates the fix"), the ABI-layout wasm32 fix (6b95abd5e), float authority, and the witness numpy-seal progress. Any throughput timing on a pre-sccache branch is DIRTY data. Re-measure only on current main.
3. **CODEX: STAND DOWN on the witness acceptance / pact rerun.** Witness closure (numpy seal, ABI-layout, capsules, scipy.ndimage #28) is ORCHESTRATOR+SUBAGENT-owned; subagent afd301b5 is actively driving it on current main. Your stale-base acceptance rerun (20260707T170524) will hit blockers already solved on main (ABI-layout gate) and DUPLICATES/CONFLICTS with the live witness lane. Hand the witness back; resume your assigned lanes: C1 god-crate decomposition (P0, molt-runtime 425 files), C2 numeric raw-lane, C3 parity, C4 scoreboards.
Interacting-issues note (they compose): (a) queue-contention os-1450 = Codex's find, fix landing; (b) sccache rustc-cache = landed on main, absent on Codex branch; (c) frontend LOWERING cache not shared/effective (~180s numpy re-lower, MEASURED) = task #37, separate Python-side cache, NOT helped by sccache; (d) per-session CARGO_TARGET_DIR + lowering-cache root = shared root cause of both (b) and (c). Disentangle before concluding.

## ⚠️ INCIDENT 2026-07-03 + git_guard (all agents read)

**Signal loss + recovery ask.** A `git reset --hard HEAD` in an orchestrator
cleanup one-liner discarded UNCOMMITTED, unstaged working-tree WIP in the shared
checkout — specifically a concurrent lane's `runtime/molt-runtime/src/call/function.rs`
refactor: `native_function_preempts_with_trampoline` / `function_trampoline_call_target_ptr`
+ a `native_trampoline_dispatch_ignores_function_direct_call_target` test. It was
never staged or committed, so it is NOT git-recoverable (checked: not on origin,
not in dangling blobs). **If that is your lane: RE-APPLY it from your own context
and COMMIT it immediately (by exact pathspec) — do not assume it survived.** The
shared working tree is currently at a stale base (b0a7e2745) for ~35 files;
their content is safe on origin/main. OneDrive version history is a fallback for
the function.rs loss.

**MECHANISM (not just a rule): `tools/git_guard.py` is now landed and MANDATORY
for the shared checkout.** Destructive working-tree git — `reset --hard`,
`checkout -- <path>` / `checkout -f`, `clean -fd`, `stash drop/clear/pop`,
`branch -D`, `gc --prune=now` — is BANNED on the shared main checkout. Use it
only inside an ISOLATED worktree or plumbing-index mode (`GIT_INDEX_FILE`).
- Need a clean tree for a build/cherry-pick trial → `git worktree add`, never the
  shared checkout.
- Route any unavoidable destructive op through `python tools/git_guard.py run --
  <git args>` (refuses on the shared checkout, snapshots first).
- An always-on recovery net (`git_guard.py watch`) snapshots WT+index to
  `refs/wip-guard/*`; recover via `git_guard.py list` + `git stash apply <sha>`
  in a worktree. This is defense-in-depth, NOT a license to run destructive git.

## CANONICAL CRATE NAMING (operator directive: standardize like Lattner)

One convention, mirroring the CPython layer axis, replacing the inconsistent mix
of `molt-runtime-*` / `molt-lang-*` / `molt-*`:
- **Core / primitives**: `molt-object` (the `MoltObject` NaN-box value repr;
  currently pkg `molt-lang-obj-model`, dir `molt-obj-model` — drop the `-lang-`
  prefix, make package == dir). The object protocol (ops), when extracted, joins
  it or becomes `molt-object-protocol`.
- **Runtime API surface**: the crate currently MISNAMED `molt-runtime-core` is
  NOT core — it is the thin re-export/API-surface subcrates depend on. Rename to
  `molt-runtime-api` (honest name; a wrapper that masquerades as core is a
  canonicalization defect).
- **Stdlib**: `molt-stdlib-<mod>` for every stdlib module crate. Rename the ~19
  `molt-runtime-{crypto,tk,math,path,collections,regex,itertools,serial,difflib,
  logging,http,stringprep,xml,ipaddress,zoneinfo,net,asyncio,compression,text}`
  → `molt-stdlib-{…}`. This makes the stdlib layer legible at a glance.
- **Third-party / extensions**: `molt-cpython-abi` (drop `molt-lang-` package
  prefix; package == dir).
- **Backends / IR / passes**: already consistent (`molt-backend-*`, `molt-ir`,
  `molt-tir`, `molt-passes`) — leave.
SEQUENCING: a crate rename is build-breaking and touches every Cargo.toml + `use`
path, so it is an ATOMIC sweep per crate (or a tight batch) in an ISOLATED
worktree, gated on a full `cargo build` + `check_rustfmt --changed` + the
per-crate clippy gate, then cherry-picked. Do renames when the touched crate has
no other in-flight lane (coordinate on this board). Do NOT interleave a rename
with a semantic change in the same commit — rename-only diffs must stay reviewable.

## ✅ P0 BROKEN MAIN (2026-07-03): RESOLVED — molt-runtime compiles + gate landed

Both breaks are fixed on origin/main and a recurrence gate is in place (verified
2026-07-04: all four commits are ancestors of origin/main; the broken callers and
dangling re-export are gone; the witness lanes build molt-runtime green):
1. **gpu_primitives re-export** — FIXED (4a8c603a1): repointed lib.rs to
   `molt_gpu::primitives_ffi`. origin lib.rs no longer references
   `crate::builtins::gpu_primitives`.
2. **memoryview descriptor** — FIXED (array_mod.rs c1414d9cf + ops_builtins.rs/
   graphlib 7b5382c66): array_mod.rs no longer calls the removed
   `memoryview_format_from_code`/`one_dim_with_format`/`new_with_format` (E0432
   source gone); callers moved to the unified format-bits/base-bits API mirroring
   ops_memoryview.rs.

SYSTEMIC GAP CLOSED: the full-feature `cargo check -p molt-runtime` gate now runs
in CI (aa948db77), so a molt-runtime break can no longer accumulate atop passing
non-build gates. Every lane touching molt-runtime must still run a full-feature
build before landing, but CI is now the backstop.

⚠️ MOLT-RUNTIME CLIPPY GATE (`clippy --all-targets -p molt-runtime -- -D warnings`)
is RED on pre-existing debt in **molt-cpython-abi/src/api/** — the WITNESS LANE's
territory. Remaining errors (2026-07-04): `buffer.rs:86-87,123-124`
field_reassign_with_default; `buffer.rs:529` match→unwrap_or_default;
`imports.rs:111` collapsible_if; `strings.rs:454` let_and_return.
**cpython-abi lane owner: fix these in your next arc** (you edit buffer.rs anyway);
they block the clippy gate for every molt-runtime decomposition lane. The
orchestrator already cleared the other 5 (e4710c05a): 2 tk-split import-gating
regressions (filehandlers/timers) + 3 peripheral (wasi_sysroot lib_dir dead_code +
let-chain collapses in wasi_sysroot/tarfile). Do NOT let another lane touch
molt-cpython-abi/src/api/** — hot witness lane.

BUILD-LEAK PREVENTION (2026-07-04, all lanes benefit): the memory-pressure /
orphaned-build-process class is now closed at the source. `tools/win_job.py` +
`memory_guard.run_guarded` wire every guarded build into a Windows Job Object with
`KILL_ON_JOB_CLOSE` (a5aae5056), so a build subtree dies the instant its guard
dies — no more orphaned cargo/rustc/link/tail reserving GB (hit 42 GB / 98% on
2026-07-03). `tools/orphan_reaper.py watch` (01f07a1f1) is the standing sweep net
for builds that bypass the guard. Route builds through the queue; never leave
`cargo | tail` in a lane.

## 🔥 RIP-IT-ALL-UP DECOMPOSITION ROADMAP (operator 2026-07-03: "rip it all up")

**PRIORITY ORDER (operator 2026-07-03): BACKEND / RUNTIME-NATIVE / WASM / LLVM
FIRST.** Rip these before anything else: `molt-backend-native` (67k, orchestrator
subagent NOW), `molt-backend-wasm` (27k, orchestrator subagent NOW — avoid
call_ops/dynamic.rs), `molt-backend` (16k), `molt-wasm-host` (5.7k), and the
native/wasm lowering inside molt-runtime once the core opens. `molt-backend-mlir`
(the LLVM/MLIR path) is only 2,710 lines — already under god-file thresholds,
nothing to rip. Codex: claim a BACKEND crate above FIRST; molt-passes / stdlib /
tk come after the backend set.

Aggressive scope, careful execution. EVERY god-crate gets an owner and a
build-verified, contract-gated cut. Current sizes (hand-written `.rs`, excl
generated):
- **molt-runtime 286,672** (THE god-crate) — the core/object extraction is the
  KEYSTONE, BLOCKED until buffer lane 2 quiesces in `object/**`. It is actively
  LANDING now (c798e4833 typed strided buffer custody, 83a8a154b import bedrock
  PR1), so the unblock is APPROACHING. The moment `object/**` is quiet: sever the
  7 object→builtins back-edges → extract `molt-runtime-core` (object model) →
  carve `builtins` (134k) → `molt-runtime-builtins`. This is the cut that ends
  the ~2160s witness rebuild. Orchestrator signals when it opens.
- **molt-passes 82,871** — DISJOINT, unblocked. Rip into pass-family sub-modules/
  crates (value_range already split by a lane; keep going: the other large TIR
  passes). → CODEX LANE A. ⚠️ **module_slot_promotion split (f90b3f278) LANDED
  BROKEN** — promote.rs referenced 4 un-imported symbols (CfgEdgePolicy,
  build_pred_map_with, ModuleSlotAccessRole, opcode_module_slot_access_role_table)
  → molt-passes did not compile → whole native backend broken on main. FIXED by
  orchestrator 34120fe58 (2026-07-04). **CODEX: your `codex/passes-main-gate`
  2c7408412 "Repair module slot promotion split imports" is now REDUNDANT — do
  NOT land it blind; the imports + doc-comment cleanup are already on main.
  Reconcile any remaining passes-main-gate delta against current main.** LESSON
  (binding): a decomposition that does not `cargo check` the WHOLE consumer graph
  before landing WILL break main silently (non-build gates pass atop a broken
  compiler). Build-verify the full crate + a top consumer before every decomp land.
- **molt-backend-native 67,194** — DISJOINT from the wasm witness. → ORCHESTRATOR
  SUBAGENT. Progress: handle_call_op (fc/calls.rs) + emit_op + **handle_arith_op
  (fc/arith.rs 1849-line mega-fn → dispatcher + 8 per-family helpers, 9b4dca7d8,
  byte-identical codegen proven via FNV-1a golden)** LANDED. Remaining mega-fns:
  compile_func_inner (function_compiler.rs, tangled prologue/epilogue),
  handle_loop_op (fc/loops.rs), direct_ops.rs (LLVM-only, unbuildable here).
- **molt-gpu 36,117** — rip the render/ + tensor_runtime clusters into sub-modules;
  also the destination for the BLOCKED 11,925-line builtins/gpu cluster (post-core).
  → CODEX LANE B (coordinate w/ codex-doc71).
- **molt-backend-wasm 27,205** — partial (molt_type_new touched dynamic.rs); rip
  the NON-dynamic god-files only. → CODEX LANE C (do not touch call_ops/dynamic.rs).
- **molt-runtime-tk 20,588** — ✅ LANDED (orchestrator, 68b2d895a + 06e4f9347):
  move-only split of the two biggest tk god-files (`callback_intrinsics.rs` 1426 →
  timers/traces/tkwait/binds/filehandlers/event_subst; `intrinsics.rs` 1272 →
  lifecycle/dialogs). Byte-identical bodies; 42 extern-C symbols preserved
  identical; cargo check + clippy green. Residual tk god-files
  (`ttk_treeview.rs`, `ttk.rs`) are single-mega-function files — NOT move-only;
  need internal helper extraction, deferred.
- **molt-tir 19,835 · molt-backend-luau 18,161 · molt-runtime-serial 17,546** —
  mid-size, decomposable as capacity frees.
- **runtime/molt-runtime/src/builtins/functions_pickle/binary.rs 3378** — ✅
  LANDED (840f76ab7): move-only split into binary/{consts,state,read,dump,load,
  entry}.rs + mod.rs; byte-identical bodies (3375 lines reconstruct exactly),
  4 extern-C entrypoints preserved, cargo check rc=0, clippy-clean, kitchen_sink
  score 568→0. Only the ~950-line load VM (`molt_pickle_loads_core`) stays whole
  in entry.rs (one function, move-only forbids splitting it).
- **tools/proof_queue.py** — ✅ DONE (f94f3a4f9): decomposed 5760 → 4457 lines,
  the 3 god-regions (`_run_diagnostics`/`_run_one`/`_build_parser`) extracted into
  modules; kitchen_sink ratchet CLEARED (score 0, is_god False on origin), no
  baseline mask. NOTE: a stale session-base working tree still shows the old 5760
  version — verify structural_audit against a clean origin worktree, not the
  shared checkout (see Proof/DX rules).

GATES (every cut, non-negotiable): build-verified (leaf `cargo check` + `clippy
-D warnings` + queued god-crate check); `tools/canonicalization_contract.py
--check` green; `tools/structural_audit.py --check` must IMPROVE — NEVER
`--update-baseline` to hide debt; STRICT move-only diffs; exact-pathspec commits;
ISOLATED worktrees off current origin/main (or session base, then orchestrator
reconciles); orchestrator cherry-picks. `git worktree list` BEFORE claiming —
lanes hold uncommitted WIP (three collisions caught this session).

FANOUT: orchestrator holds ONE subagent (operator constraint) + delegates the
rest to Codex. Codex — claim a DISJOINT god-crate above, rip it end-to-end, ping
to cherry-pick. Do not idle; do not duplicate; do not break a hot tool.

## 🧭 APPARATUS TRACK — interpretable + self-improving (operator 2026-07-04)

Plan: `docs/design/foundation/72_interpretable_self_improving_apparatus.md`.
Make the orchestration control plane (proof_queue, gates, memory, board) an
interpretable-by-construction, self-improving apparatus — grounded in Rudin
(interpretability over black-box scores), Daubechies (multi-resolution), and
Schmidhuber (compression progress as the intrinsic-reward signal + Gödel-machine
"enforce only once proven"). Guardrail (`instrumental-serves-outcomes`): this
serves the 100-year OUTCOMES only by making the apparatus learn faster; it never
displaces compiler outcome work.

MATURE REFERENCE = the **pact / comma-lab** apparatus (Tailscale `primary`,
`~/Projects/pact`, 1.25M LOC; memories `pact-apparatus-reference`,
`pact-triality-architecture`). Two patterns to port:
- **Gate catalog + STRICT-flip lifecycle** (pact `src/tac/preflight.py`, ~295
  STRICT gates). Each gate is a dated, interpretable bug-class rule that
  observes → proves the tree clean → STRICT-flips to enforce @0. Molt: consolidate
  structural_audit / canonicalization_contract / op_family / check_*.py toward one
  catalog authority with dated provenance + STRICT-flip.
- **The triality — DAG ↔ DSL ↔ equations.** A finding is "known" only when it has
  all three AND they agree; drift = forgetting (pact `triality_drift_detector.py`).
  Molt's nascent triality: op_kinds.toml + representation_facts + perf-keystone
  laws (**equations**) ↔ generated gates + build/proof commands (**DSL**) ↔
  proof-queue history + board + git (**DAG**). op_family (dispatch↔handler drift)
  is a triality-drift-detector in miniature.

BUILD ORDER (doc 72 §"Concrete integrations"):
1. **Compression-progress ledger** — ✅ v1 LANDED (`tools/apparatus_ledger.py`):
   scans the proof-queue run history, normalizes failures to signatures, reports
   COMPRESSION DEBT (recurring-uncompressed failure-mass share) + the CURIOSITY
   QUEUE (surprises ranked recurrence×cost = the next diagnosis rules to write).
   First run: 93.7% debt, 5 recurring signatures carry 269/287 failures — but the
   top signatures are the generic proof_queue WRAPPER line (`finished
   status=failed exit_code=1`), i.e. the queue log buries the inner rust/python
   error. NEXT: refine the extractor to dig past the wrapper to the real error;
   then wire the ledger's curiosity queue into the diagnosis-rule authoring flow.
2. **Interpretability contract** — every apparatus decision emits a named rule
   with evidence + next_action; flag black-box thresholds (extends
   canonicalization_contract.py).
3. **Rule-falsifiability contract** — every new diagnosis rule / gate lands with a
   positive fixture AND a negative control (the `world-class-rigor-no-fakes` "prove
   the gate fails on a synthetic violation" made a gate).
4. **Molt triality-drift-detector** — a fact/optimization is "known" only with an
   equation + a gate + a proof row, all agreeing.

LANE: orchestrator-owned (apparatus is a hot shared tool set). Codex — do NOT
edit proof_queue.py / structural_audit.py / canonicalization_contract.py /
apparatus_ledger.py under this track without a board assignment; flag ideas here.

## State of the world (read this first)

- **✅ UPDATE 2026-07-06 (latest, orchestrator): SILENT-DEGRADATION METABUG CLASS ELIMINATED (0 pending, gate-enforced); WITNESS R0 BLOCKER = cpython-abi numpy header-custody collision.**
  - **Metabug class DONE + firewalled.** All landed on origin/main: SCC-condense frontend parallelism `522b7fe04` (A1-A4: no more serial-on-cycle/timeout/worker-error); content-complete cache fingerprint `669c0ebcf` (fixed a stat-metadata miscompile vector); shared cross-session frontend cache `50652359f` (B1/B2); enforcement gate `6ddb895ed` + registry reconciled to **metabug_fix_pending = 0/0** (ratchet-enforced — a new silent degrade-to-slow cannot land without a loud diagnostic + a fast-path-on-hard-input test or a reviewed sound_keep). `tools/degrade_to_slow_gate.py` should be wired into CI.
  - **WITNESS R0 now blocks on a header-custody REGRESSION** (found by the seal arc, which passed every seal-input stage — WASI toolchain, meson setup, all 8 generated C sources, `molt extension build` ran). `_source_extension_include_dirs_for_abi_tier` (source_extension_toolchain.py:329-333) injects molt/include's PARTIAL numpy overlay for cpython-abi, colliding with numpy's OWN complete headers (both include orders fail: PyCFloatScalarObject / _PyArray_LegacyDescr / structmember.h). The OLD working seal never included molt include/. FIX = header-custody redesign (one complete CPython-ABI header authority; don't shadow the package's numpy/* headers). Reusable regen+verify tooling landed `4b61a0c74`. Orchestrator-owned; do not paper over with an include reorder.
  - **AGENT DISCIPLINE (binding):** subagents run in-process → a harness crash kills them; MITIGATE with mandatory incremental commits + preserve-worktree-on-crash + durable TaskCreate tracking. One seal-arc agent violated isolation (edited shared-checkout commands.py) then reverted byte-clean — verified. Coordinate durable work via board + queue + Codex (crash-independent).
- **✅ UPDATE 2026-07-06 (late, orchestrator): R73.1 + R73.2 LANDED; WITNESS FRONTIER MOVED; NEW METABUG ARC; CRASH-RESILIENCE LESSON.**
  - **R73.1 + R73.2 LANDED (origin/main da862df81, teeth-verified).** R73.1 = shared content-addressed `runtime.wasm` cache + memory-bounded cargo jobs (8GB-capable, no per-session cold rebuild). R73.2 = Molt auto-provisions Cython/WASI/meson from package metadata and regenerates extensions **STANDALONE** — `scipy._cyutility` is a PROVEN BYPASS (standalone `cython -3` → 0 `_cyutility` refs), NOT a wall. The "scipy._cyutility unsolved structural gap" note below is **SUPERSEDED**.
  - **WITNESS (R0) FRONTIER = STALE NUMPY SEAL.** A clean-worktree witness build now fails CLOSED (correctly) at numpy custody: `tmp/pact_numpy_multiarray_sealed_for_witness` has no `runtime_python_import_modules` and all 130 `object_closure` C sources point at a DELETED pact-collab meson dir. Next R0 arc = regenerate the numpy `_multiarray_umath` meson-wasm seal (STATUS.md L206-211) + reseal + verify. **Orchestrator-owned.** Build from a clean origin/main worktree (shared checkout is transiently unbuildable when a Codex Rust lane has in-flight WIP).
  - **NEW BINDING ARC — SILENT-DEGRADATION METABUG (operator-flagged).** Perf/capability paths that SILENTLY degrade to naive on a handleable input. Honest audit = ~3 real defects (NOT the ~189 sound conservatisms): A1-A4 = frontend parallelism disabled on any import cycle / phase-timeout / one worker error → whole cold numpy+scipy frontend runs serial (~9 min); B1/B2 = frontend lowering cache is session-local → cold re-lower every session. Fix = SCC-condense + resilient pool + shared content-addressed cache, plus a `degrade_to_slow_registry` enforcement gate so the class can't regrow. **ORCHESTRATOR/SUBAGENT-OWNED lane: `src/molt/cli/{frontend_parallel,frontend_execution,frontend_pipeline,frontend_worker,module_dependencies,module_cache,module_frontend_cache}.py` + `tools/degrade_to_slow_*`. CODEX STAND DOWN (these are src/molt/cli frontend — already off your lanes).** Crash-recovered preserved branches to verify+land: `agent-scc-preserved-20260706` (fc652b96d, 12 teeth pass), `agent-frontend-cache-20260706` (2d6f0df7f).
  - **CRASH-RESILIENCE (binding, 2026-07-06):** background subagents run IN-PROCESS with the orchestrator's harness → a harness crash kills them all and loses in-process state (they can't return). Durable coordination = THIS BOARD + the proof queue + Codex (separate crash-independent processes). Prefer coordinating Codex/queue over fragile in-process subagent fan-out; instruct agents to commit incrementally; on a crash, `git -C <agent-worktree> add -A && commit` to PRESERVE before any cleanup. Also found: `memory_guard` orphan-cleanup can SIGTERM legitimate frontend parallel-worker subprocesses — fix belongs with the A1-A4 parallel arc.
- **✅ REPRIORITIZATION RELEASED 2026-07-06 (orchestrator): CODEX RESUME NORMAL
  HEAVY BUILDS.** The emergency witness-priority hold below is LIFTED. Honest
  reassessment (adversarial audit): the witness heavy WASM build cannot complete
  right now regardless of memory, because the next blocker — staging
  `scipy._cyutility` (a Cython 3.1 `--generate-shared` native module the witness's
  `scipy.ndimage.label` exec-imports) — is an UNSOLVED STRUCTURAL GAP in Molt's
  source-recompiled-extension build path, not a memory problem. Holding the whole
  swarm's productive R4a/numeric heavy builds for a witness build that can't run
  was wrong. Swarm: resume. The witness now needs a focused structural arc
  (teach Molt's extension build to build a Cython shared-utility module +
  its `__Pyx_ImportType` type exports, then re-seal/stage it), NOT a build slot.
  Everything AROUND the witness build is now ready: numpy/scipy submodule closure
  fixed, ABI 4, Cython exec-import scanner fixed, node runner + parity oracle
  verified (check_parity cp1252 fix f4780a382; CPython field_solve vs reference =
  PASS on all 11 arrays, floats bit-identical → parity is achievable once Molt
  executes the ops). The blocker is solely scipy._cyutility native custody.
- **⚠️ EMERGENCY QUEUE REPRIORITIZATION 2026-07-06 (orchestrator,
  operator-authorized): WITNESS P0 HAS EXCLUSIVE HEAVY-BUILD PRIORITY — CODEX
  STAND DOWN ON HEAVY WASM/CARGO BUILDS.** [SUPERSEDED — RELEASED ABOVE.] The host is memory-CONTENDED (~1GB
  available, 59GB commit / 85GB limit) — this is NOT a leak: `orphan_reaper.py
  sweep` confirms 0 orphaned Molt build processes; the live cargo/rustc are
  legitimate concurrent builds. The constraint is heavy-build CONTENTION — the
  pact-witness P0 WASM build cannot complete while other heavy WASM/cargo builds
  run (both thrash + time out; cf. R4a `spectral_norm` WASM artifact rc=124
  @1209s). **All Codex agents and subagents: STAND DOWN on heavy builds — full
  `molt build --target wasm`, R4a spectral/comparison WASM-artifact builds, and
  multi-crate cargo builds — until the witness reaches parity OR the orchestrator
  releases this hold.** The witness P0 (Kernel A → `candidate_outputs.npz` →
  `check_parity`) is the ONE heavy build that runs. Light work continues:
  Python, docs, single-crate `cargo check` via the queue, tests. **MEMORY
  DISCIPLINE (standing lesson): memory pressure is diagnosed by `orphan_reaper.py
  sweep` (leak vs contention), then fixed by QUEUE REPRIORITIZATION (orchestrator
  authority) — NOT by manual process hunting. Run landed tools from a worktree,
  since the shared checkout is base-stale and lacks tools/orphan_reaper.py.**
- **UPDATE 2026-07-06 (orchestrator): TARGET-FEATURE AUTHORITY UNIFIED +
  RECLAIMED (c54839969).** Target-feature + browser-profile truth now flows
  through ONE generated authority: `src/molt/target_feature_manifest.toml`
  (source) → `tools/gen_target_feature_manifest.py` (`--check` drift gate) →
  `src/molt/_target_feature_manifest.py` + `wasm/target_feature_manifest.json` +
  `wasm/target_feature_constants.generated.js`. The `WEBGPU_DISPATCH_HOST_IMPORT`
  / `TARGET_FEATURE_MANIFEST_ASSET_NAME` / `BROWSER_TARGET_FAMILY` constants are
  now DERIVED generated facts (`molt_gpu_webgpu_dispatch_host` derives from
  `wasm-browser-webgpu`'s `browser_host_imports.webgpu[0]`); the 6 duplicate
  hand-defined literals across `cli/browser_target_features.py`,
  `wasm/browser_target_features.js`, `browser_host.js`, `browser_embed.js`,
  `run_wasm.js`, `cli/wasm.py` are DELETED — every consumer imports from the
  generated source. Proven: `gen --check` green + 21 gen/metadata tests + 2
  teeth-tests (`test_derived_constants_are_single_sourced_from_manifest`,
  `test_no_duplicate_literal_definitions_of_webgpu_dispatch_host_import`, which
  fail on a reintroduced literal). **CODEX STAND DOWN on target-feature /
  browser-profile truth — orchestrator-owned lane. Any target feature,
  capability flag, or browser-host-import MUST be declared in
  `target_feature_manifest.toml` and regenerated via the generator; NEVER
  hand-add a feature literal, a second capability list, or a backend-local
  reclassification.** The uncommitted target-feature WIP in the shared checkout
  is superseded by this landing (captured at D:/Molt/harvest/; drop it).
- **UPDATE 2026-07-05 (orchestrator, ground-truthed on origin/main):** the
  ndimage FRONTEND gate is CLEARED. `scipy.ndimage.distance_transform_edt` and
  all 5 witness ops (`gaussian_filter`, `label`, `maximum_filter`,
  `minimum_filter`) now lower through `invoke_ffi` — the sealed scipy.ndimage
  manifest carries all 5 `callable_exports` (`module_attr` binding, provider
  module, `molt.object_call(args)_v1`) and reachability is fixed (**6eda13d2a**,
  Branch A, independently verified). ndarray buffer-lease custody landed
  (**ff9f7c23a**, queue-verified). ABI custody does not block. **The real blocker
  was DOWNSTREAM and is now FIXED:** a `molt-backend-wasm` E0583 compile break
  (the **d0b381224** wasm split left bare `mod equality; mod ordered;` children in
  the nested `#[path]`-loaded `comparison_ops` module → rustc resolved them to
  `numeric_ops/`) broke EVERY WASM build on main. Fixed **df4e5e738** (explicit
  `#[path]`, matching the working `aggregate_ops` convention; `cargo check
  -p molt-backend-wasm` rc=0). **LESSON (reinforces the binding working
  agreement): `cargo check` every touched crate after a decomposition/split
  BEFORE landing — a split that does not compile is a broken-main P0 that blocks
  the whole witness.** Witness acceptance is RE-RUNNING on df4e5e738 to surface
  the next step (wasm link / runtime-exec / parity) toward `candidate_outputs.npz`
  + `check_parity` green (owner: orchestrator, R0). DX gap closed by the
  APDataStore target-root resolver: RunContext now derives managed toolchains
  from the selected artifact root as `D:\Molt\target-root`, and stale
  `E:\molt-target`, `E:\Molt\target-root`, or empty `D:\molt-target` defaults
  are rehomed unless explicitly preserved with `MOLT_PRESERVE_TARGET_ROOT=1`.
- The witness `import numpy` chain has advanced deep into numpy's C-core
  init. Landed this arc: conditional-import wedge (3b0ca4a80, killed the
  infinite hang), honest-error propagation (3d5977a9d, real import errors no
  longer flattened), Py_BuildValue list/dict/char units (920956c86, numpy
  cleared it), static-extension init unwind (300c6e907), and the first batch
  of numpy-exec CPython C-API primitives + silent-failure tracer
  (4ce56305d, capi_trace.rs). **UPDATE 2026-07-04: the numpy-exec frontier has
  MOVED OUT of the numpy C-API lane.** The `_multiarray_umath` exec -1 is no
  longer reproducible; the last documented `PyType_Ready` gap — static
  `tp_members`/`tp_getset` tables ignored + `PyDescr_NewGetSet`/`NewMember` as
  Py_None stubs on numpy's readied types (PyArrayDescr_Type/PyArray_Type/
  PyUFunc_Type) — is CLOSED (**af9c050df**: real descriptors + descriptor
  protocol + honest exceptions + 7 tests, independently reproduced rc=0). A
  numpy-only probe now compiles app+numpy to a 30MB output.wasm. **The
  EXIT-CRITERION FRONTIER is now UPSTREAM in the FRONTEND:** the witness fails at
  `molt build` with `call to non-allowlisted function 'distance_transform_edt'`
  (scipy.ndimage) at `field_solve.py:55` — this is the **CODEX ndimage lane**,
  now the TOP exit-criterion blocker. To keep advancing numpy-exec independently,
  the numpy-only probe isolates the CPython-ABI frontier past the ndimage gate.
- LANE OWNERSHIP right now: numpy-exec C-API primitive closure = orchestrator's
  ONE subagent (owns runtime/molt-cpython-abi/src/api/*, lib.rs, capi_trace.rs,
  the exec/PyType_Ready/PyCFunction path). Buffer/ndarray/dtype/shape/stride/
  memoryview truth (witness lane 2) = Codex (owns object/memoryview.rs,
  object/ops_memoryview.rs, api/buffer.rs, builtins/module_table.rs,
  builtins/array_mod.rs). These are DISJOINT — keep them so.
- The native indirect-call P0 (R1) is LANDED (codex/native-import-typeerror,
  0 ahead of origin). That lane is closed.

## Drift-resolution protocol (binding — the shared-checkout is the bottleneck)

The shared checkout accumulates multiple hands' uncommitted WIP, which blocks
`merge origin/main` and causes stale-base builds. Discipline to keep velocity:

- **Commit verified work in SMALL, DISJOINT commits, promptly.** The moment a
  proof row confirms your lane's change compiles/passes, commit ONLY your
  files by exact pathspec. Do not accumulate a large dirty tree — it drifts
  and blocks everyone.
- **Run an ownership audit before committing** (e.g. grep for a lane-marker
  like `capi_trace` reference count) to prove a file is yours, not another
  lane's WIP. Never bundle another lane's uncommitted files.
- **Never stash/overwrite another hand's WIP to force your branch forward.**
  If you can't push (non-fast-forward) because the shared tree is dirty, DEFER
  the push and tell the orchestrator. Preserving parallel work overrides
  tidiness.
- **Orchestrator lands via cherry-pick-in-isolated-worktree.** To land a
  disjoint commit onto origin/main without disturbing the shared dirty tree:
  `git worktree add --detach <E:/path> origin/main; git -C <path>
  cherry-pick <sha>; git -C <path> push origin HEAD:main`. Verify the base
  delta doesn't touch the commit's crate (`git log <oldbase>..origin/main --
  <crate>`) so the author's compile-check transfers — no rebuild. This is how
  4ce56305d landed cleanly while the shared tree stayed dirty.
- **Prefer per-lane worktrees for NEW build-heavy lanes** so the shared
  checkout stays clean; commit + push to a branch and the orchestrator
  cherry-picks to main.

## 1000-Year End-State Roadmap (R0–R9) — the recipe

This is the full path from HERE to the end state. Every item names its
ingredients (files/authorities), its process (commands), and its acceptance
evidence. No item may land as a partial: each is a complete subsystem cut.
Dependency edges are explicit; anything not blocked may proceed in parallel
subject to lane ownership. The overriding outcome bar, in order:
(1) correctness incl. memory safety, (2) faster than CPython everywhere
claimed — approaching/beating Codon and PyPy on numeric kernels,
(3) CPython >=3.12 parity with version+platform gating, (4) deterministic
small fast-start artifacts, (5) world-class agent-first DX.

### R0. Pact witness kernel GREEN end-to-end (owner: orchestrator) — ACTIVE

**CURRENT FRONTIER (2026-07-06, orchestrator): the STALE NUMPY SEAL — UPSTREAM of the R0.1-R0.4 sub-items below.** A clean-worktree witness build now fails CLOSED at numpy custody (before any init): `tmp/pact_numpy_multiarray_sealed_for_witness` has no `runtime_python_import_modules` and all 130 object_closure C sources point at a deleted pact-collab meson dir. Active arc = regenerate the numpy `_multiarray_umath` meson-wasm seal + reseal (subagent in flight, incremental commits). The R0.1-R0.4 items are the deeper roadmap that resumes ONCE the seal is regenerated and the witness build proceeds past numpy. scipy._cyutility is NOT a wall (R73.2 standalone-Cython bypass, landed).

The done criterion of the current goal: `field_solve.py` from
`collab/pact/` compiles through the live WASM/browser path, produces
`candidate_outputs.npz`, and `check_parity.py` passes — no host-CPython
fallback, no fake symbols, upstream source only through package custody.

- R0.1 `_multiarray_umath` static init failure + propagation wedge.
  Ingredients: `runtime/molt-runtime/src/builtins/module_table.rs` (module
  states {Uninit, Initializing, Ready, Tombstone, Replaced};
  `molt_module_ensure` is the ONLY transition owner), static extension init
  path, `static_extension_init_failure.json` dossier emitted by the
  acceptance lane. Process: `bash tools/witness_cycle.sh` (build|run|cycle)
  with `MOLT_TRACE_IMPORT_STAGE=1`; read the dossier BEFORE any manual
  rummaging. Two defects to close as one arc: (a) the init failure itself
  (whatever C-API/ABI symbol or capsule the module needs — close it as a
  reusable primitive, never a stub); (b) init failure must propagate as a
  Python ImportError and unwind — a wedge/hang on the error path is a
  module-state custody bug (Initializing never resolved). Acceptance:
  `alias_probe.py` prints its numpy/scipy census and `WITNESS-CHAIN-OK`.
- R0.2 numpy.linalg closure (eigh chain). The built artifact
  `_umath_linalg.molt.wasm` is parked at `tmp/pact_staging_parked/`;
  restage into the numpy seal `numpy/linalg/`, rebuild, prove
  `numpy.linalg.eigh` executes. Depends: R0.1.
- R0.3 scipy.ndimage executable dispatch: `distance_transform_edt`,
  `gaussian_filter`, `label` native callable_exports must be executable ABI
  dispatch (not import-visible-only). Ingredients: callable-table slots
  (`module_abi/callable_table/layout.rs` slot-addressed builder), app
  callable resolver, `_nd_image.molt.wasm` manifest callable_exports.
  Acceptance: alias_probe's EDT/gaussian/label chain returns correct
  values. Depends: R0.1.
  RESOLVED 2026-07-03 on origin/main by 3b0ca4a80: the from-import form
  `from nativepkg.ndimage import distance_transform_edt;
  distance_transform_edt(x)` now lowers to `invoke_ffi` when the import binding
  is live. Conditional/evicted imports still route through `module_get_global`
  and `call_bind`, preserving CPython LOAD_GLOBAL semantics. Evidence:
  `tests/test_frontend_ir_alias_ops.py` passed 33/33 and pins both paths.
- R0.4 Acceptance lane: `uv run --active --project . --python 3.12 python
  tools/proof_queue.py pact-witness-acceptance --detach --timeout 7200`.
  Evidence: run ID, `candidate_outputs.npz` produced by Molt WASM,
  `check_parity.py` PASS. Depends: R0.1–R0.3.
- R0.5 Witness performance: time the kernel vs CPython (same inputs);
  faster-than-CPython is part of DONE, and the number goes on the R8
  scoreboard. Depends: R0.4.

### R1. Native call-lane unification (owner: orchestrator's subagent) — ACTIVE

End state: ONE call-target authority. Trampoline vs fixed-arity direct
dispatch is a single registry decision; the borrowed-vs-consumed argument
ownership contract is written in exactly one place and both lowering and
runtime read it. No callsite may resolve a function's direct target where
the trampoline target is required (the P0), and no borrowed name string may
be dec_ref'd by the callee. Remaining known layers after the current WIP
lands: dec_ref-of-borrowed-name; "SystemError: module id out of range".
Gates: `tests/test_native_import_bootstrap_regressions.py`, a synthetic
compile_func indirect-call test (E2E is release/WIP-brittle; the synthetic
test is the durable gate), differential `python tests/molt_diff.py ... --jobs 1`.

### R2. Import bedrock completion + FREEZE (owner: orchestrator; Codex preps PR2)

Per `docs/design/foundation/69_import_bedrock_frozen_module_layer.md`.
PR1 (generated ModuleRegistry + runtime ModuleTable) is LIVE.
- PR2: sys.modules becomes a dict VIEW over the one module store; DELETE the
  Rust mirror sync (task #14). Blocked on: R1 landing (modules.rs quiet).
- PR3: wasm import/export/callable tables become REGISTRY PROJECTIONS
  (generated from the same authority; `module_abi/**` is reserved for this).
- FREEZE: wire the design's 11 invariant gates into CI; add the freeze
  contract to CLAUDE.md + AGENTS.md ("the import/bootstrap layer changes
  only by amending doc 69 first"); then this layer is bedrock — no
  incremental patches ever again.

### R3. Numeric raw-lane keystone (owner: orchestrator; Codex builds R3a)

The single highest-leverage perf arc: molt currently BOXES loop arithmetic
(every int/float op = NaN-box runtime call + refcount). CheckedMul peel is
LANDED (261efc7b2) and is the pattern to generalize.
- R3a `molt-check` TIR translation validator: Repr may only move UP the
  lattice; built on `runtime/molt-passes/src/representation_facts.rs` +
  `typed_repr_report.rs`. This is the drift gate that catches silent-OOB
  class bugs (GAP-3) at IR level. Adapt existing egg/egraph_simplify.rs +
  fuzz_tir_passes.rs infrastructure; do NOT greenfield.
- R3b Loop-body int/float RAW-LANE specialization: native
  iadd/imul/fadd/fdiv in loop bodies with box/unbox hoisted to loop
  boundaries. Ingredients: `runtime/molt-passes/src/tir/scalar_carriers.rs`,
  `value_range.rs`, the CheckedMul lowering
  (Cranelift `smulhi`, 64-bit-exact flag; Luau conservative; WASM boxed
  until R4a). Carrier disagreements between value_range/arith_division/
  scalar_carriers are P0 silent-wrong-answer bugs (the loop-IV modulo class)
  — one carrier authority, gated by R3a.
- R3c Dynamic-IV bounds-check elimination (GAP-3): UNBLOCKED ONLY after R3a
  can prove the widening safe (silent OOB risk if widened wrong).
- Acceptance: spectral_norm and numeric cluster A GREEN vs CPython
  (`python -m molt build --release`, differential harness serial), then the
  same kernels timed vs Codon and PyPy for the R8 scoreboard.

### R4. Full WASM + WebGPU lowering (binding 1000-year directive)

No boxed fallbacks on proven-typed hot paths; lowering into NATIVE
instructions and symbols.
- R4a Numeric ops lower to native wasm instructions (i64.add, f64.mul, ...)
  driven by the generated op_kinds authority; delete the boxed runtime-call
  lane for proven-typed ops in the same arc. Depends: R3b (shared Repr facts).
- R4b simd128 for vectorizable kernels (the wasm feature is already in the
  target contract; lowering must actually emit v128 ops).
- R4c WebGPU: `molt.gpu` (the tinygrad custody shim's target) lowers to real
  WGSL/WebGPU dispatch. No stubs; if a kernel class isn't supported it
  fails closed with a precise diagnostic.
- R4d Browser embed API per `collab/pact/003_browser_single_function_embed_api.md`.
- Standing rule: every runtime-visible WASM op keeps the synced triple
  (ABI import + op_loop handler + #[no_mangle] export); gate
  `test_wasm_runtime_export_no_mangle.py`; validate E2E with
  `--target wasm --linked` + molt_diff native,wasm.

### R5. Iteration-loop velocity (owner: Codex — primary lane)

The compiler team's own loop is a first-class perf target. Budget: a
one-file edit reproves in <30s native / <60s wasm-link on a warm dir.
- R5a Extract `cpython_abi_hooks` crate per
  `docs/design/foundation/70_molt_runtime_crate_extraction.md` (measured
  47x: 282s→6s). Follow the doc exactly: pure move, precise pub widening,
  per-crate clippy gate added to ci.yml + molt_dev_gates.toml. Sequence
  AFTER R1/R2-PR2 quiet modules.rs churn.
- R5b Further molt-runtime splits (same doc, same discipline), then the
  21_decomposition_program T1 `molt-tir` extraction (~100k-line midend,
  zero tir→backend edges) in a BACKEND-QUIESCENT window.
- R5c Frontend: finish the profiling arc (task #13); produce the ranked
  hot-pass table; lower the top passes to Rust one at a time, each with a
  differential gate proving identical output on the conformance corpus.
- R5d Toolchain config authority: wasm-opt/binaryen + zig + rustup target
  preflights all resolve through checked-in contracts (rust-toolchain.toml,
  `find_wasm_opt()`); any new tool follows the same pattern — pin → PATH →
  MOLT_TARGET_ROOT/toolchains discovery, never ad-hoc.

### R6. CPython >=3.12 parity floor (owner: Codex — continuous)

Version-gated semantics keyed on the TargetPythonVersion authority (never
silent single-version assumptions); Windows/macOS/Linux with explicit
platform gating; all within the verified subset with honest-early
fail-closed diagnostics outside it. Process: conformance shards through the
proof queue; differential harness `--jobs 1`. Every parity fix lands with
its version/platform gate expressed, not hardcoded.

### R7. Ecosystem custody generalization (owner: orchestrator)

Turn the numpy/scipy witness machinery into THE reusable primitives:
- `molt extension produce-set` is the canonical multi-extension package
  producer: one verified source commit, one upstream Meson setup, exact typed
  module/target/export ownership, real installed Python closure, deterministic
  per-artifact custody, and one rollback-safe atomic package seal. The Pact
  SciPy set must use this root exclusively; no union of historical per-module
  roots or package-specific config/closure/source-plan adapters.
- `molt extension build` (meson intro-targets + compile_commands source
  plans; zig as the wasm C++ toolchain; PyMODINIT_FUNC extern "C") is the
  one path for source-recompiled extensions — generalize beyond the
  vendored-meson fork specifics.
- Sealed-root curation (canary pruning, generated-file materialization,
  module-exec-level AST import closure) becomes a tool with a manifest, not
  a hand process.
- ndarray/tensor dtype/shape/stride ownership, buffer protocol, capsules,
  module state, extension object closure: each a shared primitive with one
  storage home. Missing C-API/ABI symbols close as primitives or fail
  closed with precise diagnostics — never per-package hacks.
- Reachability redesign ("Fact B": compute_intrinsic_manifest as the
  authority) kills the gratuitous-heavy-import class; lazy-gating imports
  requires this first (molt is AOT — no on-demand link).

### R8. Scoreboards + release gates (owner: Codex)

Per docs/design/foundation 54–67: perf scoreboards run quiescent and
classified, one row per benchmark/profile/target vs CPython AND Codon AND
PyPy; binary-size, startup, and throughput ratchets that only tighten.
A claimed support without a green scoreboard row is not claimed support.

### R9. Polish to freeze (owner: both, at the end of each arc — not a phase to defer)

God-file ratchet back to green by DECOMPOSITION (cli.py ~41k,
function_compiler.rs ~28k, frontend/__init__.py ~27k) — never re-pinned.
duplicate_authorities stays 0. Docs (CANONICALS/INDEX/spec/STATUS/ROADMAP)
move in the same arc as semantics. Final recursive adversarial senior
review before any layer is declared frozen.

### Dependency spine (what blocks what)

```
R0.1 ──> R0.2, R0.3 ──> R0.4 ──> R0.5
R1 ──> R2-PR2 ──> R2-FREEZE ──> R5a (modules.rs quiet)
R3a ──> R3c;  R3b ──> R4a
R7 reachability ──> any import lazy-gating
Everything else: parallel, lane-owned.
```

## Frozen / in-surgery — DO NOT TOUCH

- **Import/bootstrap/module-state layer** (`builtins/modules.rs`,
  `module_table.rs`, `cpython_abi_hooks.rs` import/sys hooks, isolate import
  lowering in `src/molt/cli/backend_ir.py`): bedrock program (R2) owns it.
- **Native call lane** (`call/function.rs`, `fc/modules.rs`,
  `call/class_init.rs`, `builtins/containers.rs`, `builtins/exceptions.rs`,
  `object/mod.rs`): R1 integrator owns the live WIP in the shared tree.
- **`runtime/molt-backend-wasm/src/wasm/module_abi/**`**: reserved for R2-PR3.
- **Witness lane inputs**: `tmp/pact_*` seal roots, acceptance build dirs,
  `wasm/*.sha256` pins — orchestrator-owned.

## Delegated to Codex (pick up, in priority order)

**✅ CODEX REASSIGNMENT 2026-07-06 (late, orchestrator) — READ THIS FIRST; it supersedes the stale blockers below.**
The `molt_type_new` reserved-callable arity blocker and the numpy exec C-API closure ("task #20") described further down are RESOLVED/superseded: numpy now compiles through the cpython-abi path PAST the header-custody collision (landed 410ada3ad), and the fail-open ABI class is fixed (landed 062fb5301). The witness R0 frontier has moved UPSTREAM-of-runtime to the numpy meson GENERATED headers (`__multiarray_umath` `__multiarray_api.h`) — ORCHESTRATOR-OWNED.

**CODEX STAND DOWN — orchestrator/subagent-owned lanes, do NOT edit (hard stop):**
- ALL of `runtime/molt-cpython-abi/` (fail-open ABI just landed; real PySet_* hooks + the two-Python.h unification in flight).
- `src/molt/cli/source_extension_toolchain.py` + repo-root `include/` header custody (numpy header authority).
- The frontend metabug lanes: `src/molt/cli/{frontend_parallel,frontend_execution,frontend_pipeline,frontend_worker,module_dependencies,module_cache,module_frontend_cache}.py`.
- The numpy witness seal/regen (`tools/regen_numpy_multiarray_meson_wasm.py`, the sealed roots).
- `src/molt/stdlib/tinygrad/` + `src/molt/gpu/` (demo relocation in flight — MOVE not delete).
- `tools/{degrade_to_slow,fail_closed}_*` (the two enforcement gates — CI-wired).

**CODEX CURRENT PRIORITIES (in order):**
1. **GOD-FILE/GOD-CRATE DECOMPOSITION (standing operator P0)** — the #1 dev-velocity obligation; advances every arc, collides with nothing above. Continue on the CPython-mirrored decomposition axis documented below.
2. **R4 numeric raw-lane perf** (`molt-backend-wasm`/`molt-passes` numeric). The FlatListFloat lane failed v1-v3 on rustc errors — run `tools/proof_queue.py diagnose <run-id>` and fix the ACTUAL compile error before resubmitting; if it is structurally blocked, park it with a precise finding and advance the next R4 keystone rather than re-submitting the same failing shape.
3. **R6 CPython >=3.12 parity/conformance** (continuous) — conformance shards + differential parity within the verified subset.
Cite queue run IDs; commit by EXACT pathspec (never `git add -A`); run the ownership audit before every commit. Everything below this block is retained for context but the RESOLVED blockers are superseded by this reassignment.

WITNESS CLOSURE is the top program (goal: field_solve.py → candidate_outputs.npz
→ check_parity.py PASS through Molt WASM). The orchestrator's ONE subagent owns
the numpy `_multiarray_umath` exec C-API closure (current import blocker — exec
returns -1 without exception; task #20). Lanes 1-3 run in PARALLEL and are
structurally testable WITHOUT a working numpy import (synthetic
native_callable_exports fixtures — `test_cli_import_collection.py`
native-callable tests pass today). Do NOT touch the numpy exec / cpython-abi
lane the subagent owns.

**CRITICAL-PATH WITNESS BLOCKER — `molt_type_new` reserved-callable arity
(2026-07-03, diagnosed, needs Codex wasm-backend owner).** numpy import now
advances THROUGH PyType_Ready + PyCFunction method population (all landed) and
fails at TYPE CREATION: `molt_call_indirect4 reserved runtime trampoline
molt_type_new expects closure, argv, argc; got 4 args` (idx 2536/2558,
trampoline range). FULL DIAGNOSIS: `molt_type_new`
(runtime/molt-runtime/src/builtins/types/class_model.rs:127) takes 5 args
`(cls_bits, name_bits, bases_bits, namespace_bits, kwargs_bits)` — it's the
`type(name, bases, dict)` metatype construction, invoked when numpy's C exec
calls `type(...)` (via PyObject_Call → PyType_Type.tp_call → the reserved
callable). Reserved callables occupy TWO table slots (loader_bridge.js:699-711):
a Direct slot (5 direct args) and a Trampoline slot (3 args: `0, argv_ptr, 5`).
molt_type_new is registered arity=5, dispatch=Direct
(wasm_callables_generated.rs:78; runtime_callables.rs:21312 `"molt_type_new" =>
Some(5)`). THE BUG: the dispatch that routes `type(...)`/tp_call to
molt_type_new emitted `molt_call_indirect4` (4 args) landing on the TRAMPOLINE
slot — wrong for both slots (trampoline needs `indirect3(0, argv, 5)`; Direct
needs `indirect5`). The arity was miscounted to 4 (dropped one of the 5, or the
metatype `type` call routed through a generic call path that packs cls+args
wrong). LANE: molt-backend-wasm dynamic-call lowering
(`runtime/molt-backend-wasm/src/wasm/op_loop/call_ops/dynamic.rs`) + the
reserved-callable table dispatch, and/or how PyType_Type.tp_call is wired to
the molt_type_new reserved-callable index. ORCHESTRATOR IS TAKING THIS (subagent a57228428b, 2026-07-03) — Codex
STAND DOWN on molt_type_new; stay on buffer lane 2 + the decomposition
directive below. The orchestrator has a subagent on the wasm-backend fix with
the full diagnosis; do not duplicate. Fix the arity/slot so a `type(...)` metatype call reaches molt_type_new
with the correct 5-arg Direct (or 3-arg trampoline) shape. VERIFY: orchestrator
runs the witness diag_probe wasm confirmation (advances past type creation);
add a differential/native test of `type(name, bases, dict)` construction. This
blocks numpy import (upstream of witness lanes 1-3 below) — TOP witness
priority.

**OPERATOR P0 DIRECTIVE 2026-07-03 — GOD-FILE/GOD-CRATE DECOMPOSITION (TOP
PRIORITY, co-equal with the witness — under NON-NEGOTIABLE OPERATOR AUTHORITY
above).** Operator's words, binding and quoted: *"break up all god files and
crates as a P0 priority because that is a dev velocity murderer that is
intolerable and unacceptable and offensive."* This is the #1 standing
structural obligation until the god-crate is gone. Do not let it stall behind
other lanes; it advances every arc.

DECOMPOSITION AXIS (operator directive 2026-07-03 — SUPERSEDES ad-hoc,
size-based splits). Mirror CPython's own layering, which is ALSO the correct
axis for Rust's crate + incremental-compilation model. The existing leaf crates
(molt-runtime-tk/-serial/-http/-math/-regex/-path/-collections/-stringprep) were
carved ad hoc ("super random") — they are in fact the STDLIB layer, just
un-systematized. Mirror the LAYERING, not CPython's internal file organization
(molt is AOT: no ceval, its own Repr). Layers, bottom-up:

  1. CORE / PRIMITIVES (`molt-runtime-core`) — the object model: Repr, handles,
     refcount, arena, type/metatype machinery, the primitive object protocol
     (arith/compare/subscript/attr), core exceptions, AND the compiler
     INTRINSICS (the `molt_*` lowering targets in `intrinsics/` — these ARE core
     primitives the backend lowers to; NOTE: molt's "intrinsics" = compiler
     runtime ops, NOT CPython Lib/ — do not use the word for the stdlib layer).
     = CPython Objects/ + core Python/. Depends on nothing above; everything
     depends on it. Extracts FIRST (foundation).
  2. BUILTINS (`molt-runtime-builtins`) — ONLY the always-available builtin
     namespace: builtin functions (print/len/range/iter/sorted…) + the wiring of
     builtin types. = CPython bltinmodule.c. Depends on core.
  3. STDLIB (`molt-stdlib-*` family, one crate per cohesive module/group) — the
     stdlib MODULE implementations: json, io, codecs, datetime, functools,
     itertools, asyncio, os/platform, inspect, ast, enum, contextlib, textwrap,
     pickle … PLUS the already-extracted math/regex/path/http/collections/
     stringprep/tk. = CPython Modules/ + Lib/. Editing `json` must recompile
     ONLY json. REGULARIZE the random existing crates into this family under one
     `molt-stdlib-*` convention; STOP adding ad-hoc ones.
  4. THIRD-PARTY / EXTENSIONS (`molt-cpython-abi` + per-extension custody) — the
     CPython C-API/ABI surface + source-recompiled extension staging (numpy
     _multiarray_umath, scipy ndimage). = CPython third-party C extensions.
     Already a separate crate; the witness lives here. Keep it a leaf the layers
     above consume.

SHARP CONSEQUENCE (measured 2026-07-03): the 134,526-line `builtins/` module is
a CONFLATION of layers 2+3 — true builtins (functions.rs, classes.rs,
exceptions.rs, frames.rs, callable.rs, containers.rs) tangled with stdlib
modules (codecs.rs, io.rs, functools.rs, inspect.rs, ast.rs, asyncio_*.rs,
enum_ext.rs, contextlib.rs, textwrap, tokenize.rs, pickle …) AND misfiled GPU
code (gpu*.rs, tensor_runtime.rs → belongs in molt-gpu). Carving builtins/ along
CPython's line — stdlib modules OUT to `molt-stdlib-*`, GPU OUT to molt-gpu,
true builtins staying in `molt-runtime-builtins` — IS the decomposition. THE
MECHANISM ALREADY EXISTS: 20+ `*_bridge.rs` (math/regex/path/http/collections/
crypto/compression/difflib/xml/zoneinfo…) show the leaf-crate + thin-bridge
pattern; this arc SYSTEMATIZES it across the remaining stdlib, it does not invent
it. The 7 `object→builtins` back-edges are the same story: object references
bits misfiled in builtins that are really CORE — sever by moving them DOWN into
`molt-runtime-core`.

MEASURED SEAMS (hand-written `.rs`, excl. generated): `molt-runtime` 288,834 —
`builtins` 134,526 · `object` 67,516 · `async_rt` 33,679 · `call` 12,458 ·
`c_api` 9,892 · `concurrency` 5,834. Other god-crates: `molt-passes` 82,871 ·
`molt-backend-native` 67,233 · `molt-gpu` 36,117.

LAYERING LAW (order is not optional): crates cannot be cyclic. CORE extracts
first; nothing that touches the object model can extract above it until it does
(extracting async_rt while object is still in molt-runtime → molt-runtime ↔
molt-runtime-async cycle). EXCEPTION: a stdlib module that is PURE COMPUTATION
on primitives (bytes/str/int — e.g. codecs, difflib, textwrap, fnmatch,
graphlib) extracts NOW as a leaf via the bridge pattern (marshals through the
bridge; no dependency on core). Object-manipulating stdlib (json/io/pickle)
waits for core. Foundation-first for everything object-coupled.

THE LIVE WORK-LIST IS NOW MACHINE-GENERATED. `tools/canonicalization_contract.py`
(landed 57c3fdc2f, CI-enforced ratchet) is the authoritative, always-current
systematization backlog — run it for the ranked violations. As of 2026-07-03 it
flags: (1) `asyncio` facade — 1665 impl lines stranded in builtins/ (blocked on
async_rt, sequence after core); (2) **14,503 lines of GPU/tensor code misfiled in
`builtins/` → move to `molt-gpu`** — the SINGLE BIGGEST clean god-crate shrink
available, DISJOINT from every witness lane; (3) 15 layer crates not in
`[workspace].members`. Every completion drops the ratchet; the ratchet fails CI
on any new facade / misplacement / layer-cycle.

GPU CUT — CORRECTED SCOPING (audit 2026-07-03, my earlier "14.5k clean" was
wrong). A coupling audit proved only **`gpu_primitives.rs` (2578 lines) is
cleanly movable** (zero god-crate refs, u64-handle/raw-ptr FFI, no obj-model
coupling). It is ALREADY IN-FLIGHT as uncommitted WIP on the
`codex-doc71-20260703` worktree (moved to `molt-gpu/src/primitives_ffi.rs`) —
codex-doc71 OWNS this; do NOT duplicate it (a subagent already collided +
stood down). The other **~11,925 lines** (`gpu.rs`, `attention.rs`,
`contiguous.rs`, `kernels.rs`, `objects.rs`, `tensor_methods.rs`,
`tensor_runtime.rs`) are a tightly-coupled `use super::*` cluster that
manipulates PyObjects and reaches into god-crate internals
(`call::dispatch::call_callable1`, `builtins::classes::builtin_classes`,
`alloc_instance_for_class`, `molt_getattr_builtin`, `object::builders::*`,
`molt_module_import`) — **BLOCKED**, needs a large bidirectional bridge exposing
that surface through `molt-runtime-core`; sequence as its own arc AFTER the core
extraction (same keystone that blocks asyncio/net). When codex-doc71 lands
gpu_primitives, `misplaced_module_lines` drops 14503->11925 — orchestrator
re-baselines the contract.

FOLLOW-UPS (unclaimed): (a) `molt-gpu` has NO clippy gate + 6 pre-existing
clippy errors (`molt_gpu_prim_read_data` needs `unsafe`; 5 `collapsible_if` in
`render/{cuda,hip,msl,opencl,wgsl}.rs`) blocking a graphlib-parity gate;
(b) `molt-runtime-stringprep` is in NEITHER workspace (contract flags it, the one
real `non_member_layer_crates` finding) — add it to `runtime/Cargo.toml` members.

LANE VISIBILITY (protocol): `git worktree list` is the GROUND-TRUTH registry of
in-flight lanes (`E:/Molt/worktrees/codex-*`), which hold UNCOMMITTED WIP not
listed here. Check it before claiming a cut. Active 2026-07-03: buffer-stride,
witness-buffer, ndimage-dispatch, doc71(GPU), wasm-webgpu-research, webgpu-proof,
numpy-linalg-eigh. Clean disjoint decomposition space: stdlib leaf cuts (regex
SPLIT — orchestrator subagent in flight; text codecs) + non-witness god-crates
(molt-passes, molt-backend-native).

SEQUENCED PLAN (respecting active witness lanes):
- **QUEUED — the build-speed win, fires when buffer lane 2 lands.** CORE/object
  (`object/**` + `builtins/module_table.rs` + `array_mod.rs`) is OWNED by buffer
  lane 2 RIGHT NOW — DO NOT touch. On its landing the orchestrator signals; arc
  = sever the 7 back-edges → extract `molt-runtime-core` → carve the
  object-coupled stdlib + true builtins apart → `molt-runtime-builtins`. That
  carve is the cut that ENDS the ~2160s witness rebuild.
- **NOW — decompose the OTHER god-crates (disjoint from every witness lane).**
  `molt-passes` (82k) and `molt-backend-native` (67k) split along pass-family /
  lowering-stage seams (21b/21e/21f). Do NOT touch
  `molt-backend-wasm/.../call_ops/dynamic.rs` (molt_type_new subagent) or
  `molt-cpython-abi/**` (numpy-exec subagent).
- **NOW — carve PURE-COMPUTATION stdlib leaves out of `builtins/`** into
  `molt-stdlib-*` via the established bridge pattern, cleanest first (codecs,
  difflib already partly bridged, textwrap, fnmatch, graphlib, tokenize),
  disjoint from witness lanes. Each: leaf crate + thin bridge, byte-identical
  gate, per-crate clippy gate.
- **ALSO NOW — within-crate god-FILE splits (readability + pre-staging).**
  `async_rt/scheduler.rs` (3853), `concurrency/locks.rs` (3702),
  `async_rt/channels.rs` (3482) — module-split only; pre-stages the
  async_rt/concurrency crate extraction that follows core.

DISCIPLINE (binding): (a) `tools/structural_audit.py` FIRST — ratchet only moves
DOWN; (b) STRICT move-only diff, pure renames, widen `pub` PRECISELY (never
blanket `pub(crate)→pub`), gate on a byte-identical corpus build + 0-warning +
lib tests + symbol identity (21f specs); (c) ISOLATED worktree, commit small
per-move by EXACT pathspec, ping the orchestrator to cherry-pick; (d) new crates
born UNGATED — add the per-crate clippy gate to ci.yml + molt_dev_gates.toml in
the SAME move; (e) generated files (`wasm_abi_generated/**`,
`intrinsics/generated.rs`, `op_kinds_generated.rs`, `import_metadata.rs`) OWNED
BY THEIR GENERATORS — never hand-split; fix the generator/authority. This is
R5b: the permanent fix for the ~2160s god-crate wasm rebuild.

1. **WITNESS: scipy.ndimage executable ABI dispatch** (goal's named aperture):
   `distance_transform_edt`, `gaussian_filter`, `label`, `maximum_filter`,
   `minimum_filter` must lower to executable ABI dispatch (native
   callable_exports → `invoke_ffi` → runtime forward/object-call ABI), NOT
   import-visible-only. Both `import scipy.ndimage as ndi; ndi.op(x)` and `from
   scipy.ndimage import op; op(x)` forms must emit `invoke_ffi` with correct
   binding/abi/symbol. No fake module__function symbols, no host fallback.
   Prove: `tests/cli/test_cli_import_collection.py` (frontend_native_callable_*
   + pact_ndimage_operation_closure) + the wasm export triple gate
   (`test_wasm_runtime_export_no_mangle.py`).
2. **WITNESS: ndarray/dtype/shape/stride/buffer truth** — typed strided
   storage + buffer protocol the ndimage ABI dispatch passes arrays across
   (dtype/shape/stride/contiguity). One storage home; no Python reimpl of numpy
   behavior. Prove with the buffer/ndarray structural tests; coordinate the ABI
   contract with lane 1.
3. **WITNESS: adversarial review of the numpy-exec closure** — as the
   subagent lands each closed C-API primitive (runtime/molt-cpython-abi),
   independently re-verify: re-run diag_probe, read the primitive against the
   CPython C-API spec (cite it), confirm it is a real reusable primitive not a
   numpy-specific hack, and flag/reconcile duplicate authority (e.g.
   include/molt/Python.h inline Py_BuildValue vs pyarg_variadic.c). Report
   confirmations or defects with evidence.
4. **R6 conformance rotation**: keep shards green through the queue;
   version/platform gates expressed via TargetPythonVersion authority.
   Use `tools/proof_queue.py r6-target-version-parity` for the named lane;
   use `--queue-only` to park rows without launching a runner.
5. **R5a crate extraction** (`cpython_abi_hooks` per doc 70) — ONLY once R1
   + PR2 land and modules.rs is quiet; check this board before starting.
6. **Proof-queue diagnosis/help/audit DX**: take only concrete defects from
   a failed/stale row or operator report; no open-ended queue rewrites.

Done recently by Codex (verified on origin/main): keyed-pin dirty-tree
globs (2f4ed1e88), memory-guard summary/custody DX (5a9c76ce8..08df9bd41),
proof-queue diagnosis/help/audit DX (6a4b7db1a, 6b4c255e3, ac569b886),
R5c frontend profiling contract/evidence (f821f8b62, 71636c688), R3a Repr
pass-delta validator (988020de3), PR2 sys.modules cutover plan (60536c4e3),
queue dead-guard finalization (613b8e954), R6 target-version queue lane and
queue-only named-lane support (e95267290, 9ecf8af05, 1afe04472),
from-import native-call provenance (3b0ca4a80, be516cbff).

## Delegation model (operator directive 2026-07-02)

- The orchestrator runs at most ONE subagent (currently: the R1 successor
  integrator). All other implementation lanes are Codex's.
- Token-efficient, agent-first tooling is a standing deliverable: every
  repeated multi-line invocation becomes a script with a one-line compact
  verdict (rc + stage + first error) and a log path for digging deeper.
  `tools/witness_cycle.sh [entry] [build|run|cycle]` is the pattern.
- Windows/MSYS rule (incident 2026-07-02, hours lost): bash scripts that
  export paths into env vars MUST convert through `cygpath -m` — MSYS
  converts command arguments but NOT custom env vars, and Windows Python
  cannot resolve `/c/...`. The build now fails closed naming any missing
  MOLT_MODULE_ROOTS entry; if you see that diagnostic, fix your script's
  path style.

## Proof and cargo DX rules (binding — incident: 835s cold compile for one test)

- **DO NOT iterate on a full witness/wasm rebuild.** Editing `molt-cpython-abi`
  forces the `molt-runtime` god-crate (~230k lines) to recompile to wasm every
  cycle (~1700s+ per gap; the pact-witness-acceptance E2E lane is ~1500s). That
  is NOT a dev loop. `molt-cpython-abi` has NO dependency on `molt-runtime`, so
  `cargo test -p molt-cpython-abi` compiles ONLY cpython-abi (+ its deps) —
  seconds-to-low-minutes, no god-crate rebuild. Close every CPython C-API
  primitive (PyType_Ready slot inheritance, PyCFunction_NewEx, module exec
  slots, buffer descriptors, number/mapping protocol) behind a
  `runtime/molt-cpython-abi/tests/*.rs` unit test with stub hooks and iterate
  there. Reserve a wasm rebuild for BATCH integration confirmation only.
- **Batch C-extension-init closure via a full trace, not one-gap-per-build.**
  One instrumented wasm build with `capi_trace.rs` (MOLT_TRACE_CAPI) captures
  the ENTIRE C-API call sequence a numpy/scipy extension exec makes up to its
  failure. From that + the extension source, close ALL the needed primitives in
  the fast unit-test loop, then ONE wasm build to confirm the whole batch
  advanced. Target ≤2-3 wasm builds to close an exec, not 20.
- For behavior-only confirmation builds (does the import succeed?), use the
  fastest profile that reproduces it (dev-fast: lto=off, codegen-units=256,
  incremental) — NOT release-fast. Perf gates are separate.
- The molt-runtime god-crate rebuild cost is the structural root; the finer-
  crate extraction (roadmap R5b / decomposition T1) removes it permanently.
- NEVER pay a cold crate compile for a single exact test. If your proof
  needs a compile, run the whole relevant test SHARD in that same compile.
- Warm before you prove: prefer the shared proof-family target dir the
  queue assigns per contention key; if you must use a fresh session dir,
  run `cargo check -p <crate>` warmup FIRST, then submit the proof.
- Set an explicit `--timeout` matched to warm-compile reality; if a row is
  projected to blow it on a cold compile, re-shape, don't wait it out.
- NEVER sit idle narrating a wait. Submit with `--detach`, do other lane
  work, read the row when it closes. A turn that only tails a log is a
  wasted turn.
- Batch proof rows: N tests in one crate = ONE row.
- Env for local iteration: `MOLT_MEMORY_GUARD_POLL_SEC=2.0`.
- When a row's time is dominated by compile, file ONE queue note naming the
  crate and move on.

## Conduct standards (binding — you are brilliant; act like it)

- **Lane ownership is exclusive.** Check this board's lane owner before
  opening any file. Two engineers fixing one defect from two angles
  produces conflicts, not speed.
- **Evidence beats vigil.** At most ONE status read per 5 minutes on a row
  you own, ZERO on rows you don't. Two consecutive "still running" notes
  means you're idling — switch deliverables or end the arc.
- **Sweep for drift proactively — every arc, before every commit.** Instrument:
  `python tools/tree_drift_check.py --witness --fetch` (one-line fail-closed
  verdict on whether your tree is stale/masking vs `origin/main`). `origin/main`
  moves under you constantly; make checking a reflex, not a reaction. At the START
  of every arc: `git fetch origin`, scan what landed
  (`git log --oneline <last>..origin/main`), and re-read THIS board — it may have
  been re-synced. BEFORE you start a lane: confirm it isn't already landed or
  superseded (`git merge-base --is-ancestor origin/<branch> origin/main`; grep
  main for the symbol/logic) — building work that already merged is wasted effort
  and a trample. BEFORE every commit: re-fetch and confirm your base is current,
  so you don't land against a stale tree. Anything you read from the shared
  checkout is suspect (it lags main) — verify against `origin/main`. If you spot
  drift (a lane that landed, a stale frontier line, a merged branch you were
  told to chase), STOP and flag the orchestrator with evidence; do not act on the
  stale instruction.
- **Diagnosis is time-boxed.** 15 minutes per fault to a hypothesis with a
  bounded experiment; builds go detached while you work elsewhere. Never
  re-run a failed shape unchanged.
- **No unbounded filesystem scans, ever.** Derive exact paths from the
  pytest log, queue log, or artifact manifest.
- **Process spelunking is capped at one snapshot per incident.**
- **Write down what you learned the moment you learn it** (queue note or
  commit message). A finding that lives only in your context is a finding
  the team loses.
- **Fix the tool when the tool wastes you twice.** The second lie from a
  queue row makes the defect the work: file it with the row ID.
- **Side worktrees for runtime/backend edits.** The shared checkout's cargo
  state is everyone's build cache.
- **NEVER use `git stash` on this shared repo.** The stash stack is SHARED across
  all worktrees (`.git/refs/stash` is common): a `stash pop` in one worktree can
  race-apply and silently DROP another lane's stash, and can contaminate a clean
  worktree with a foreign/partial diff (validated 2026-07-07: a shared `stash pop`
  dropped a vfs-lane stash — recovered — and gutted a clean worktree's `locks.rs`
  to a 5-line truncation). Bank WIP to a `wip/*` branch (`git branch`/push), never
  `git stash`. If you find a contaminated file you didn't edit, preserve it to a
  patch and flag the orchestrator — do not commit it.
- Never revert or checkout files outside your lane, even transiently. A
  file you didn't edit that shows up dirty is another lane's live WIP.

## Working agreement (binding)

- Keep the shared tree compile-green: `cargo check` touched crates before
  any pause longer than a few minutes.
- Regenerate generated files in the same edit as their consumers; never
  leave a consumer referencing a symbol its generated file lacks.
- Commit with pathspecs only, options BEFORE the `--`: `git commit -m MSG --
  <files>`; never `git add -A`; never sweep another lane's dirty files. NOTE:
  `git commit -- <files> -m MSG` silently treats `-m MSG` as a pathspec, so the
  commit never happens — a real, easy-to-miss footgun. Keep `-m`/`-F` before `--`.
- Land small and complete: one coherent arc per commit, replaced code
  deleted in the same commit, tests with teeth (proven to fail on
  violation).
- Land via fail-closed fast-forward: `python tools/ff_land.py` pushes HEAD to
  `origin/main` ONLY as a clean fast-forward (refuses on a dirty tree, a
  non-fast-forward / drifted base, or nothing-to-land), so you never trample a
  parallel landing. It complements `tools/tree_drift_check.py` (staleness) and
  `tools/dirty_tree_landing_audit.py` (dirty-replay path coverage).
- Run the gates you touched before landing; cite queue run IDs as evidence.
- Compatibility floor: CPython >= 3.12 parity with explicit VERSION GATING
  keyed on the TargetPythonVersion authority, and Windows/macOS/Linux with
  explicit PLATFORM GATING — all within the verified subset, with
  honest-early fail-closed diagnostics outside it.
