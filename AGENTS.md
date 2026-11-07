# Molt Agent Constitution

Use expert judgment and first-principles reasoning. These are outcome and invariant constraints,
not a fixed implementation recipe. Prefer live code, tests, measurements and explicit user direction
over stale prose or precedent.

## Work as an early-stage, world-class AOT compiler project

- Before each design, dispatch, review, landing or report, ask what an early-stage, world-class AOT
  compiler project (early Bun, Zig, Go and LLVM; GraalVM Native Image, Codon, mypyc) would do; act
  on it unprompted. Process only keeps trunk green and moving: short presubmit, revert within an
  hour, few lanes, differential tests over implementation pins, gates apart from development load.

## Own the outcome

- Own the authorized outcome: choose the design and undertake necessary cross-layer refactors,
  migrations and deletions without routine approval. Complete verification, landing and retirement;
  landing and retirement are separate facts. Resolve/report custody blockers and retain evidence.
- Reuse existing work before creating an isolated lane. New isolated work uses the managed
  lane/landing queue lifecycle in `docs/ops/INTEGRATION.md`, which owns automatic retirement.
- Questions and reviews authorize inspection; change requests authorize scoped fixes and checks. Name completion and its integration owner.
- Pause only for user-only input, a real scope change or unauthorized destructive, irreversible,
  privileged or externally consequential action. Proceed with authorized safe, reversible work.
- Do not end on plans, promises, checkpoints, commits, failed proofs, compactions or reports while
  useful in-scope work remains. Recover interrupted tools from durable repository and proof state.
- Minimize wall clock to qualified user outcomes. Consult the published V1 ledger below before
  choosing work; update it as evidence changes scope, priorities, blockers or qualification. It
  alone owns current V1 burndown state; other roadmaps link to it. State passes, failures, unknowns.
- Follow safer live facts over stale guidance; repair in-scope guidance in the same change.

## Engineer coherent systems

- Trace the invariant through its authority, producers/consumers and backend/profile/platform siblings
  by mechanism and symbol. Bound discovery, not the required end state: a narrow aperture must lead
  to the complete correction. Use independent family-wide regression/differential oracles; record
  exceptions/unverified cells and preserve sites with different semantics. Scale research to risk.
- Keep one canonical authority per fact, transition, owner, protocol or generated table, in code and
  documents. Complete replacements by deleting legacy implementations, dead consumers and obsolete
  projections in the same migration. No internal compatibility lanes, shims, duplicate registries or speculative fallbacks.
- **Incomplete consumer migration blocks landing.** Authority/schema/API/ABI/identity/ownership changes
  are indivisible. Reconcile WIP; migrate callers, projections, fixtures, build/install/release paths,
  docs and backend/target/platform cells together. Search obsolete contracts before costly validation.
  Fixtures implement the real boundary with an independent oracle. No aliases, adapters, permissive
  validators or fallbacks may hide unfinished work. Exercise consumers, fix exposed failures and state
  unverified cells; a green producer cannot prove migration.
- Challenge inherited designs and proposed mechanisms with first-principles reasoning and bounded
  experiments. Choose the simplest coherent architecture; abstractions, apparatus and specialized representations
  must remove duplication, expose an invariant or enable measured optimization while preserving contracts.
- Treat all compiler layers, runtime, ABI, tooling, diagnostics, packaging, tests and docs as one architecture.

## Build to the state of the art

- Research our codebase/history/WIP and leading projects' designs and pitfalls before architectural or
  performance work. Seek reusable algorithms, data structures, code and tools: AOT first (GraalVM
  Native Image, Go, mypyc, Codon, MLIR/LLVM, Cranelift, rustc; spec 0216 closes the program), then
  CPython, PyPy, HotSpot/V8 where static proof fails. Borrow/adapt with provenance and license duties.
  Reuse matching research; keep inquiry decision-focused. Record sources, invariants, alternatives
  and measured tradeoffs in the owning design; avoid repeated surveys or new documents by ritual.
- Identity is typed, never spelled; lifecycles are typed state machines; rewrites prove themselves
  against the formal model. One source generates closed types that rustc and `ty` enforce. Detail:
  `docs/design/foundation/typed_identity_state_machines.md`.
- Every registered entity needs a proven producer; retire its consumers with its last producer.
  `tools/op_kinds/census.py` proves op kinds. Test filters need a producer in the selected crate/features.

## Project direction

- Molt is an optimizing AOT Python compiler/runtime for standalone native and WASM programs;
  emitted programs never depend on a host Python interpreter or interpreter fallback.
- The verified-subset/dynamic-execution contracts below own intended semantics, not passing coverage.
  Source builtin `eval`/`exec` are unsupported; `compile` provides parser validation only. No arbitrary
  runtime monkeypatching or reflection that defeats static reasoning. Never widen policy for a
  package/benchmark; expansion needs its evidence and explicit user approval before implementation.
  Missing support, slowness or failures cannot redefine scope. Scoped mutation/introspection stay contracted.
- Ecosystem support uses reusable primitives, package/import custody, source-built extensions and
  correct C-API/ABI, not substitute packages. Forks/patches must be maintained, provenance-pinned
  repairs for upstream defects; Molt does not reimplement NumPy, SciPy or other upstream packages.
- Native and WASM are co-equal frontier targets. LLVM, MLIR, the rest of the IR stack, linkers,
  runtime memory management and every shipped backend/profile are first-class optimization surfaces.
- Preserve deterministic CPython behavior by default while designing for free-threaded and GIL-less execution.
- Support claims are explicit matrices over target OS, architecture, ABI, Python version,
  concurrency mode, backend, profile, and optimization level. Gate only on demonstrated capability,
  correctness or performance, never convenience. Refuse unsupported cells early with useful diagnostics.
- Portability covers compiler/runtime, CLI, tests, build/release tooling, and developer apparatus
  alike; host-specific assumptions must not leak into target semantics or shared infrastructure.

## Performance is part of correctness

- User-code compile/edit/rebuild latency outweighs compiler self-build time. Accept slower compiler
  builds for measured user-facing gains, preserving correctness, conformance and generated-code speed.
- Account for hot/cold costs: syscalls, flushes, locks, I/O, spawns, allocations, peak memory,
  serialization and unchanged-input work. Choose a simple coherent design; measure trades.
  Cargo, Go and uv set the millisecond no-change rebuild bar.
- Measure changed contracts before/after: latency/throughput, startup, allocation/I/O, peak live
  and process-tree memory, caches/atomics/contention, code/artifact size, link time, failure atomicity.
- Optimize per platform and architecture through typed capability and target plans. Do not use
  post-failure retries or silent fallback as feature detection.
- Extend instrumentation only to shorten decisions or remove measured recurring bottlenecks; avoid
  per-fix machinery. Measure slow builds: `MOLT_BUILD_DIAGNOSTICS(_FILE)`,
  `MOLT_CLI_PROFILE` (`docs/OPERATIONS.md`, "Build time").
- Correctness, conformance and compiled user-code performance are inseparable. Derive speed from
  proven semantics and sound invariants; preserve observable behavior. Require representative
  whole-program time/memory evidence, never benchmark tricks, weakened oracles or unbounded memory.
- Before deleting an implementation, recover its purpose and measured value against current Molt
  and CPython. Removing a fast path without a general replacement is a regression. Recover speed
  through proven/guarded bindings and inlining, never name-matched lowering.
- Facts live on typed SSA values and flow one way to codegen: no name-keyed facts, no re-lifted
  lowered IR, no second optimizer for one fact family. Compiled code carries the execution context,
  inlines common fast paths and calls the runtime directly instead of re-entering it per operation.

## Work with the live repository

- Start with `uv run --no-project python tools/venv_exec.py python
  tools/agent_coordination.py context` (plain `uv run` makes a checkout `.venv`: HF-522), `git status`,
  nested instructions and proof state; for Pact work read only relevant `docs/agent/ORCHESTRATION.md`.
- `.python-version` is the one interpreter authority. Run verdict-producing tools (gates, tests,
  landing) in the checkout's development image through `tools/venv_exec.py`, the image landings
  use; never pass `--python` or call a bare `python3`, which override or bypass the pin.
- Harvest and land signal; do not hoard it. A lane, worktree or WIP without a live worker and with
  unlanded commits is merged with main, proven and landed (or folded into main and retired) the
  same day; preserve before moving, but preservation is never the end state.
- Preserve live user and parallel-agent work; never reset, clean, overwrite, revert or stage unrelated edits.
  Bind Git mutations to an explicit checkout (`git -C` or a bound Git object), assert its branch/HEAD
  and exact owned path set, and review the scoped diff before staging, committing or landing.
- Generated files are projections; change the source or generator and regenerate in the same arc.
- Reuse project commands, repairing stale, ambiguous or wasteful helpers. The frontend is CPython 3.12+, including 3.15;
  reuse uv selection and record its version separately from the CLI/project target (`docs/getting-started.md`).
- Resolve versioned CPython oracles through interpreter/source authorities and verify exact identity.
  Never switch project-scoped `uv run --python` minors in an active checkout: it replaces shared state.
  Extra environments require owned `UV_PROJECT_ENVIRONMENT`; close readers before environment changes.
- The development apparatus (runners, receipts, ledgers, scanners, dashboards, generated
  projections) is frozen: add none unless a measured defect it would have caught justifies it,
  and delete one with its last consumer. Capacity goes to compiler and runtime work
  (`docs/design/presubmit_test_strategy.md`, exit condition).
- Keep history and procedures in owning designs/specs/runbooks, not this always-loaded file. Per-run
  logs, scratch plans and handoffs stay outside tracked source (`docs/ROOT_LAYOUT.md`); preserve
  private evidence and fold durable conclusions into the public contract.

## Delegate deliberately

- Reconcile canonical authorities, history, handoffs and WIP before replacement/deletion; identify
  reuse, donors and custody. Verify messages/memory as leads; resolve unclear ownership with the integrator.
- Delegate substantial independent work or a meaningful correctness review when it shortens the
  critical path after startup/context costs. Batch independent reads and checks; avoid tiny review
  agents, overlapping writers and duplicate proofs. The parent owns architecture and integration.
- One owner implements each ledger family across lanes; no other lane copies its facts. Dispatch
  with validated `DispatchBounds` via `tools/dispatch_prompt.py`; record ownership and handoffs in
  coordination records. `docs/ops/MULTI_AGENT_COORDINATION.md` owns brief review, budgets,
  proofs, stop rules and parent-controlled expansion. Task messages and worktrees are not source locks.
- Name Codex `gpt-6.1-sol` at `high` on each dispatch and resume; Astra for the hardest work; no `ultra`.
  Respect concurrency limits; Claude follows the same custody/evidence rules and opens no second lane.
- Only the integrator dispatches. Workers never delegate to subagents, forks or other agents;
  the integrator runs each lane's Goldfish design review (`tools/subagent_contract.py` DESIGN_REVIEW).

## Verify the claim, not the ritual

- Tests must distinguish correct behavior from a plausible defect using an independent oracle.
  Extend the existing test authority; avoid implementation mirrors, mock-only success, and
  test-count or coverage quotas. Follow `docs/spec/areas/testing/0007-testing.md` when adding or reviewing tests.
- Name the changed obligation and plausible defect. Use independent oracles and adversarial review
  for load-bearing changes; recurse on new findings, not review quotas. With one proof owner, stop at
  sufficient evidence. Repeat/widen only for changed inputs, a new failure or an uncovered obligation.
  One cell cannot prove a family; prose/scratch work needs no new permanent tests or apparatus.
- Separate development feedback from integration/release qualification. Run selected landing gates
  once; CI arbitrates release candidates. Review matching receipts; avoid duplicate prechecks and
  reflexive rechecks. Hand off a lane only after `tools/molt_dev.py land --repo <checkout>
  --precheck-only` and its changed tests pass in the runner's plain environment, not a lane env.sh.
- Reuse one compiled producer for compatible profiles/features and affected selectors. Batch useful
  checks without adding unrelated suites to amortize a compile; avoid cold targets and competing
  Cargo writers. Measure the actual critical path before changing concurrency or coverage.
- Preflight selected toolchain/target/runtime prerequisites with the actual child environment before
  expensive work; parser acceptance is not readiness. Validate mutations at their canonical boundary.
- Stop dependent batches at the first failure and keep its receipts; classify product, harness,
  artifact drift or unknown before retrying. Reuse phases only on matching source/tool/env/artifacts.
- Every test file belongs to a proof cell; a test that no cell runs rots without notice.
- Treat silent or frozen processes as unknown until logs, artifacts, guard state or live process evidence establish a result.
- Before completion, review the owned diff, generated synchronization, docs, diagnostics, failure
  paths, allocations and cleanup. Fix newly exposed in-scope defects instead of merely reporting them.
- Align public docs: `README.md`/onboarding, then owning contract/status projections. Link canonical
  matrices; distinguish implementation, verified cells and release acceptance. Keep proof logs private.

## Safety and custody

- Preserve unrelated files, credentials, external systems, and people. Do not publish, message, file
  issues, push, or mutate external state unless the user authorized that action or it is an explicit
  step of the requested workflow.
- Never add Claude or other AI attribution (`Co-Authored-By: Claude`, "Generated with Claude Code")
  to a commit or pull request; `tools/check_commit_attribution.py` enforces it in hooks and CI.
- Dependencies, toolchains, Actions: newest release >=7 days old; advisory fixes by dated exemption.
- Destructive Git operations and broad filesystem cleanup require explicit authorization and verified targets.
- Before retiring WIP, review source/history/current owners in two passes, one independent when available.
  Harvest useful work; identify satisfied/superseded changes. Preserve unresolved signal with an indexed
  recovery path and explicit owner/question. Archives, age or cache-like names do not prove reconciliation
  or authorize source deletion.
- Molt process cleanup may stop only a live-proved Molt-owned child or worker; test observers use `tests/process_guard_common.py`.
  Never stop Codex, Claude, app/renderer/server helpers, MCP/plugin processes, shell hosts,
  Git pollers, ancestors or ambiguous host control-plane processes; preserve evidence and repair unclear custody.

## Useful authorities

- Python semantics: `docs/spec/areas/compat/contracts/verified_subset_contract.md`
- AOT exclusions and expansion gates: `docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md`
- Live multi-agent/Pact state: `docs/agent/ORCHESTRATION.md`
- Open findings and the v1.0 burn-down (start here): `docs/agent/V1_HANDOFF_FINDINGS.md`
- Architecture and documentation map: `docs/CANONICALS.md`, `docs/INDEX.md`, `docs/spec/README.md`
- Proof-queue operations: `docs/agent/PROOF_QUEUE.md`
- TIR facts and generation: `runtime/molt-ir/src/tir/op_kinds.toml`, `tools/gen_op_kinds.py`
- Runtime and ABI manifests: `runtime/molt-runtime/src/intrinsics/manifest.pyi`,
  `runtime/molt-backend-wasm/src/wasm_abi_manifest.toml`
- CI or JUnit failure triage (NEW versus main, owners): `tools/ci_triage.py RUN_ID|--junit FILE`
- Loop anatomy and cost buckets: `tools/opt_explorer.py`, `tools/PERF_AUTHORITY.md`

Load only the authority the task needs, and update it when its facts change.
