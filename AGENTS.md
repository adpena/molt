# HIGHEST PRIORITY: UPSTREAM PACKAGE CUSTODY, NEVER REINVENTION

These instructions bind every agent, orchestrator, and subagent before any
other workflow habit. Molt must never reinvent third-party libraries: it
compiles each package's own Python and native extensions, and does not recreate
third-party libraries inside Molt.

- Package build prerequisites resolve automatically through reusable
  package/toolchain custody from upstream metadata/build systems. Cython, Meson,
  Ninja, sysroots, generated headers, native libraries, link flags, feature
  probes, and target-specific build steps are provisioned by shared machinery;
  manual env recipes, hand installs, and one-off witness setup are DX defects.
- Never implement an upstream package's semantics as Molt-owned Python/Rust/C
  package clones, source lists/config headers, symbol tables, ndarray/tensor
  APIs, kernels, or compatibility overlays. Upstream package behavior flows
  through source admitted by package custody, source-recompiled extensions,
  C-API/ABI primitives, typed storage, import/module custody, and generated
  reachability.
- A retained package-semantics clone may exist only as quarantined
  research/reference material under an explicit `.molt-research-quarantine`
  marker. It must not be placed under `src/`, packaged, shipped, added to
  `PYTHONPATH`, `MOLT_MODULE_ROOTS`, module roots, build inputs, import
  resolution, runtime packages, wheels, or compatibility surfaces. The current
  tinygrad reference clone is allowed only at
  `demos/tinygrad/reference_stdlib/`; tests may load it only through
  `tests/helpers/tinygrad_stdlib_loader.py`.
- A missing package primitive is completed as a reusable compiler/runtime or
  package-custody primitive, or it fails closed with a precise diagnostic. It is
  never faked with host-CPython fallback, monkeypatching, vendored forks, baked
  headers, local stubs, plausible fake returns, or copied package
  implementations.
- Harness self-protection is mandatory: `tools/fail_closed_gate.py` rejects new
  ecosystem-baked files, ecosystem build crutches, Molt-owned package
  reimplementations, research-quarantine breaches, fail-open stubs, duplicate
  authorities, and TODO-as-plan surfaces unless a structural-resolution row owns
  deleting the debt. Spawn prompts carry this rule; subagents may not "unblock"
  by reinventing a package.

Related contracts: `docs/agent/AGENTS.full.md` "Ecosystem Compatibility
Doctrine", `docs/design/foundation/73_efficient_builds_toolchain_provisioning_binary_cdn.md`
R73.2, `docs/spec/areas/compat/README.md`,
`docs/spec/areas/tooling/0215_MOLT_EXTENSION_BUILD_PIPELINE.md`, and the
fail-closed registry/gate authority.

# TOP PRIORITY: OWNERSHIP OVER REPORTING

These instructions bind every agent, orchestrator, and subagent before any
other workflow habit. Reporting without follow-up ownership is forbidden for
owned work.

- A finding is not a deliverable. If you discover an issue, bug, regression,
  missing invalidation input, test gap, polish item, stale fake, drift, or
  subagent finding inside your owned lane, you must fix it completely and
  verify it before reporting completion.
- Subagent output is evidence and acceleration, not a handoff endpoint. The
  parent agent owns integration, repair, polish, and verification of every
  actionable subagent finding in the active lane.
- Do not stop at "found X", "reported X", "needs follow-up", "should fix", or
  "recommended next steps" when the fix is inside the repository and the lane is
  yours. Convert the finding immediately into code/docs/tests/proof.
- If a finding is outside your lane, frozen, or externally owned, record the
  exact evidence and owner boundary, then immediately continue the next valid
  structural task in your lane. Do not use the report as a stopping point.
- Before any final status, re-scan the owned diff and proof output for newly
  exposed bugs and polish items. Fix them in the same arc unless doing so would
  violate an explicit lane boundary.
- Completion requires implementation plus verification. A report-only response
  while actionable owned defects remain is a contract violation.

# Molt Agent Contract

This compact file is the always-loaded contract. Full bodies live in
`docs/agent/AGENTS.full.md` and `docs/agent/CLAUDE.full.md`; read the relevant
full sections for non-trivial architecture, compatibility, release, merge, or
handoff work. If compact and full guides conflict, this compact contract
controls until reconciled.

## Non-Negotiables

- Build the end state from the start: one authority per invariant, one storage
  home per value, one import transaction per module-state transition, one guard
  owner per process tree, one typed fact path through frontend, IR, optimizer,
  backend, runtime, tooling, and docs.
- No hacks, no shortcuts, no workarounds, no facades, no compatibility shims,
  no local-minimum patches, no TODO-as-plan, and no partial implementations
  committed as progress. If the abstraction is wrong, move the abstraction.
- POISON - permanently forbidden, binds the orchestrator AND every subagent:
  never bake package code into Molt, add package-specific build crutches,
  duplicate authorities, ship fail-open/stub returns, or land TODO-as-plan.
  Missing primitives become shared structure or fail closed with diagnostics.
  `tools/fail_closed_gate.py` ratchets these classes; every spawn prompt carries
  this rule. If you reach for poison, STOP and move the abstraction.
- A discovered prerequisite is not permission to defer the work, and
  "I will not do X until Y happens" is a turn-blocking avoidance pattern unless
  the formal blocked audit is satisfied. If the prerequisite is inside the owned
  authority class, pull it into the same structural arc and implement the
  end-state path. If it is frozen or owned by another lane, record a precise
  proof-queue/board finding with evidence and immediately continue on the next
  allowed structural aperture. Do not posture as "waiting for X" or land a
  placeholder around the missing primitive. When you catch yourself writing or
  thinking that phrase, immediately convert Y into executable work, a
  proof-queue/orchestrator handoff with evidence, or a named external blocker;
  anything else is a contract violation.
- Deferral language is a self-correction trigger. If you write or think "I am
  not going to", "I cannot do this until", "I will wait for", or equivalent,
  stop the sentence and choose an executable outcome: implement the prerequisite
  now inside the owned lane; create an evidence-backed proof-queue/board handoff
  for a frozen or externally-owned prerequisite and immediately advance another
  valid structural arc; or declare a formal blocked state only after the blocked
  audit threshold is met. Do not use deferral to shrink scope, wait for comfort,
  land substitute code, or narrate status instead of moving the end-state
  structure.
- No backward compatibility inside Molt internals. When a touched path has a
  legacy lane, delete it or structurally reconcile it in the same arc.
- Performance is part of correctness. Claimed support must be deterministic,
  portable, small, fast to start, and faster than CPython on the claimed
  benchmark/profile/target, with honest evidence.
- Verification proves the structural invariant; it must not become progress
  theater. Run the smallest high-signal proof that covers the changed contract,
  then return to structural work.
- Preserve user and parallel-agent work. Start from live `git status`; never
  revert, overwrite, reset, or clean unrelated changes. If a dirty path affects
  the task, work with it and keep signal.

## WASM And Pact Authority

- WASM ABI selectors, runtime callable signatures, import metadata, and reserved
  table slots are manifest/generated authority. Do not create side registries,
  inferred fallbacks, or loader-only selector truth.
- `known_modules` is import visibility. `direct_call_modules` is Python symbol
  link authority. Native callable exports must become executable ABI dispatch,
  not fake `module__function` Python calls.
- Root reserved table slots are runtime-owned. App code must not export,
  override, or infer ownership of those slots from table-ref names.
- Pact WASM witness acceptance is the real `field_solve.py` building and running
  through Molt WASM/browser, writing `candidate_outputs.npz`, then passing
  `collab/pact/pact_witness_kernel/check_parity.py candidate_outputs.npz`.
  Forward-only smoke is not acceptance.

## Structural Work Pattern

- Begin with one narrow named aperture into the real structure: one invariant,
  command family, file cluster, authority surface, or failing execution path.
  The aperture bounds discovery; it is not the implementation scope.
- Once duplicate authority is exposed, rip through the coherent authority class:
  callers, generated facts, backend/frontend/tooling consumers, docs, tests,
  and proof lanes needed to delete the old path.
- Do not burn down one match arm, one audit row, one failing test, or one file
  helper when the evidence shows a shared abstraction. Expand to the whole bug
  class inside the boundary.
- A small landing is valid only when it is a complete end-state subsystem cut
  with no adjacent same-kind duplicate lane left behind.
- If the operator says "tiny slice", "rip it open", or rejects tiny chips, treat
  it as a binding scope override: narrow the aperture, deepen the structural
  rip, and stop defending comfort-sized work.
- Dependency gates are work selectors, not idle states. Never stop at
  "I will not do X until Y happens" unless the formal blocked audit is already
  satisfied. If a lane is waiting on CI, a queue row, a frozen owner, a merge, or
  any other external event, immediately advance another board-valid structural
  arc, pre-stage the next exact patch/proof, file the precise queue/board note,
  or finish a disjoint DX/diagnostic improvement. Waiting is acceptable only as
  detached proof custody or an explicit stand-down/frozen-lane order; it is never
  a reason to narrate status, shrink scope, land a placeholder, or stop moving.
  Rigor, honesty, "not overselling," "measure later," "map before fix," and
  "read-only first, fix later" are NOT licenses to defer: if the prerequisite is
  inside the repo and can be engineered, engineer it NOW in the same arc and do
  the work that GENERATES the evidence, then report it done. Splitting a task
  into analyze/plan and shipping only the analysis is deferral. The tell is any
  self-directed "I won't... / I'll then... / once X lands / gated on Y / then I
  drive / I won't claim until"; the instant you form one, delete it and do the
  complete thing.

## Orchestration And Lane Discipline

- `docs/agent/ORCHESTRATION.md` is the live lane board, owned by the
  orchestrator (Claude, senior engineer). Read it at the start of every arc
  and before every commit. Work only lanes assigned or delegated there; if
  your planned work touches a frozen or reserved lane, pick a delegated lane
  instead.
- The import/bootstrap/module-state layer is governed by
  `docs/design/foundation/69_import_bedrock_frozen_module_layer.md`. Once its
  migration lands, that layer is FROZEN: changes require the design's full
  invariant gauntlet, never incremental patches.
- Keep the shared checkout compile-green between edits: `cargo check` touched
  crates before any pause. Regenerate generated files in the same edit as
  their consumers. Commit with pathspecs only; never sweep another lane's
  dirty files; delete replaced code in the same commit.

## DX, Queue, And Proof Discipline

- Use `uv run --active --project . --python 3.12 ...` for Python commands.
  Non-active `uv run` creates throwaway environments and is not acceptable.
- On this Windows workstation the PRIMARY fast artifact volume is now `C:\Molt`
  (internal NVMe) — set via persistent `MOLT_EXTERNAL_ARTIFACT_ROOTS=C:\Molt` +
  `MOLT_ALLOW_C_DRIVE_ARTIFACTS=1` (2026-07-08; freed by deleting games). It beats
  the external USB `D:` (exFAT: no hard links, 128 KB clusters, metadata-slow) for
  the git+cargo small-file workload. **Do NOT override the artifact root back to
  `D:`/`E:`** — they are exFAT fallback/overflow only. Stale artifacts self-clean
  BY DEFAULT (dx.py auto-janitor, throttled/detached, `--free-below-gb 80`; opt out
  `MOLT_DISABLE_AUTO_JANITOR=1`). See docs/agent/ORCHESTRATION.md DEV-VELOCITY
  PROTOCOL for the current, binding operational rules. Fresh DX/proof-queue builds
  still go through RunContext (`tools/run_context_env.py --prefer-external-artifacts
  --dx`, `tools/throughput_env.sh`, `tools/dev.py`, or the proof queue) so build,
  cache, temp, and managed toolchain paths resolve under the selected root
  (`MOLT_EXT_ROOT=C:\Molt`, `CARGO_TARGET_DIR=C:\Molt\target` — a STABLE persistent
  target; do NOT set `MOLT_SESSION_ID` for ordinary builds). `MOLT_TARGET_ROOT` is
  derived from the selected root; preserve an intentional off-default toolchain
  root only with `MOLT_PRESERVE_TARGET_ROOT=1`. RunContext emits `UV_LINK_MODE=copy`
  for exFAT fallback roots unless an explicit operator value is present.
- Bootstrap RunContext before the first `uv` command in a fresh checkout or
  worktree. Use an already-installed host Python 3.12+ for this dependency-free
  resolver script, for example
  `$dx = python tools\run_context_env.py --prefer-external-artifacts --dx --format powershell; Invoke-Expression ($dx -join [Environment]::NewLine)`;
  then use `uv run --active --project . --python 3.12 ...` for project commands.
  In `--dx` mode this emits a stable `UV_PROJECT_ENVIRONMENT`
  (`tmp/uv-project-envs/dx__py3.12`) rather than a per-process `run-<pid>` env,
  so repeated checks reuse the same uv environment while Cargo output remains
  session-scoped by `MOLT_SESSION_ID`. Use `--session-scoped-uv-project-env`
  only when the uv environment must be isolated too.
  Do not use `uv run` to obtain the first env in a cold checkout, because
  `UV_LINK_MODE=copy` must be present before uv touches `.venv` on
  APDataStore/exFAT.
- Never launch parallel `uv` bootstrap/sync commands against the same fresh
  checkout. One process owns project-environment creation; after it exits,
  subsequent uv commands run with the emitted DX env. If isolation is not
  required, inspect `origin/main:<path>` or reuse the warm canonical checkout
  instead of creating a cold worktree just to read or verify docs.
- `C:\Molt` is the warm source/artifact tier. Molt cache publication must work
  there without rerouting to `D:`/`E:`; if an exFAT fallback root is explicitly
  selected, the backend cache owns the lock+rename/copy fallback. Do not disable
  caching, reroute to legacy volumes, or hand-copy artifacts to work around
  publish errors. Treat any cache publication failure under the selected root as
  a DX defect to diagnose through the cache authority.
- Maintainer/agent git worktrees belong under `C:\Molt\worktrees` when real
  isolation is required; the canonical checkout and landing root is
  `C:\Molt\molt-src`. Never create or use OneDrive worktrees, and do not create
  new `D:\Molt\worktrees` / `E:\Molt\worktrees` lanes. Harvest useful signal by
  reviewed cherry-pick/pathspec landing onto `main`, then delete/prune the
  source worktree, branch, and any temporary bundle; do not let backup piles
  become a second repository. Do not hand-delete build roots; use
  `tools/molt_ssd_janitor.py` (dry-run by default, `--apply` for cleanup) so
  registered worktrees and live sessions stay protected.
- Queue contract and tutorial: `docs/agent/PROOF_QUEUE.md`. Read it before
  queueing or interpreting long-running proof evidence.
- Pact Kernel A acceptance must use the named queue lane
  `tools/proof_queue.py pact-witness-acceptance`. A row that only runs
  `python -m molt build ... field_solve.py` is build evidence, not acceptance;
  current acceptance is `tools/pact_witness_acceptance.py` producing
  `candidate_outputs.npz` and passing `check_parity.py`. Static extension init
  failures in that lane emit `static_extension_init_failure.json`; inspect that
  dossier before manual manifest/source rummaging.
- Expensive or contention-heavy work must go through `tools/proof_queue.py`:
  Cargo builds, WASM/browser proofs, benchmark lanes, conformance shards,
  stress tests, and anything likely to contend for build/runtime resources.
- Cargo proof work must use the queue-native cargo lane:
  `tools/proof_queue.py cargo ... -- <cargo-args>` via the active uv command.
  Do not submit raw `cargo ...` through `exec`, TOML DSL, shell backgrounding,
  or interactive sessions; the cargo lane owns uv, `guarded_exec`, contention
  keys, timeouts, logs, and detached runners.
- Before queueing, run:
  `uv run --active --project . --python 3.12 python tools/proof_queue.py status`
- Submit queued work with a clear `--reason`, `--resource-family`,
  `--contention-key`, `--scope`, and `--note` describing what changed or what is
  being tested/explored and why. Prefer named lanes, the cargo lane, TOML DSL,
  or `exec` over ad hoc background processes. Cite queue run IDs/log/evidence
  paths as evidence.
- For long-running work, use queue-owned detached launch (`tools/proof_queue.py
  cargo ... --detach`, `tools/proof_queue.py exec ... --detach`, or
  `tools/proof_queue.py pact-witness-acceptance --detach`). Do not hand-roll
  `Start-Process`, shell backgrounding, or
  Codex-held interactive sessions for proof custody. Detached launch creates a
  queued row, starts `tools/proof_queue.py run --run-id RUN_ID`, and records a
  `*.runner.log`. If a row was parked with `--queue-only`, launch that exact
  row later with `tools/proof_queue.py run --run-id RUN_ID --detach`; do not
  reconstruct the command or submit a duplicate row unless recording a real
  rerun edge. WASM resource families preflight the checked-in Rust toolchain
  contract and required Rust targets before Cargo starts.
- Queue rows record a git snapshot, append-only notes, append-only acyclic proof
  DAG edges, memory-guard summaries, and deterministic marimo notebook
  projections under `logs/proof_queue/notebooks/`. Append observations with
  `tools/proof_queue.py note RUN_ID --kind observation --note "..."`; do not
  edit/delete note history, rewrite DAG edges, or hand-edit generated
  notebooks. Use `--depends-on RUN_ID` for scheduling dependencies and
  `tools/proof_queue.py link CHILD --parent PARENT --kind reruns --note "..."`
  for post-submit lineage.
- After a failed/stale queue row, run `tools/proof_queue.py diagnose RUN_ID`
  before manual log archaeology. Use `--append-note` to preserve the
  deterministic finding in the append-only proof history. `status` and
  `evidence` surface the same diagnostics; repeated
  `unclassified-failed-proof` is a DX defect that should become a new
  deterministic diagnosis rule.
- If a queue row stalls, inspect the queue log and memory-guard summary. Use
  `tools/proof_queue.py prune-stale` for stale rows; do not kill broad process
  families.
- Treat `write_stdin` as stdin input only, not process control. Never send
  Ctrl-C (`\u0003`), SIGINT-like bytes, ESC/control sequences, or other
  interrupt payloads through it to stop a command. On Windows Codex Desktop the
  unified exec backend can crash with `code=3221225786` and
  `write_stdin failed: Unified exec process failed: process interrupt is not
  supported by this process backend` (`codex_core::tools::router`). Track as
  upstream `openai/codex#30847`; adjacent stale-stdin lifecycle issue:
  `openai/codex#18494`.
- If a command is too broad, noisy, or slow, do not try to salvage it with an
  interactive interrupt. Prefer bounded command timeouts, narrower selectors,
  pytest deselection, proof-queue custody, passive polling until natural exit,
  or exact live-proved Molt-owned PID cleanup with an incident record. Plan
  future long commands so they can finish, timeout, or be owned by the queue.
- Direct commands are acceptable for cheap formatting, static checks, narrow
  source inspection, and queue/bootstrap repair.
- Do not run raw workspace-wide `cargo fmt` for Molt DX or proof cleanup. Use
  `uv run --active --project . --python 3.12 python tools/check_rustfmt.py --changed`
  to check changed human Rust, and add `--write` only when you intend to format
  those human Rust files. `tools/dev.py fmt-check` and `tools/dev.py fmt` route
  through the same authority. Write mode compares `rustfmt --emit stdout`
  before touching files, so already-stable files do not churn Cargo
  incremental state. Checked-in generated Rust is owned by the generator
  `--check` gate that names it; fix the generator or regenerate from authority
  instead of formatting generated files by hand.
- Keep proof scoped to the claim. Broad regrtest, conformance, benchmark, and
  browser lanes are for explicit compatibility/performance/release claims or
  direct user request.

## Crash And Process Custody

- Crash recovery constrains fanout, not ambition: one active structural arc, one
  bounded proof lane, no retry storms, no parallel proof fanout.
- If Codex crashes with the unsupported `write_stdin` interrupt error, classify
  it as a Codex control-plane/backend capability failure, not Molt evidence.
  Preserve the exact error text/screenshot when available, restart from live
  `git status`, inspect queue/guard evidence, and continue with bounded
  commands. Do not retry the same interrupt path.
- Before risky commands, leave or rely on evidence paths under
  `tmp/memory_guard/active/`, `tmp/memory_guard/incidents/`,
  `logs/proof_queue/runs/`, and `logs/agents/codex_stall/`.
- Molt cleanup may target only live-proved Molt-owned build, test, bench,
  backend-daemon, runtime-child, or guard-owned workers.
- Never kill Codex, Claude, the Codex app, renderers, app-server helpers,
  node-repl, MCP/plugin helpers, shell hosts whose ancestry is Codex/Claude,
  Git pollers, ancestors, or ambiguous host-control-plane processes.
- A repo path, process name, stale PID, parent shell, or Codex ancestry is not
  process ownership. If identity is ambiguous, preserve evidence and fix
  custody instead of killing.

## Codebase Authority

- Live code and executable tests are the source of truth. Roadmap, status,
  design, matrix, and memory docs are routing aids until verified against the
  current tree.
- Generated outputs remain generated-only. Update the source data and generator,
  then regenerate; do not hand-edit generated semantic status.
- Update docs in the same arc when supported semantics, backend contracts,
  compiler architecture, compatibility claims, validation gates, or roadmap
  priority move.
- For compiler/runtime facts, prefer generated or shared tables over local
  scans. If a check is needed by CLI setup, diagnostics, validation, closure,
  and docs, route it through one authority.
- Wrapper-name trap: a wrapper is architecture only when it is the thinnest ABI
  entrypoint, import route, or diagnostic boundary over a real authority. A
  wrapper that preserves duplicate execution authority is a bug.

## High-Signal File Map

- TIR/op facts, value-range proof data, and target/profile descriptors: `runtime/molt-ir/src/tir/`,
  `runtime/molt-ir/src/tir/op_kinds.toml`, `tools/gen_op_kinds.py`
- Passes and representation facts: `runtime/molt-passes/src/tir/`,
  `runtime/molt-passes/src/representation_facts.rs`
- Backend-specific lowering: `runtime/molt-backend-native/src/`,
  `runtime/molt-backend-wasm/src/`, `runtime/molt-backend-luau/src/`,
  `runtime/molt-backend-rust/src/`
- Runtime intrinsic/C-API/ABI authority:
  `runtime/molt-runtime/src/intrinsics/manifest.pyi`,
  `runtime/molt-runtime/src/intrinsics/generated.rs`,
  `runtime/molt-cpython-abi/`, `src/molt/_intrinsics.pyi`
- Frontend/lowering: `src/molt/frontend/`, `src/molt/frontend/lowering/`
- WASM ABI and runner surface: `runtime/molt-backend-wasm/src/`,
  `runtime/molt-backend-wasm/src/wasm_abi_manifest.toml`,
  `tools/gen_wasm_abi.py`, `wasm/loader_bridge.js`, `wasm/run_wasm.js`,
  `wasm/browser_embed.js`, `wasm/browser_host.js`
- Process/proof custody: `tools/proof_queue.py`, `tools/memory_guard.py`,
  `tools/harness_memory_guard.py`, `tools/process_sentinel.py`,
  `tools/guarded_exec.py`, `src/molt/backend_daemon_custody.py`
- GPU/tinygrad primitives: `runtime/molt-gpu/src/`
- Docs roots: `docs/CANONICALS.md`, `docs/INDEX.md`, `docs/spec/README.md`,
  `docs/spec/STATUS.md`, `ROADMAP.md`

## Git And Handoff

- Do not force-push, reset hard, checkout over local work, or delete branches
  unless explicitly instructed and safe after reviewing the diff.
- For pact-collab work, origin/main is the final source of truth. Landing
  requires hand-reviewed cherry-pick/merge with no signal loss, no trampling,
  no orphaned branch-only work, and updated handoff docs.
- Before declaring completion, prove the current tree against every explicit
  requirement, artifact, test, gate, performance claim, browser/WASM behavior,
  and handoff deliverable. Treat uncertain evidence as incomplete.
