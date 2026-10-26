# Molt Agent Constitution

Keep these project facts and operating invariants in every agent session. Use judgment:
prefer live code, tests, measurements and explicit user direction over stale prose.

## Own the outcome

- Carry authorized work through implementation, integration, verification and cleanup; act when enough is known.
- Follow through on authorized landings and their post-landing work without waiting for another
  prompt. A confirmed landing and completed retirement are separate facts: resolve or report an
  actionable custody blocker, and preserve the evidence until the integration owner closes it.
- Reuse existing work before creating an isolated lane. New isolated work uses the managed
  lane/landing queue lifecycle in `docs/ops/INTEGRATION.md`, which owns automatic retirement.
- Questions and reviews authorize inspection; change requests authorize scoped fixes and checks. Name completion and its integration owner.
- Pause only for input the user alone can provide, a real scope change, or a destructive,
  irreversible, privileged, or externally consequential action that was not already authorized.
  Proceed with safe, reversible work that follows from the request.
- Do not end on plans, promises, checkpoints, commits, failed proofs, compactions or reports while
  useful in-scope work remains. Recover interrupted tools from durable repository and proof state.
- Ground claims in current evidence; state passes, failures, skips and unknowns. Prioritize the user's
  outcome and measured critical path over activity; close owned WIP before adjacent investigations.
- If project guidance conflicts with the live repository or has gone stale, follow the safer current
  fact and repair the guidance in the same arc when it is in scope.

## Engineer coherent systems

- Before each repair or optimization, map the family: sibling sites across backend, profile and
  platform implementations, upstream producers and assumptions, downstream consumers and exports,
  and lateral analogous sites. Search by failure mechanism as well as symbol spelling.
- Enter through one concrete aperture, then follow the invariant through its complete authority
  class. Bound discovery, not the end state. Prefer one structural correction with explicit
  invariants and family-wide regression or differential tests challenged by adversarial review.
  Record justified exceptions and unverified cells; preserve look-alike sites with different semantics.
- Keep one canonical authority for each fact, state transition, storage owner, protocol, and
  generated table. Move all consumers together and delete the replaced implementation: no internal
  compatibility lanes, shims, duplicate registries, or speculative fallbacks.
- **Incomplete consumer migration is a correctness defect and a landing blocker.** A changed
  authority, schema, API, ABI, identity or ownership rule is one indivisible migration. Reconcile
  existing solutions and preserved WIP first; then trace callers, generated projections,
  fixtures/mocks, build and installation paths, release wiring, documentation, and every applicable
  backend/target/platform cell, and migrate the whole family with its replaced lane. Search for
  obsolete contracts and signatures before expensive validation. Stale fixtures must implement the
  real boundary and keep their independent oracle. Never hide an unfinished migration with aliases,
  adapters, permissive validators or fallbacks. Land only when the affected consumers exercised the
  new contract and its exposed failures are fixed; state unverified cells. A green producer or
  isolated helper cannot prove migration.
- Prefer the simplest direct design. Add abstraction only to remove duplication, make an invariant
  explicit, or enable measured optimization. Systematize families instead of type- or backend-local
  fixes; specialized representations are welcome when they materially help and keep the protocol.
- Treat all compiler layers, runtime, ABI, tooling, diagnostics, packaging, tests and docs as one architecture.

## Build to the state of the art

- Before you design or rewrite a subsystem, find how leading compilers and runtimes solve it
  (MLIR/LLVM, Cranelift, rustc, CPython, PyPy, HotSpot/V8, Codon) and cite them in its design doc;
  a deviation needs a measured reason. When findings share a root cause, fix the cause, even when
  that is a deep structural refactor, and land it as complete per-contract migrations.
- Identity is typed, never spelled. Op kinds, exceptions, classes, decorators and attribute keys
  come from one declarative definition that generates closed types, so the compiler (exhaustive
  Rust matches, `Literal` types under `ty`) rejects a stale consumer. A source-scraping audit is a
  bridge: delete it when the type system enforces the same fact.
- Every registered entity needs a proven producer; retire its consumers with its last producer.
  `tools/op_kinds/census.py` proves op kinds. Test filters need a producer in the selected crate/features.

## Project direction

- Molt is an optimizing Python compiler and runtime, not a reimplementation of NumPy, SciPy, or
  other upstream packages. Ecosystem support comes from reusable compiler/runtime primitives,
  package and import custody, source-built extensions, and correct C-API/ABI behavior. Audited forks
  or vendored patches are acceptable only when they are the maintained, provenance-pinned solution
  to an upstream defect—not a hidden substitute implementation.
- Native and WASM are co-equal frontier targets. LLVM, MLIR, the rest of the IR stack, linkers,
  runtime memory management and every shipped backend/profile are first-class optimization surfaces.
- Preserve deterministic CPython behavior by default while designing for free-threaded and GIL-less execution.
- Support claims are explicit matrices over target OS, architecture, ABI, Python version,
  concurrency mode, backend, profile, and optimization level. Gate only on demonstrated capability,
  correctness or performance, never convenience. Refuse unsupported cells early with useful diagnostics.
- Portability covers compiler/runtime, CLI, tests, build/release tooling, and developer apparatus
  alike; host-specific assumptions must not leak into target semantics or shared infrastructure.

## Performance is part of correctness

- Ask what it costs before you design or write it. State the hot- and cold-path costs (syscalls,
  flushes, locks, bytes written and read, process spawns, allocations, peak memory, serialization
  round trips, work repeated on unchanged inputs) and how each scales, then choose the smallest.
  A cost accepted to remove a larger one is a named, measured trade. Leading tools set the bar: a
  no-change rebuild in Cargo, Go or uv costs milliseconds.
- Measure load-bearing changes before and after using metrics that expose the changed contract:
  latency, throughput, startup, allocations, bytes, peak live memory, process-tree RSS/commit, cache
  behavior, atomics, contention, code size, link time, artifact size and failure atomicity.
- Optimize per platform and architecture through typed capability and target plans. Do not use
  post-failure retries or silent fallback as feature detection.
- Build missing telemetry, profilers, benchmarks, or inspection tools when they materially shorten
  the path to a correct decision; reuse one instrumentation authority. Measure a slow build first:
  `MOLT_BUILD_DIAGNOSTICS(_FILE)`, `MOLT_CLI_PROFILE` (`docs/OPERATIONS.md`, "Build time").
- A local benchmark win does not justify duplicated control paths, weaker determinism, or unbounded
  memory. Record the relevant tradeoffs and gate the policy on reproducible evidence.
- Before deleting an implementation, learn why it existed and measure what it was worth. Removing a
  fast path without a general replacement is a regression: record the numbers against the current
  path and CPython, and recover them through general mechanisms (proven or guarded bindings,
  inlining), never through name-matched lowering.
- Facts live on typed SSA values and flow one way to codegen: no name-keyed facts, no re-lifted
  lowered IR, no second optimizer for one fact family. Compiled code carries the execution context,
  inlines common fast paths and calls the runtime directly instead of re-entering it per operation.

## Work with the live repository

- Start with `uv run --python 3.12 python tools/agent_coordination.py context` (`--json` for its
  versioned model), `git status`, nested instructions and current source/proof state. For active
  multi-agent or Pact work, read the relevant `docs/agent/ORCHESTRATION.md` sections.
- Preserve user and parallel-agent work; never reset, clean, overwrite, revert or stage unrelated edits.
  Bind Git mutations to an explicit checkout (`git -C` or a bound Git object), assert its branch/HEAD
  and exact owned path set, and review the scoped diff before staging, committing or landing.
- Generated files are projections; change the source or generator and regenerate in the same arc.
- Reuse project commands, repairing stale, ambiguous or wasteful helpers. The frontend is CPython 3.12+, including 3.15;
  reuse uv selection and record its version separately from the CLI/project target (`docs/getting-started.md`).
- Resolve versioned CPython oracles through the interpreter/source authorities and verify the exact
  reference identity. Never use project-scoped `uv run --python` to switch minors in an active
  checkout: it replaces the shared environment. Extra environments need an explicitly owned
  `UV_PROJECT_ENVIRONMENT`; close existing readers before changing an environment.
- Keep operational history, incidents, long procedures and tutorials out of this always-loaded file;
  put durable detail in the owning design, spec, runbook, skill, test or live board. Keep per-run
  logs, scratch plans and worker handoffs out of tracked source (`docs/ROOT_LAYOUT.md`); preserve
  private evidence locally and fold durable conclusions into the owning public contract.

## Delegate deliberately

- Before designing, implementing or deleting, search canonical authority, history, handoffs and WIP.
  Report the invariant's owner and what to reuse, reconcile or retire, including preserved donors.
  Verify task messages and memory as leads; ask the integrator about unclear ownership or custody.
- Dispatch bounded, disjoint work through `tools/dispatch_prompt.py` with validated `DispatchBounds`;
  `docs/ops/MULTI_AGENT_COORDINATION.md` owns required outcome, ownership, scope, proof, budget and stop rules.
  Workers return at their deliverable/budget; the parent reassesses the critical path before any expansion.
- Record active worker ownership and handoff state in the coordination records described in
  `docs/ops/MULTI_AGENT_COORDINATION.md`; task messages and worktree names are not source locks.
- Match model and effort: strongest reasoning for architecture/correctness, faster workers for
  bounded scans and mechanical work. Respect operator concurrency limits for all workers; use Astra
  for complex, mission-critical or long-running reasoning. Claude Fable/Opus follows the same search,
  ownership and evidence discipline for independent design/review, without a second integration lane.

## Verify the claim, not the ritual

- Tests must distinguish correct behavior from a plausible defect using an independent oracle.
  Extend the existing test authority; avoid implementation mirrors, mock-only success, and
  test-count or coverage quotas. Follow `docs/spec/areas/testing/0007-testing.md` when adding or reviewing tests.
- Prove the changed contract with one proof owner; widen only for the actual claim. A sufficient
  narrow proof needs no broad suite; one green cell cannot prove a family. Review matching receipts
  instead of reproducing every PASS. Reruns need changed inputs, a new failure or a named uncovered
  obligation. Run required integration gates once; skip duplicate prechecks, benchmarks and proof fanout.
- Preflight selected toolchains, targets and runtime prerequisites through their owners with the full
  child environment before expensive work; parser acceptance alone does not prove readiness. Validate
  mutation targets canonically and turn mistakes into checks at that boundary, without duplicate gates.
- Stop dependent command batches at the first failure. Classify product, harness, artifact drift or
  unknown before retrying; preserve failed receipts. Reuse completed phases only when source, tool,
  environment, workload and artifacts match; rerun only missing or invalidated obligations.
- Run the owning proof families locally before pushing; CI arbitrates release candidates. Make
  repeated manual triage, profiling or log pipelines a tested tool and point to it below.
- Every test file belongs to a proof cell; a test that no cell runs rots without notice.
- Treat silent or frozen processes as unknown until logs, artifacts, guard state or live process evidence establish a result.
- Before completion, review the owned diff, generated synchronization, docs, diagnostics, failure
  paths, allocations and cleanup. Fix newly exposed in-scope defects instead of merely reporting them.
- Align public docs in the same change: `README.md` and onboarding, then owning contract/status
  projections. Link canonical matrices; distinguish implementation, verified cells and release
  acceptance. Keep local proof history out of public overviews.

## Safety and custody

- Preserve unrelated files, credentials, external systems, and people. Do not publish, message, file
  issues, push, or mutate external state unless the user authorized that action or it is an explicit
  step of the requested workflow.
- Never add Claude or other AI attribution to a commit or pull request: no `Co-Authored-By: Claude`
  trailer and no "Generated with Claude Code" footer. `tools/check_commit_attribution.py` enforces
  this in the `commit-msg` hook and in CI (the `repository_policy` proof-plan family).
- Dependencies, toolchains, Actions: newest release >=7 days old; advisory fixes by dated exemption.
- Destructive Git operations and broad filesystem cleanup require explicit authorization and verified targets.
- Before retiring old WIP, make at least two passes through its source, history and current owners,
  including an independent pass when available. Harvest and land useful changes through current
  authorities; identify what is already satisfied or intentionally superseded. Preserve unresolved
  signal with an indexed recovery path and explicit owner or question. An archive alone is not
  reconciliation, and age or a cache-like name is not evidence that source can be deleted.
- Molt process cleanup may stop only a live-proved Molt-owned child or worker; test observers use `tests/process_guard_common.py`.
  Never stop Codex, Claude, app/renderer/server helpers, MCP/plugin processes, shell hosts,
  Git pollers, ancestors or ambiguous host control-plane processes; preserve evidence and repair unclear custody.
- Before a risky or long-running proof, leave a recoverable command/cwd/status/ evidence capsule
  under the established guard/incident machinery. Prefer detached project custody for expensive or
  contention-heavy work; see `docs/agent/PROOF_QUEUE.md` when that machinery is actually needed.

## Useful authorities

- Live multi-agent/Pact state: `docs/agent/ORCHESTRATION.md`
- Open findings and the v1.0 burn-down (start here): `docs/agent/V1_HANDOFF_FINDINGS.md`
- Architecture and documentation map: `docs/CANONICALS.md`, `docs/INDEX.md`, `docs/spec/README.md`
- Proof-queue operations: `docs/agent/PROOF_QUEUE.md`
- TIR facts and generation: `runtime/molt-ir/src/tir/op_kinds.toml`, `tools/gen_op_kinds.py`
- Runtime and ABI manifests: `runtime/molt-runtime/src/intrinsics/manifest.pyi`,
  `runtime/molt-backend-wasm/src/wasm_abi_manifest.toml`
- CI failure triage (NEW versus main, ledger owners): `python3 tools/ci_triage.py RUN_ID`
- Loop anatomy and cost buckets: `tools/opt_explorer.py`, `tools/PERF_AUTHORITY.md`

Load only the authority the task needs, and update it when its facts change.
