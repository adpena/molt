# Molt Agent Constitution

Use expert judgment and first principles within these outcome and invariant constraints. Prefer
explicit user direction and verified live code, tests and measurements to stale prose or precedent;
repair in-scope guidance. `AGENTS.md` is standing policy; `CLAUDE.md` remains `@AGENTS.md`.

## Operate like an early-stage AOT compiler project

Start with the limiting user outcome and trace it across the pipeline. Correct the shared
invariant or missing fact that removes the most work. Use a failing case or benchmark to expose
the mechanism and a distinct case to check it. Keep trunk green and moving: short presubmit,
revert within an hour, few lanes, differential tests over implementation pins, release gates apart
from development load. Add process only for a measured bottleneck.

## Bind the work and finish the outcome

- Bind startup to the assigned absolute checkout, branch, HEAD and dirty/owned paths: inspect
  `git -C <checkout> status --short --branch`, `git -C <checkout> rev-parse HEAD`, nested instructions
  and the relevant claim or proof receipt. For unassigned work or repository-wide reconciliation,
  use `uv run --no-project python -B tools/venv_exec.py python tools/agent_coordination.py context`;
  it inspects every registered worktree. Read only task-relevant authorities; consult the published
  `docs/agent/V1_HANDOFF_FINDINGS.md` before choosing work. It alone owns V1 burndown, priorities,
  blockers and qualification; update it when evidence changes them. Other roadmaps link to it.
- Reconcile assignments, WIP/history/handoffs and donors before replacement. Messages/memory and
  path-only records are leads, not locks: verify source, current owner and running-command state.
  Staleness grants no write permission; resolve ambiguous/live matching ownership with the integrator.
- Questions/reviews authorize inspection; change requests authorize scoped fixes/checks. Name outcome
  and integrator. Choose needed cross-layer refactors/migrations/deletions without routine approval;
  finish verification, landing and retirement while useful authorized work remains. Recover interrupted
  tools from durable source/proof state. Pause only for user-only input, scope change or unauthorized
  destructive, irreversible, privileged or external action.
- Fix encountered defects, including pre-existing ones, or assign an owner and same-day slot.
  Reuse existing work; managed lanes, canonical landing and automatic retirement belong to
  `docs/ops/INTEGRATION.md` and `tools/molt_dev.py`. Harvest unlanded work without a live worker
  into main, prove and land it or fold it into main and retire it the same day. Preservation precedes
  movement and is not the endpoint; landing and retirement are separate observed facts.
- Review diff, generated synchronization, docs/diagnostics, failure/resource paths and cleanup before
  completion. Report behavior, executed evidence/identities, failures/unknowns/unverified cells, ledger,
  remaining obligations and observed landing/retirement. Before rescheduling partial work, check its
  current owner, retained evidence and closure against the ledger.

## Preserve the Python AOT contract

- Molt compiles standalone native/WASM programs, never depending on host Python or interpreter
  fallback. Cranelift is the primary development and JIT backend; LLVM is the primary release
  backend and emits WASM. Native and WASM name targets. Share optimization and ABI semantics
  across backends; keep target-specific choices in lowering. GPU work is experimental. Every IR
  layer, linker, memory manager and shipped backend/profile is an optimization surface. Portability includes all tooling,
  tests and developer apparatus; host assumptions cannot enter target semantics/shared infrastructure.
- `docs/spec/areas/compat/contracts/verified_subset_contract.md` owns intended Python semantics;
  `docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md` (0216) owns exclusions.
  Source `eval`/`exec` are unsupported; `compile` only validates parsing. No arbitrary runtime
  monkeypatching or reflection defeating static reasoning; scoped mutation/introspection retain
  their contracts. Missing support, slowness or failing tests cannot shrink scope. Expansion needs
  evidence and explicit user approval before implementation; packages/benchmarks grant no exception.
- Support upstream ecosystems through reusable primitives, package/import custody, source-built
  extensions and correct C-API/ABI. No substitute NumPy/SciPy or other package reimplementations;
  forks/patches are maintained, provenance-pinned repairs for upstream defects.
- Never rewrite Python in Python: compile CPython's pinned Python stdlib sources, patched only
  where the verified subset requires; C modules/hot paths use Rust intrinsics and thin Python glue.
  The stdlib participates in whole-program pruning, specialization and inlining, not a prebuilt
  library. Per-module summaries preserve rebuild speed; pure bodies initialize an immortal image
  heap at build time; target facts are constants. The exact plan is
  `docs/spec/areas/compat/plans/stdlib_lowering_plan.md`, “Closed-world stdlib (architecture)”.
- Preserve deterministic CPython behavior by default while designing for free-threaded/GIL-less
  execution. State support as matrices over OS, architecture, ABI, Python version, concurrency,
  backend, profile and optimization level. Gate on demonstrated capability/correctness/performance,
  never convenience; reject unsupported cells early with useful diagnostics.

## One architecture, complete migrations

- Trace invariants by symbol/mechanism through authorities, consumers and backend/profile/platform
  siblings. Bound discovery, not correction; preserve different semantics, report class-sweep siblings,
  exceptions/unverified cells and use independent family-wide oracles. All layers form one architecture.
- Keep one authority per fact, transition, owner, protocol or table. **Incomplete consumer migration
  blocks landing:** reconcile WIP and migrate callers, generated projections, fixtures, build/install/
  release paths, docs and target cells together. Delete replaced implementations, dead consumers
  and obsolete projections in the same migration. Search obsolete contracts before expensive validation. No internal compatibility
  lanes, shims, duplicate registries or speculative fallbacks. No aliases, adapters or permissive
  validators may hide unfinished migration. Exercise real consumers; a green producer is insufficient.
- Define operation semantics once in late-TIR primitives every backend lowers, otherwise one
  `molt-codegen-abi` generator. Backends select instructions only; migrate duplicate lowering and
  build each backend with its feature enabled.
- Research code/history/WIP and leading designs before architecture/performance work: AOT first
  (GraalVM Native Image, Go, mypyc, Codon, MLIR/LLVM, Cranelift, rustc), dynamic systems where static
  proof fails. Reuse matching research; record sources/licenses, invariants, alternatives and measured
  trades in the owning design. Choose the simplest coherent architecture; bounded experiments
  challenge precedent. Abstractions must remove
  duplication, expose invariants or enable measured optimization; scale inquiry to risk, avoid ritual.
- Identity is typed, never spelled; lifecycles use typed state machines; rewrites prove against the
  formal model. One source generates rustc/`ty`-enforced closed types:
  `docs/design/foundation/typed_identity_state_machines.md`. Runtime entries, callees, globals,
  attributes and representations travel as typed IDs, slots, offsets or guarded code addresses;
  strings serve reflection/messages. Spelled-identity sites only decrease. Apply “Typed facts at the
  gate” in `docs/design/presubmit_test_strategy.md` to signatures and fact representation.
- Every registered entity needs a proven producer; retire consumers with its last producer.
  `tools/op_kinds/census.py` owns op-kind proof; test filters need a producer in selected crate/features.
  Generated files are projections: change their source/generator and regenerate in the same arc.

## Performance is correctness

- Correctness, CPython conformance and compiled performance are inseparable. Preserve observables
  with proven semantics and representative whole-program time/memory evidence; no benchmark tricks,
  weakened oracles or unbounded memory. The bar is equivalent C
  (`clang -O2`, within 2x instructions per loop iteration); CPython is semantic oracle and floor.
  Work above that bar may land as progress, with its ledger row open.
- Read stage status in `docs/design/foundation/call_anatomy.md` before performance analysis,
  ordering or speed claims. Whole-program facts no code can change are compile-time constants:
  proven sites emit the instruction, fixed-offset load or direct compiled call, with no runtime
  call to find/check/bind it. Guards/caches/site records require a possible writer named in the
  diagnostic. A cheaper runtime entry only helps unproven sites. Measure instructions retired
  and executed runtime calls per iteration before/after; static call-site counts include cold paths.
- Carry facts on typed SSA values one way through lowering to codegen: no name-keyed facts,
  re-lifted IR, shape re-derivation or second optimizer for one fact family. A refusing pass diagnoses
  lost facts; no silent generic-runtime degradation. Compiled code carries execution context and
  inlines fast paths, leaving runtime work cold where static proof is unavailable.
- Prove storage, bounds and ownership at the widest valid region in shared IR. Identify edges
  that invalidate facts; preserve the exact continuation, committed effects and observable frame
  state. Measure the emitted hot path before moving a check into a backend.
- User compile/edit/rebuild latency outweighs compiler self-build time; accept measured user gains
  without losing correctness/conformance/generated speed. Treat preparation as a dependency graph:
  carry verified compiler/runtime/object/link identities, key reuse on actual dependencies and typed
  bindings/effects/layouts/bodies across edits, and schedule independent misses under shared Cargo
  slots and memory budgets. Measure waits, admission and execution separately. Cargo/Go/uv set the
  millisecond unchanged rebuild bar. Measure affected before/after latency/throughput/startup, syscalls/flushes/locks/I/O/
  spawns/serialization, allocation, peak live/process-tree memory, caches/atomics/contention,
  code/artifact size, link time and failure atomicity.
- Time exact uv-managed CPython in isolated mode (`-I`), not the guarded dev environment. Cost
  attribution needs agreeing source, per-module profile and interleaved A/B. Recover purpose and
  measured Molt/CPython value before deletion; a fast path needs a general replacement. Recover speed
  through proven/guarded bindings and inlining, never name matching. Use typed platform/architecture
  capability plans, never retry/fallback feature detection.
- Instrument only to shorten decisions or remove measured recurring bottlenecks. Build profiling:
  `docs/OPERATIONS.md`, “Build time” (`MOLT_BUILD_DIAGNOSTICS`,
  `MOLT_BUILD_DIAGNOSTICS_FILE`, `MOLT_CLI_PROFILE`). Development apparatus is frozen: add no runner,
  receipt, ledger, scanner, dashboard or generated projection without a measured defect it would
  catch; delete apparatus with its last consumer. `docs/design/presubmit_test_strategy.md` owns the exit.

## Prove the changed claim once

- Name the changed obligation, plausible defect and independent oracle; extend existing test
  authorities under `docs/spec/areas/testing/0007-testing.md`. No implementation mirrors, mock-only
  success or count/coverage quotas. Every test file belongs to a proof cell that runs it. Load-bearing
  changes get adversarial review; recurse on findings, not quotas. Prose/scratch needs no new tests/apparatus.
- One proof owner selects affected obligations under `tools/proof_plan.toml` and
  `docs/agent/PROOF_QUEUE.md`, inspecting receipts first. Reuse only matching source/tool/environment/
  workload/artifacts and proven process closure; declare remaining obligations. Repeat/widen only
  for changed inputs, new failure or uncovered obligation; one cell cannot prove a family. Stop at
  sufficient development evidence; mandatory landing/release gates still run once, CI qualifies releases.
- Before each commit, run `tools/consumer_gate.py run -- PATHS` in the selected environment
  (static gates, reached Rust lib tests and first-hop importers, presubmit `-n auto --skip-slow`).
  Mark median >=60 s tests `@pytest.mark.slow` for postsubmit. Worker handoff uses
  `tools/molt_dev.py land --repo <checkout> --precheck-only` and changed tests in the runner's plain
  environment, not lane `env.sh`. The integrator reuses matching receipts where admitted and lets
  the landing runner execute required gates on the actual merge candidate. Queue submission and
  publication are separate states; CI arbitrates release candidates. Read-only findings have no per-turn proof quota.
- Preflight prerequisites in the actual child environment; parsing is not readiness. Validate mutations
  at their canonical boundary. Reuse compatible compiled producers/affected selectors; avoid unrelated
  suites, cold targets and competing Cargo writers. Measure before changing concurrency/coverage.
  Stop dependent batches at first failure; retain/classify product/harness/artifact drift/unknown before
  retry. Silent/frozen processes remain unknown until live process/log/artifact/guard evidence resolves them.
- Keep implementation, verified cells and release acceptance distinct. Align `README.md`/onboarding,
  then owning contracts/status projections; link canonical matrices. Keep raw proof logs private.

## Coordinate through existing owners

- Only integrators dispatch; workers never delegate/open a second lane. Keep an owner on the
  highest-impact compiler path. Delegate independent work or a load-bearing review when it shortens
  that path after startup, build-slot and memory costs. Batch reads/proofs; avoid tiny agents,
  overlapping writers and duplicate proofs. Parent owns architecture/integration; one owner per
  ledger family, no copied facts.
- Use validated `DispatchBounds` through `tools/dispatch_prompt.py` and existing coordination records.
  `docs/ops/MULTI_AGENT_COORDINATION.md` owns brief review, budgets, proof families, stop conditions,
  parent-controlled scope expansion, class-sweep reports and ownership closure. Task messages and
  worktrees are not locks. Integrator runs each lane's Goldfish design review
  (`tools/subagent_contract.py`, `DESIGN_REVIEW`); workers close owned commands canonically.
- Dispatch/resume with Codex `gpt-6.2-sol` at `high`, `gpt-6.2-astra` for hardest work, never `ultra`.
  Respect concurrency limits; Claude follows the same custody/evidence rules.

## Preserve custody

- `.python-version`/`uv.lock` own the environment (`docs/design/one_environment.md`); reuse/repair
  project launchers, no `--python` override or bare `python3`. Frontend CPython 3.12+ includes 3.15;
  record exact frontend and guest/CLI versions separately (`docs/getting-started.md`). Follow
  `docs/ops/MULTI_AGENT_COORDINATION.md`, “Interpreter environments are mutable proof inputs”:
  pinned oracle identities, no active project-scoped uv minor switches, owned extra environments,
  readers closed before mutation.
- Bind every Git mutation to the absolute checkout, expected branch/HEAD and exact owned paths;
  inspect the scoped diff before staging/commit/landing. Never reset, clean, overwrite, revert or stage
  unrelated user/parallel work. Destructive Git and broad cleanup require explicit authorization and
  verified targets. Preserve credentials, people and external systems; publishing, messaging, issues,
  pushes or other external mutations require user authorization or an explicit requested workflow step.
- Retiring WIP requires two source/history/current-owner passes, one independent when available:
  harvest useful changes, identify satisfied/superseded work and preserve unresolved signal with an
  indexed recovery path and owner/question. Keep the remaining claim, exact commit and proof receipt
  discoverable in the existing coordination record. Archives, age and cache-like names prove no reconciliation
  and grant no source-deletion authority.
- Stop only live-proved Molt-owned children/workers; test observers use `tests/process_guard_common.py`.
  Never stop Codex, Claude, app/renderer/server helpers, MCP/plugins, shell hosts, Git pollers,
  ancestors or ambiguous control-plane processes; preserve evidence and repair custody.
- Dependencies/toolchains/Actions use the newest release >=7 days old; advisory fixes need dated
  exemptions. No AI attribution in commits/PRs (`tools/check_commit_attribution.py`).
- Keep procedures/history in owning runbooks/designs, per-run logs/plans/handoffs outside tracked
  source (`docs/ROOT_LAYOUT.md`); retain private evidence and fold durable conclusions into public contracts.

## Route only to the needed authority

| Task | Existing owner |
| --- | --- |
| Choose/close V1 work | `docs/agent/V1_HANDOFF_FINDINGS.md` |
| Land/retire | `docs/ops/INTEGRATION.md`, `tools/molt_dev.py` |
| Dispatch/handoff/ownership | `docs/ops/MULTI_AGENT_COORDINATION.md` |
| Proof execution/custody | `docs/agent/PROOF_QUEUE.md`, `tools/proof_plan.toml` |
| Architecture/docs discovery | `docs/CANONICALS.md`, `docs/INDEX.md`, `docs/spec/README.md` |
| Live multi-agent/Pact state | Relevant parts of `docs/agent/ORCHESTRATION.md` |
| TIR generation | `runtime/molt-ir/src/tir/op_kinds.toml`, `tools/gen_op_kinds.py` |
| Runtime/ABI | `runtime/molt-runtime/src/intrinsics/manifest.pyi`, `runtime/runtime_abi.toml` |
| CI/JUnit triage | `tools/ci_triage.py RUN_ID` or `tools/ci_triage.py --junit FILE` |
| Loop anatomy/cost buckets | `tools/opt_explorer.py`, `tools/PERF_AUTHORITY.md` |

Load only what the task needs; update the owning authority when its facts change.
