# APPARATUS FROM COMMA-LAB — harness engineering molt should adopt

**Status:** research synthesis + ranked adoption plan (doc-only landing).
**Sources studied (2026-07-10):**

- GitHub `adpena/comma-lab` @ `1fbcc84ca` (2026-07-10), cloned and read in depth.
  On the operator's `primary` Tailscale host this same repo is live at
  `~/Projects/pact` (remote = `git@github.com:adpena/comma-lab.git`) — "pact" is
  its working name; "comma-lab" the publication name.
- `primary` host (macOS, `adpena@100.81.85.28`, reached read-only via the
  `tertiary` fleet hop): `~/.claude/` (settings, hooks, skills, plugins),
  `~/Projects/pact/.claude/` (the project hook spine), `~/Projects/fmtools`
  (public repo `adpena/fmtools`). Nothing was modified; no secrets copied.
- Three parallel deep-dive subagent reads over the clone (triality machinery;
  costate/PowerPlay/LADDER; .omx memory + hooks + fmtools). File:line citations
  below are against the clone @ `1fbcc84ca`.

**Why this doc exists (operator intent, near-verbatim):** molt needs an
APPARATUS — gates, hooks, guards, a memory DAG — that "helps us learn and
actually remember lessons, gives Claude more capability and memory, and means
the operator doesn't have to remember everything and constantly remind you."
pact encodes months of harness engineering; this doc maps its mechanisms and
gives molt a ranked, concrete adoption plan. Each adoption names the standing
molt directive (M## in `memory/MEMORY.md` / `POINTERS.md`) it mechanizes, so
drift gets caught by tooling instead of operator reminders.

The one-sentence thesis of the whole pact apparatus:
**every operator lesson becomes a mechanical enforcement surface (gate / hook /
guard / registry) with an explicit waiver grammar — never prose.** Its
manifesto (`docs/meta_engineering_vision.md`) states the destination: every
arbitrary constant/config replaced over time, against signal, by a learned or
discovered optimal; ~300 STRICT preflight gates are "structural extinction of
recurring bug classes"; META-meta gates protect the catalog itself from drift.

---

## Part 1 — Mechanism map

### 1.1 The hook spine (the single highest-leverage mechanism)

pact's project `.claude/settings.json` wires three Claude Code lifecycle
points to repo tools (`comma-lab/.claude/settings.json`; verified live on
primary at `~/Projects/pact/.claude/settings.json`):

- **SessionStart** -> `tools/costate_digest.py --session-start` (a read-only,
  fail-open, <5s "sense organ" digest injected into every session: score
  pointer, live run state, latest shadow-controller recommendations, ranked
  lever queue, failure ledger, an explicit ACTUATION BOUNDARY footer) +
  `dashboard_ctl.py ensure-up`.
- **Stop** (every main-agent turn end) -> three detectors, all fail-open,
  loop-safe (guarded by `stop_hook_active` + a persisted `last_block_head`
  marker), event-triggered (silent when no new commits):
  1. `tools/triality_drift_detector.py` — did this turn's commits keep the
     three knowledge representations in sync? (see 1.2)
  2. `tools/magnitude_dismissal_detector.py` — did the agent dismiss a finding
     as "small/weak/noise" without relative-significance math? (see 1.8)
  3. `tools/auto_push_main.py` — auto-push with a three-layer secret scan
     (regex floor -> gitleaks -> local-FM advisory; see 1.9).
- **PreToolUse (matcher: Bash)** -> `tools/launch_guard_hook.py` — refuses a
  raw heavy-trainer launch that bypasses the governed admission-gated path
  (the P0 machine-crash class). Design invariants stated in its docstring:
  FAIL-OPEN (any exception => allow; errors logged to
  `.omx/state/launch_guard_hook_errors.log` with a loud-escalation threshold
  so fail-open is never silent), a PURE unit-tested `decide(command, env)`
  surface, positional execution detection (mentioning the trainer path is
  fine; executing it is gated), and an explicit operator override token
  (`TAC_LAUNCH_GUARD_OK=1`).

A Stop-hook "block" prints `{"decision":"block","reason":"<leg-specific
nudge>"}` and exits 0 — it re-engages the agent at turn end with a firm,
actionable message. It never wedges the session (a Stop hook must never brick;
every detector's last resort is `except Exception: sys.exit(0)`).

**Why this matters more than any single gate:** the hooks make the standing
expectations *ambient*. The agent cannot end a turn, start a session, or run a
dangerous command without the apparatus getting a deterministic look-in. This
is precisely the "operator doesn't have to remind" property.

The user-level `~/.claude` on primary is deliberately thin (one PreToolUse
hook: `rtk-rewrite.sh`, a token-optimizing command-rewrite proxy; plugins:
ralph-loop, hookify, commit-commands). The apparatus lives at project level,
in-repo, versioned — which is the right call for molt too.

### 1.2 Triality: DAG <-> DSL <-> canonical equations (M63's origin)

Design doc: `docs/triality_dag_dsl_equations_deepmath.md`. The campaign is ONE
object viewed through three genuinely different representations:

| Leg | Representation | Lives in | Good at |
|---|---|---|---|
| DAG | trajectory/history | `.omx/research/sub015_DAG_*.md` dated FEED blocks + `[[wikilinks]]` | *what happened* (order, provenance) |
| DSL | executable program | `src/tac/witness_dsl/` (typed lever/gauge factories -> trainer argv) | *what to do next* (compiles intent deterministically) |
| Equations | the law | `src/tac/canonical_equations/` + append-only JSONL registry | *why it works* (confirmed relationships + anchors) |

"A finding is only 'known' when it is expressible in all three and they
AGREE. Drift between legs (a DAG claim with no equation; a DSL flag with no
DAG row; an equation no run produced) is the campaign-level form of
forgetting." Agreement is enforced at five mechanical seams:

1. **Commit-window cross-leg check** — `tools/triality_drift_detector.py`
   (1237 lines, the Stop hook). Scans the git window `last_head..head`
   (marker: `.omx/state/triality_drift_marker.json`); regex-classifies the
   union of commit subjects + changed files; a lever/wire-in/curriculum change
   must touch the DSL leg, a measured/verdict/refuted/ratified change must
   touch the equations leg, and any SUBSTANTIVE commit touching *no* leg is
   drift. Escape valve `[no-triality]` / `[skip-drift]` in the commit subject
   (window-wide), plus a structured per-commit disposition via the commit
   serializer. Additional independent legs each with their own waiver token:
   consumer-wiring (`[consumers-generic]`), recall-evidence ("STORES
   CONSULTED:" line required in decision docs), verdict-scope (below),
   schedule-provenance, DSL-config-bypass.
2. **DSL <-> reality** — `witness_dsl/lever_registry.py::completeness()`
   AST-derives the DSL's emitted flags and reconciles them against the real
   trainer argparse: `unmapped` = coverage gap, `stale` = real drift. No
   hand-maintained registry to rot.
3. **Equations <-> DSL executable bridge** — `witness_dsl/lawref.py`:
   a `LawRef(equation_id, inputs, ladder_class)` compiles a DSL constant *from
   an equation* through a registered pure evaluator, with a value-provenance
   ladder (`derived_live > derived_at_config > measured_anchor >
   hardcoded_waiver`), anchors read from JSON artifacts by key-path with
   optional sha256 + staleness bounds, and fail-closed config-conditionality
   (an anchor tagged for schedule A used in schedule B raises, never silently
   falls back).
4. **DAG/memo <-> equations lint** — `src/tac/preflight.py` "Catalog #344"
   check: an `.omx/research/*.md` memo stating an empirical finding without
   referencing a canonical equation id blocks unless it carries
   `# FORMALIZATION_PENDING:<rationale>`.
5. **Equations <-> empirical reality** — every equation carries
   `EmpiricalAnchor`s with `predicted_output`, `empirical_output`,
   `residual`, provenance grade, optional measured `noise_floor`;
   `is_well_calibrated` requires per-axis residual < 2.0; recalibration
   triggers are declared per equation.

### 1.3 Canonical equations registry (structured findings with teeth)

`src/tac/canonical_equations/equation.py` + `registry.py` +
`.omx/state/canonical_equations_registry.jsonl` (574 events, append-only,
fcntl-locked; latest event per `equation_id` wins). Load-bearing schema
decisions:

- `CanonicalEquation`: `equation_id` (`snake_case_vN`), `one_line_summary`,
  `latex_form`, `python_callable_module_path`, `domain_of_validity`,
  units in/out, `empirical_anchors[]`, per-axis residual map,
  `next_recalibration_trigger`, `canonical_consumers`, `canonical_producers`.
- **Orphan ban**: an equation with no consumers AND no producers is refused at
  construction — structural extinction of "tribal knowledge with no
  machine-readable consumer."
- `EmpiricalAnchor` carries a 4-value verification taxonomy
  (`VERIFIED_VIA_SOURCE_INSPECTION` / `VERIFIED_VIA_EMPIRICAL_ANCHOR` /
  `INFERRED_FROM_DOMAIN_LITERATURE` / `ASSUMED_AWAITING_VERIFICATION`) and a
  noise-floor clearance predicate (`delta_exceeds_floor`).
- 60+ `tools/register_*_equations.py` scripts append events idempotently; each
  per-equation module has a `build_*()` constructor + its own test.

The day-to-day intent (`docs/canonical_equations_tour.md`): when a session
learns something quantitative, codify it in the registry — not in chat, not in
a docstring. The registry is not a score predictor; it is the auditable set of
assumptions downstream consumers reason from.

### 1.4 Costate controller (the marginal-value observer)

Design: `.omx/research/costate_controller_design_20260705.md`. The triality's
missing fourth object is the Pontryagin costate lambda = dS/dx — the measured
marginal-objective shadow price per state channel — flowing measurement ->
decision. Implementation is rigorously ADVISORY:

- `tools/costate_observer_loop.py`: a tiny periodic supervisor spawning
  short-lived `costate_shadow_report.py --write` subprocesses; writes ONLY an
  advisory sidecar (`costate_shadow.jsonl`); self-terminates when no live
  trainer owns the run dir.
- `src/tac/witness_control/shadow_controller.py`: observe -> estimate ->
  recommend -> STOP. Ranked recommendations (ROLLBACK / STOP_OR_RETREAT /
  CONTINUE / INVESTIGATE) by expected-ΔS-per-cost with evidence chains.
  **NEVER-REGRESS**: any candidate whose central predicted effect worsens the
  objective is structurally refused ("it can never rank").
  **Containment is tested, not promised**: `test_no_actuation_capability`
  source-scans the package and asserts zero
  subprocess/os.system/kill/Popen/signal tokens.
- `witness_control/telemetry_binding.py`: the binding-vs-inert audit — is each
  configured lever actually BINDING on the run, or INERT? Includes liveness
  checks on the *instruments themselves* ("a verdict from a corrupted
  instrument is not a verdict"; a stalled sensor reads DETECTOR_STALLED).
- The costate honesty ladder: ANALYTIC -> MEASURED (with propagated stderr) ->
  PARTIAL -> UNIDENTIFIABLE ("honest refusal + the probe that would identify
  it"). Never guess a marginal value.
- Weng-harvest invariants adopted for any future actuation phase:
  **authority-outside-the-loop** (the controller may never touch the
  evaluator/permission/scoring surfaces that judge it — "a controller that can
  touch its own evaluator will Goodhart it"), a diversity floor for
  never-fired levers, and a regression-discipline acceptance template.

### 1.5 PowerPlay (acceptance + ordering law for self-improvement)

`src/tac/witness_dsl/powerplay.py` +
`canonical_equations/powerplay_variant_ii_cost_isomorphism_20260702.py`:
Schmidhuber's PowerPlay (arXiv:1112.5309) adapted as the campaign's acceptance
law, with an exact registered isomorphism (residual 0.0) between the contest
score and a PowerPlay Variant-II cost. Three executable mechanisms:

1. `variant_ii_accept(cost_new, cost_pred, eps)` — admit a modification iff it
   strictly improves the objective with no regression on any held task.
2. `CorrectnessDemonstration.validate()` — fail-closed unless every scored
   quantity was measured through the real authority (an `EvidenceGrade` enum;
   PROXY/ANCESTOR/borrowed numbers always violate), the config demonstrably
   RUNS within resource ceilings, and net objective does not regress. Built
   explicitly as the anti-#205 object (a config once accepted on a borrowed
   ancestor metric with no runnability check, then OOM-died); the test
   reconstructs that exact incident and asserts rejection.
3. `simplest_unsolvable_rank()` — order candidate improvements by expected
   gain per (description + validation) cost; cheapest-to-describe-and-validate
   first.

Crucially the task repertoire and evaluator are FIXED and outside the loop —
PowerPlay's acceptance machinery without open-ended task invention, precisely
to avoid Goodharting ("trivial task invention = our means-as-ends trap").

### 1.6 LADDER (staged, measured promotion)

Three coherent forms, one principle: rungs from easy/coarse to hard/fine;
**promotion fires on a measured condition, never a name and never the clock**
(the epoch is only a fail-safe cap):

- Curriculum homotopy (`witness_dsl/curriculum_dsl.py`): declarative stages
  compile to the real trainer CLI with a structural never-invent-flags guard.
- Island-birth homotopy (`witness_curriculum/ladder_homotopy.py`):
  assist-then-withdraw — start each rung with maximum scaffolding, anneal
  assistance to zero, and gate the assistance on the per-class costate so help
  is withdrawn the moment a sub-case is won (uniform always-on help is a
  MEASURED net-negative anti-pattern); a stagger invariant proves concurrent
  stages compose.
- Maturity ladder L0-L7 (`docs/vehicle_operating_system.md`): every component
  climbs L0 Sketch ("the NAME IS NOT A CLAIM") -> L1 mechanism unit-tested ->
  L2 intrinsically measured -> L3 archive-real (hash-bound) -> L4
  exact-scored -> L5 contextually optimized -> L6 composable (commutator
  measured) -> L7 promotion-ready (paired independent authorities on the SAME
  artifact hash). Hard gates: no long training before L1/L2; no cross-vehicle
  composition before L4. The 8-gate production standard is machine-recorded
  per lane in `.omx/state/lane_registry.json` (`impl_complete,
  real_archive_empirical, contest_cuda, contest_cpu, strict_preflight,
  three_clean_review, memory_entry, deploy_runbook`, each with evidence).

### 1.7 The .omx memory system + graph memory (wikilinks with a reader)

`.omx/` is the durable, resumable state tier (the repo is built for a
Ralph-style loop: work "from disk state, not from fragile chat memory" —
`docs/omx_ralph_runbook.md`). Key stores:

- `project-memory.json` — structured `directives[]` (operator rules with
  priority + date) and `notes[]` (dated append-only observations).
- `.omx/state/*.jsonl` ledgers, all append-only, latest-event-wins:
  `canonical_task_status.jsonl` (task lifecycle with predicted cost /
  predicted-ΔS band / actual-ΔS / commit shas / blockers),
  `review_counter.jsonl` (recursive adversarial review: N consecutive CLEAN
  passes required; a non-clean round RESETS the counter),
  `probe_outcomes.jsonl` (adjudicated verdicts with thresholds, staleness
  windows, expiry — the machine memory of "we already measured this"),
  `harness_failure_ledger.jsonl` (infra forensics with
  hypothesized/falsified causal status), `active_lane_dispatch_claims.md`
  (mandatory claim-before-dispatch ledger), `current_focus.md` (derived
  control-plane snapshot + the canonical score pointer),
  `next_catalog_number.txt` (monotonic Catalog-# allocator).
- **The canonical frontier pointer** (`canonical_frontier_pointer.json`): ONE
  file is the single source of truth for "the best number," structurally
  extincting best-number drift across docs/commits/reports. Every commit
  message states "pointer ... UNMOVED" or the new value — an honesty clause
  parsed by humans and hooks alike (live commits on primary confirm this
  convention is universal).
- **Graph memory** (`src/tac/graph_memory/` + `tools/graph_memory_recall.py`):
  the `[[wikilink]]` corpus (memory notes, DAG FEED blocks, registries) is
  parsed into a typed graph (9 node types, 8 edge types incl. `supersedes`,
  `produces/consumes`, `sister`, `blocks`) with forward+backward adjacency and
  7 typed recall tools (time/keywords/entity/topic/decision/neighbors/
  supersedes). Motivating bug named in the docstring: the goldfish-memory
  class — "apparatus WRITES better than it READS." Recall is reconstructed
  subgraph traversal, not flat retrieval; `obsidian_export.py` re-emits
  synthesized edges as real `[[wikilinks]]`.
- **Catalog #N** (`docs/meta_bug_class_catalog.md`, 813KB): the numbered
  bug-class ledger. Code cites catalog numbers at load-bearing sites
  ("Catalog #229", "Catalog #113") and memory slugs by name
  (memory `reference-apple-ondevice-fm-fmtools-classifier-capability`), so
  every guard traces to the incident that motivated it.

### 1.8 STRICT gates, the warn->strict flip, and the waiver grammar

`src/tac/preflight.py` (87k lines) aggregates ~300 STRICT gates; sister
`confound_gates.py` holds the "DEFAULT-HARMFUL x SILENT x
MEASUREMENT-CORRUPTING" family. The lifecycle discipline molt should copy:

- Gates land **WARN-ONLY** with a **named strict-flip condition** ("flip to
  strict=True once live-count reaches 0"), then get flipped in a separate
  commit. This lets a gate land before all violations are fixed without
  weakening it forever.
- **Uniform waiver grammar**: same-line `# <CLASS>_OK:<rationale>` with
  placeholder rationales rejected (>=4 chars, real text), commit-subject
  tokens `[skip-<gate>]` for window-wide opt-outs, and every waiver logged.
  The design phrase in the code: "ESCAPE VALVE (never binary)".
- **META-meta gates protect the apparatus itself**:
  `check_subagent_contract_module_integrity` verifies the prompt-pattern
  constants exist; the eightfold P1 gate ("ONE FACT, ONE STORE, ONE KEY")
  refuses a significance key that doesn't resolve to a held DSL lever; the P4
  gate ("NO METER WITHOUT A CANARY") requires every measurement class to ship
  a positive control.
- The magnitude-dismissal detector (Stop hook + static sister gate importing
  the SAME pure predicates — one classifier, two surfaces): dismissing a
  finding as "weak/small/noise" requires either relative-significance math
  (ΔS as a fraction of the REMAINING gap to target — an absolute 0.012 that
  is 13-27% of the remaining gap is not "weak") or a cited measurement of
  un-recoverability, or a waiver with rationale.
- The verdict-scope leg: any negative verdict (KILLED/FALSIFIED/REFUTED) must
  declare `verdict_scope: instance|formulation|family|paradigm`; killing a
  FAMILY needs a citation/theorem or >=2 falsified formulations; a kill at
  >=formulation needs a reformulation queue. This mechanizes "fix/kill the
  class, not the instance — but prove which one you killed."

### 1.9 Guards for multi-agent git + push hygiene

- `tools/subagent_commit_serializer.py`: serializes subagent commits through
  an fcntl lock; `git add -- <named files only>` (refuses `-A`/`.`); expected
  content-sha256 per file; a precise rc map distinguishing lock-wait,
  sha-mismatch, sibling-hunk absorption, post-commit HEAD mismatch, and
  sister-checkpoint ABORT/WAIT. The `verify-landing` skill mandates it.
- `tools/auto_push_main.py`: Stop-hook auto-push with layered secret scanning
  — deterministic regex floor (authoritative), gitleaks (authoritative), then
  the fmtools on-device-FM advisory (tighten-only, never authority, fail-open,
  default log-only because the FM false-positives on hex hashes), a durable
  event log, a kill-switch file, and a prompt-echo guard (discard FM findings
  that quote the prompt's own few-shot exemplars — an observed local-model
  hallucination class).
- `src/tac/deploy/claims.py` + the dispatch-claims ledger: lane custody with
  self-retiring terminal statuses (`falsified_`, `retired_`,
  `measured_implementation_retired_`, `stale_assumed_dead`...) — a claim
  carries the seed of its own retirement (the "Godel self-retirement"
  property M64 references), so stale custody cannot silently block a lane.
- `docs/canonical_subagent_pre_flight_checklist.md`: before first Write, every
  subagent runs premise-verification tools (`check_sister_files_recently_landed`
  exit 8 = STAND_DOWN_DUPLICATE / 9 = WAIT_AND_REASSESS) — the duplicate-work
  class killed bidirectionally.

### 1.10 Subagent contract as code

`src/tac/subagent_contract.py` holds the harvested prompting patterns as
NAMED CODE CONSTANTS (`GROUNDED_PROGRESS` — "audit each claim against a tool
result from this session"; `NO_ENDING_ON_PROMISES`; `FINAL_MESSAGE_REGROUNDING`;
`ANTI_GOLDPLATING`; `FRESH_CONTEXT_VERIFIER`). `tools/dispatch_prompt.py`
composes spawn prompts from these constants (re-typed prompts drift), and a
preflight integrity check asserts the constants still exist. Skills
(`.claude/skills/verify-landing`, `witness-status`) encode the verify chain
and honest-status protocol as invocable procedures, with honesty rules inline
("say 'pointer unmoved' if asked about the score"; "list what you did NOT
verify").

### 1.11 fmtools (the local-AI classification pattern)

`adpena/fmtools` (public; live at `~/Projects/fmtools` on primary) wraps
Apple's on-device Foundation Models: `@local_extract` / `@fm.generable()`
typed structured extraction with closed enums (`fm.guide(anyOf=[...])`), a
backend layer (apple_sdk default, FFI optional), caching/debugging/pipeline
surfaces. macOS 26+ / Apple Silicon only — **the specific runtime does not
transfer to molt's Windows/Linux fleet; the integration contract does**:

1. The deterministic heuristic is the always-authoritative FLOOR.
2. The local model is a SUBPROCESS in its own venv (host project gains zero
   deps), invoked with a closed-enum output schema.
3. **Tighten-only**: the model may ADD a hold/flag, never clear a
   deterministic one.
4. **Fail-open**: absent/timeout/bad-JSON => the deterministic verdict stands,
   with an honest "advisory owed" label.
5. Default OFF or log-only so the common path pays zero model cost; operator
   env flag upgrades to enforcement.
6. Every model verdict is labeled advisory and logged durably; guard against
   the model echoing its own prompt exemplars.

Used at: secret-scan semantic layer (auto_push), meter-vs-actuator name
disambiguation (P4 gate), magnitude-dismissal semantic confirm, dashboard
event classification against the real failure-ledger ids.

---

## Part 2 — Ranked adoption plan for molt

Ranking = (drift-classes extinguished x operator-reminder burden removed) /
implementation cost. Each item names the molt artifact to build, the M##
directives it mechanizes, and the existing molt apparatus it composes with.
molt today has NO `.claude` hooks at origin/main — everything below is
additive. All new hooks follow the pact invariants: fail-open + error-log +
loud-escalation, pure unit-tested decision surface, event-triggered,
loop-safe, uniform waiver grammar with mandatory rationale.

### A1. The hook spine (`molt-src/.claude/settings.json` + 3 hook tools) — DO FIRST

**Build:**
- `tools/hooks/session_digest.py` (SessionStart): a <5s read-only digest —
  the goal pointer (witness-closure state toward pact-collab 100-yr green,
  M01), CLAIMS/queue custody snapshot (`proof_queue.py status`, CLAIMS.md at
  origin/main), worktree/drift debt (`drift_harvest.py` count, M67), build
  wall-clock trend (M09), and the top standing directives. This is molt's
  costate digest: the agent starts every session already knowing the state
  it would otherwise be reminded of.
- `tools/hooks/bash_guard.py` (PreToolUse, matcher Bash): pure
  `decide(command, env)`; blocks (a) destructive git against the shared
  checkout — `reset --hard`/`checkout --`/`clean -fd`/`stash drop` unless the
  cwd provably differs from the shared root (M17/M18; lifts
  `tools/git_guard.py` from convention to mechanical), (b) bare
  `git add`-then-commit sweeps (require `git commit -- <pathspec>`, M20),
  (c) raw heavy `cargo build`/`molt build` invocations that bypass
  `proof_queue.py` when a queue is live (M27's <=1-2 builds; the pact
  launch-guard class), (d) pushes to origin over https (M19). Override:
  `MOLT_GUARD_OK=1` in-command + logged rationale.
- `tools/hooks/stop_gates.py` (Stop): runs the A2 landing gate and A3 drift
  detector below; prints `{"decision":"block","reason":...}` on violation.

**Mechanizes:** M12, M16, M17, M18, M19, M20, M27, M67, M01.
**Composes with:** git_guard.py, proof_queue.py, drift_harvest.py,
tree_drift_check.py. **Cost:** small (the pact hooks are direct templates;
port decide() + tests). Note Windows: use `msvcrt`/lockfile in place of
fcntl; keep every hook ASCII-safe and UTF-8-explicit (M43's cp1252 class).

### A2. Land-or-blocker Stop gate (M12 — the operator's #1 fury, mechanized)

**Build:** `tools/hooks/landing_gate.py`, invoked from stop_gates. Window =
session-start HEAD marker .. current HEAD (per-worktree) + proof_queue rows +
a blocker ledger (`.molt/state/blockers.jsonl`, new, append-only). If a turn
produced substantive tool activity but the window shows NO landed commit, NO
queue row in flight, and NO named-blocker entry, emit a block: "This turn
reported without landing. Land a commit/proof/passing test via the queue, or
record the real external blocker in blockers.jsonl, then stop." Escape:
`[report-only]` commit-subject token or a blocker row — both logged.
This is exactly pact's triality-detector *shape* (git-window marker,
fail-open, block-JSON) pointed at molt's most-repeated directive.
**Mechanizes:** M12 (land signal every turn; name real blockers), M05
(a PASS is a hypothesis until reproduced — the gate asks for the artifact).
**Composes with:** proof_queue.py (rows = evidence), ff_land landing flow.

### A3. Molt triality drift detector (M63 made mechanical)

**Status (2026-07-11): COMPLETE.** The commit-window hook is complemented by
`tools/triality.py` and `.molt/state/triality_registry.json`: a lesson is KNOWN
only when DAG, DSL, and equations name the same invariant fingerprint. The seed
set covers content-addressed custody, GLOBAL_BRIDGE serialization,
configured-vs-effective attestation, and exhaustive-by-construction dispatch.

Molt's three legs already exist in nascent form; name them and gate them:

| pact leg | molt leg |
|---|---|
| DAG (FEED blocks + wikilinks) | `memory/MEMORY.md` + topic files + docs/agent ledgers (PROOF_QUEUE, CLAIMS, POISON/PANIC ledgers) |
| DSL (witness_dsl registries) | generated-authority registries: `fc/op_family.rs` single-source dispatch (M39), op_kinds/repr_facts authorities (M54), `fail_closed_registry.toml`, `artifact_poison_registry.toml`, table authorities behind `check_table_drift.py` (M45) |
| Equations (canonical laws + anchors) | gates + attested measurements: fail_closed_gate scans, parity/differential results, bench_evidence attestations (M10) |

**Build:** `tools/triality_gate.py` (Stop-hook leg + CI mode):
commit-window classifier —
- a commit whose subject/diff matches bug-class vocabulary
  (`fail.closed|poison|silent|wrong.answer|truncat|miscompile`) must touch a
  gate, a registry, or a test (the "equations" leg) — else block with "you
  fixed an instance; where is the class extinction?" (M16, M32-M45 pattern);
- a perf-claim commit (`speedup|faster|[0-9.]+x|perf`) must touch
  bench_evidence/PERF_AUTHORITY attestation (M10) — no unmeasured perf claims
  land silently;
- a finding-class commit (`landed|falsified|root.cause|discovered`) must touch
  a memory topic file, a docs/agent ledger, or the A4 findings registry — the
  lesson is recorded where the next session reads it.
Escape valve `[no-triality]` + rationale; window-wide; logged.
**Mechanizes:** M63, M05, M10, M16. **Composes with:** every existing gate
(they become the "equations" leg the detector points commits at).

### A4. Findings registry with empirical anchors (equations layer, molt-shaped)

**Build:** `tools/findings_registry.py` + `.molt/state/findings_registry.jsonl`
(append-only, locked, latest-event-per-id), record shape lifted from
`CanonicalEquation`/`EmpiricalAnchor`:
`finding_id` (snake_case_vN), one-line summary, claim (machine-checkable form
where possible), `domain_of_validity` (targets/OS/py-version — M02's verified
subset!), `anchors[]` (predicted vs measured, residual, authority tier per
M28: `molt build --release` for perf, `molt_diff.py --jobs 1` for parity,
noise floor for bench numbers), `producers`/`consumers` with the ORPHAN BAN
(a finding nobody consumes is refused), verification taxonomy
(VERIFIED_VIA_SOURCE_INSPECTION / VERIFIED_VIA_EMPIRICAL_ANCHOR / INFERRED /
ASSUMED_AWAITING_VERIFICATION), recalibration trigger.
Then two consumers:
- **memo->registry lint** (Catalog-#344 port): a docs/agent/*.md or memory
  topic file stating a measured finding without a `finding_id` reference
  blocks unless `# FORMALIZATION_PENDING:<rationale>`.
- **MEMORY.md graduation**: an M## hook line that encodes a quantitative
  claim should point at a finding_id; `POINTERS.md` gains a findings column.
This is where "actually remember lessons" gets teeth: perf keystones (M46-48),
witness measurements (M55), build-time profiles (M09) become queryable,
recalibratable records instead of prose that silently rots.
**Mechanizes:** M05, M06 (provenance grades + primary-source citation), M10
(before/after attestation), M49 (trace-one-value instrumentation results have
a home). **Composes with:** bench_evidence.py, apparatus_ledger.py,
PERF_AUTHORITY.md, check_perf_freshness.py.

### A5. Local-AI advisory classifier (the fmtools pattern, molt-shaped)

**Status (2026-07-11): COMPLETE.** `tools/advisory_classifier.py` provides the
subprocess-isolated closed-enum contract, prompt-echo firewall, timeout/fail-open
behavior, and append-only advisory event ledger. The fail-closed, magnitude, and
findings consumers expose suggestion-only adapters; deterministic results remain
authoritative.

Apple FM doesn't run on the molt fleet; the CONTRACT does (1.11's six rules).
**Build:** `tools/advisory_classifier.py` — a thin driver that resolves a
local-model backend (env `MOLT_FM_CMD`; e.g. an Ollama/llama.cpp small model
on the Windows box, or nothing) and exposes
`classify(text, schema: closed-enum) -> verdict|None` with subprocess
isolation, timeout, fail-open None, durable event log
(`.molt/state/advisory_events.jsonl`), and a prompt-echo guard. First three
consumers, each keeping its deterministic floor authoritative:
1. **fail_closed_gate ambiguity triage**: Scans A-E regex hits that are
   borderline (rip-poison vs move-demo, M32/M33) get an advisory
   `poison|misplaced_valuable|benign` tag recorded in the gate report —
   tighten-only, never clears a deterministic hit.
2. **Magnitude-dismissal confirm** (with A6).
3. **Findings auto-tagging**: propose `finding_id`/leg placement for new
   memos (advisory suggestion in the lint output, never auto-write).
**Mechanizes:** M33 (poison vs misplaced distinction at scale), M05 (the
advisory can only tighten, so no fake authority). **Composes with:**
fail_closed_gate.py, artifact_poison_gate.py, A3/A4.

### A6. Magnitude-dismissal + verdict-scope guards

**Build:** port `magnitude_dismissal_detector.py` nearly verbatim (its
predicates are already pure + reusable): dismissing a lane/finding as
"small / not worth it / noise / don't re-chase" in commits or docs requires
(a) relative significance against the REMAINING gap to the standing goal
(100-yr witness green; build-time floor M09; parity subset M02), or (b) a
cited measurement of un-recoverability, or (c) `# MAGNITUDE_DISMISSAL_OK:<why>`.
Add the verdict-scope ladder to molt's kill decisions:
`verdict_scope: instance|formulation|family|paradigm` required on any
KILLED/FALSIFIED/WONTFIX in docs/agent ledgers; family kills need >=2
falsified formulations or a theorem; >=formulation kills need a queued
reformulation. This mechanizes molt's recurring "fix the class not the
instance" — and its dual, "don't kill the class on one instance's evidence."
**Mechanizes:** M16, M46/M47's "DON'T re-chase" lines (those are
verdict_scope:instance records!), M11 (objective evidence before
reclaim/retire). **Composes with:** A3 (same Stop-hook window machinery).

### A7. Dispatch prompts + subagent contract as code

**Build:** `tools/dispatch_prompt.py` + `tools/subagent_contract.py`:
named constants for the blocks every molt spawn already needs by convention —
GROUNDED_PROGRESS ("audit each claim against a tool result from this
session"), NO_ENDING_ON_PROMISES, FRESH_CONTEXT_VERIFIER (M05's independent
verification), WEB_AUTHORITY (M06's grant + require WebSearch/WebFetch,
cite primary sources), LANDING_CONTRACT (M12 block), TRIALITY_WIRING (A3
legs), model-tier guidance (M70). Dispatchers compose prompts from constants;
a gate asserts the constants exist (anti-rot). Kills the drift class where
each hand-typed spawn prompt forgets a different clause.
**Mechanizes:** M05, M06, M12, M70. **Composes with:** agent_coordination.py,
ORCHESTRATION.md conventions.

### A8. Lane maturity ladder (L0-L7, molt-shaped) + gate liveness canaries

**Build:** add `maturity` to proof-queue/lane records
(`.molt/state/lane_registry.json`): L0 sketch -> L1 mechanism unit-tested ->
L2 native E2E (differential) -> L3 artifact-real (hash-bound wasm/seal) ->
L4 exact-verified (parity PASS on real authority) -> L5 perf-attested ->
L6 composable (works with concurrent lanes' landings) -> L7 promotion-ready
(paired authorities: Windows native + wasm parity, >=3.12 matrix, M02).
Hard gates: no 10-30min wasm build (M71's cost asymmetry) before L1-L2; no
cross-lane composition before L4; ship only from L7. Enforce in proof_queue
submission (refuse an expensive row for a lane below its rung's floor).
Plus the P4 rule for molt's own apparatus: **no gate without a canary** — a
periodic `tools/check_gate_liveness.py` that runs each registered gate
against a known-bad fixture and fails if the gate no longer fires (molt has
had exactly this class: gates green atop a broken compiler, M42; configured
!= effective, M34 — this is the binding-vs-inert audit pointed at the gates
themselves).
**Mechanizes:** M34, M42, M71, M02, M15. **Composes with:** proof_queue
DAG deps, build_health_gate, degrade_to_slow_gate, molt_dev_gates.toml.

### A9. Warn->strict flip + uniform waiver grammar across existing gates

**Build:** a small convention + one audit tool. Every new molt gate lands
warn-only with a NAMED flip condition recorded in `molt_dev_gates.toml`
(`strict_when = "live_count == 0"`); `tools/check_gate_flips.py` reports
gates whose flip condition is met but which are still warn-only (the ratchet
that ratchets the ratchets). Standardize waivers everywhere:
`# <GATE>_OK:<rationale>` same-line, placeholder-rejected, plus
`[skip-<gate>]` commit tokens; all waivers appended to
`.molt/state/waivers.jsonl` for audit. Molt's ratchets (dead_code_allow,
fail_closed counts, god-file) already have the spirit; this makes the
lifecycle and escape valves uniform so agents stop inventing ad-hoc
overrides.
**Mechanizes:** M32's ratchet discipline generalized; M43 (re-pin vs
decompose becomes an audited waiver, not a silent re-pin).

### A10. Graph memory: make MEMORY.md readable by machine, not just writable

**Build:** `tools/memory_graph.py` — parse `memory/*.md` + `POINTERS.md` +
docs/agent ledgers for `[[wikilinks]]`, M## ids, finding_ids, tool paths;
build the typed graph (nodes: memory/finding/tool/gate/lane/decision; edges:
links/supersedes/produces/consumes/cites); expose recall queries
(`neighbors M12`, `supersedes M52`, `what-consumes finding_x_v1`) and an
Obsidian export. Then wire two consumers so it is READ, not just built:
session_digest (A1) surfaces the 3 nearest memories to the current lane;
the A4 lint suggests link targets. Directly fixes the goldfish class pact
named ("apparatus WRITES better than it READS") — molt's MEMORY.md line
about "a stale line once cost a 30-min detour" is the same bug.
**Mechanizes:** the MEMORY.md/POINTERS.md system itself; M22 (post-compaction
re-grounding gets a query instead of a re-read-everything).

### A11. Claims self-retirement + commit serialization hardening

Molt's CLAIMS.md already has ff-land atomic claims + staleness (M11).
Adopt the two missing pact properties: (a) **terminal-status vocabulary**
(`falsified_`, `measured_implementation_retired_`, `stale_assumed_dead`) so
a claim row can retire ITSELF with evidence, distinct from RELEASED; (b) the
**premise-verification preflight** for spawned agents
(`check_sister_landed.py`: exit 8 STAND_DOWN_DUPLICATE / 9 WAIT — kills the
duplicate-work class M22/M24 bidirectionally); (c) where multi-agent commits
collide on one checkout, port the serializer's expected-content-sha +
named-files-only discipline into agent_coordination.py (fcntl -> msvcrt on
Windows).
**Mechanizes:** M11, M20, M22, M24. **Composes with:** claim_lane.py,
ff_land.py, tree_drift_check.py.

### A12. PowerPlay acceptance for molt's own perf/apparatus changes

**Build:** `variant_ii_accept` + `CorrectnessDemonstration` as a small module
consumed by perf landings and apparatus changes: admit a perf lane only with
(i) measured net improvement on the real authority (release build, serial
differential), (ii) no regression on held benches (never-regress refusal),
(iii) demonstrated runnability within memory ceilings (M26/M27's OOM class),
(iv) evidence grade that refuses proxies (dev-profile numbers, single-run
noise — M28). Order the perf backlog by expected-gain-per-validation-cost
(cheapest-to-validate first) — this is M09/M10's doctrine given an
executable form and a citable name.
**Mechanizes:** M05, M09, M10, M28, M34. **Composes with:** bench_evidence,
check_perf_gate_wiring, degrade_to_slow_gate.

---

## Part 3 — The fmtools note (explicit, per the ask)

fmtools itself (Apple on-device FM, macOS 26+/Apple Silicon, `@local_extract`
/ `@fm.generable` closed-enum structured extraction) will not run on molt's
Windows/Linux fleet. The TRANSFERABLE pattern is the **firewalled advisory
classifier**, and pact has burned in the exact contract (1.11): deterministic
floor stays authority; model in a subprocess with zero host deps; closed-enum
schema; tighten-only; fail-open with an honest "advisory owed" label; default
off/log-only; durable event log; prompt-echo guard. Where molt should apply
it first: (1) poison-vs-misplaced diff triage feeding fail_closed_gate /
artifact_poison_gate reports (M32/M33 is a semantic judgment a regex can only
approximate); (2) magnitude-dismissal semantic confirm on the deterministic
pre-filter's candidates; (3) auto-tagging new findings/memos into the A3/A4
legs as suggestions. Backend options on molt hardware: a small local model
via Ollama/llama.cpp on the NVMe box, or (if the operator prefers zero local
inference) a cheapest-tier API call under the same contract — the contract,
not the model, is the design.

## Part 4 — What NOT to adopt (and why)

- The compression math (xray facets, Lagrangian/Pareto solvers, master
  gradient) — domain-specific; only the *shape* (typed atoms, single signal
  source, derived-not-handpicked constants) informs molt's authority
  registries, which already follow it (M39, M54).
- Full costate CONTROL (Phase B actuation) — pact itself keeps it design-only
  behind operator GO. Molt should stop at the digest + binding-vs-inert audit
  (A1/A8); molt's queue + orchestrator already own actuation.
- The 87k-line monolithic preflight aggregator — molt's many-small-gates
  layout (tools/*.py + molt_dev_gates.toml) is healthier; adopt the LIFECYCLE
  (warn->strict, waivers, canaries), not the monolith. (pact's own god-file
  is a cautionary tale molt's god-file ratchet M43 already guards.)
- Auto-push on Stop — molt lands via ff_land + queue custody; keep landing
  deliberate. Adopt only the layered secret-scan shape if/when an auto-push
  lane is ever wanted.
- omx/Ralph loop macros — Claude-Code-native molt equivalents exist
  (ORCHESTRATOR_GOAL.md + /goal); the durable-state-on-disk PRINCIPLE is
  already molt doctrine (M22, ledgers), the macro layer is not needed.

## Part 5 — Sequencing

Wave 1 (one session, pure additive): A1 hook spine + A2 landing gate + A9
waiver grammar. This alone converts M12/M17/M18/M20/M27 from reminders into
mechanics. Wave 2: A3 triality gate + A6 dismissal/verdict-scope (same window
machinery), A7 dispatch constants. Wave 3: A4 findings registry + memo lint,
A10 memory graph, A8 maturity ladder + gate canaries. Wave 4: A5 advisory
classifier, A11 claims/serializer hardening, A12 PowerPlay acceptance.
Every wave lands with tests for the pure decision surfaces (pact pattern:
`decide()` is pure and unit-tested; the hook wrapper is fail-open) and its
own gate-liveness fixture so the apparatus that catches drift cannot itself
silently rot.
