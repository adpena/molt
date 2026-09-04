# Molt Agent Constitution

This file contains the small set of project facts and operating invariants that
should be present in every agent session. It is guidance for capable engineers,
not a script. Use judgment, inspect the live system, and prefer current code,
tests, measurements, and explicit user direction over stale prose.

## Own the outcome

- Carry an authorized task through implementation, integration, verification,
  and cleanup. When enough information exists to act, act.
- Pause only for input the user alone can provide, a real scope change, or a
  destructive, irreversible, privileged, or externally consequential action
  that was not already authorized. Proceed with safe, reversible work that
  follows from the request.
- Do not end on a plan, promise, checkpoint, commit, failed proof, compaction,
  or status report while useful in-scope work remains. Recover from interrupted
  tools and continue from durable repository and proof state.
- Ground progress and completion claims in evidence from the current run. Say
  plainly what passed, failed, was skipped, or remains unknown.
- If project guidance conflicts with the live repository or has gone stale,
  follow the safer current fact and repair the guidance in the same arc when it
  is in scope.

## Engineer coherent systems

- Enter through one concrete aperture, then follow the invariant through the
  complete coherent authority class. The aperture bounds discovery; it does
  not limit the engineering end state.
- Keep one canonical authority for each fact, state transition, storage owner,
  protocol, and generated table. Move all consumers together and delete the
  replaced implementation. Do not preserve internal backward-compatibility
  lanes, shims, duplicate registries, or speculative fallbacks.
- Prefer the simplest direct design that satisfies the real constraints.
  Introduce abstraction only when it removes duplication, makes an invariant
  explicit, or enables measured optimization without obscuring ownership.
- Systematize families instead of accumulating type-local or backend-local
  fixes. Specialized representations are welcome when they materially improve
  performance or correctness and still implement the shared protocol.
- Treat frontend, IR, passes, backends, runtime, ABI, tooling, diagnostics,
  packaging, tests, and docs as projections of the same architecture.

## Project direction

- Molt is an optimizing Python compiler and runtime, not a reimplementation of
  NumPy, SciPy, or other upstream packages. Ecosystem support comes from
  reusable compiler/runtime primitives, package and import custody, source-built
  extensions, and correct C-API/ABI behavior. Audited forks or vendored patches
  are acceptable only when they are the maintained, provenance-pinned solution
  to an upstream defect—not a hidden substitute implementation.
- Native and WASM are co-equal frontier targets. LLVM, MLIR, the rest of the IR
  stack, linkers, runtime memory management, and every shipped backend/profile
  are first-class optimization surfaces.
- Design for free-threaded and GIL-less execution while preserving deterministic
  expected CPython behavior by default.
- Support claims are explicit matrices over target OS, architecture, ABI,
  Python version, concurrency mode, backend, profile, and optimization level.
  Gate only on demonstrated capability, correctness, or performance—not on
  implementation convenience. Unsupported cells fail early with useful
  diagnostics.

## Performance is part of correctness

- Measure before and after load-bearing changes. Select the metrics that expose
  the changed contract: latency, throughput, startup, allocations, bytes, peak
  live memory, process-tree RSS/commit, cache behavior, atomics, contention,
  code size, link time, artifact size, and failure atomicity.
- Optimize per platform and architecture through typed capability and target
  plans. Do not use post-failure retries or silent fallback as feature
  detection.
- Build missing telemetry, profilers, benchmarks, or inspection tools when they
  materially shorten the path to a correct decision. Reuse one instrumentation
  authority across targets wherever possible.
- A local benchmark win does not justify duplicated control paths, weaker
  determinism, or unbounded memory. Record the relevant tradeoffs and gate the
  policy on reproducible evidence.

## Work with the live repository

- Start whole-project discovery with `uv run --python 3.12 python tools/agent_coordination.py context`; use `--json` for the versioned agent-facing model.
- Start with `git status`, applicable nested instructions, and the current
  source/proof state. For active multi-agent or Pact work, read the relevant
  sections of `docs/agent/ORCHESTRATION.md`; do not preload the entire historical
  board when a narrow live section is enough.
- Preserve user and parallel-agent work. Never reset, clean, overwrite, revert,
  or broadly stage unrelated changes. Integrate with pathspecs and review every
  diff that will be committed or landed.
- Generated files are projections. Change their declarative source or generator
  and regenerate them in the same arc.
- Prefer existing project commands and helpers when they express the current
  contract. Improve or replace them when live evidence shows they are stale,
  ambiguous, duplicated, or wasteful.
- Keep operational history, machine-specific incidents, long procedures, and
  subsystem tutorials out of this always-loaded file. Put durable detail in the
  relevant design, spec, runbook, skill, test, or live board and retrieve it
  when needed.

## Delegate deliberately

- Use subagents for independent, bounded work that benefits from parallel
  attention or would pollute the orchestrator context: exploration, audits, log
  analysis, proof review, mechanical migration, and disjoint implementation.
- Keep working while subagents run. Communicate asynchronously, redirect a lane
  that drifts, and integrate their evidence rather than forwarding raw output.
- Avoid overlapping write ownership and redundant proof fanout. The parent owns
  the final architecture, integration, verification, and cleanup.
- Match model speed and reasoning effort to the task: strongest available
  reasoning for load-bearing architecture and correctness; faster workers for
  well-bounded scans and mechanical work.

## Verify the claim, not the ritual

- Prove the exact changed contract first, then widen in proportion to the claim.
  Use focused static checks, unit tests, differential tests, integration tests,
  target execution, profiling, and benchmarks as appropriate.
- One green backend/profile/target cell does not prove a family-wide claim.
  Conversely, do not run broad expensive suites when a narrow check completely
  proves the owned invariant.
- Treat a frozen or silent process as unknown until logs, artifacts, guard
  state, or live process evidence establishes its result.
- Before completion, review the owned diff, generated synchronization, docs,
  diagnostics, failure paths, allocations, and cleanup. Fix newly exposed
  in-scope defects rather than converting them into a report.

## Safety and custody

- Preserve unrelated files, credentials, external systems, and people. Do not
  publish, message, file issues, push, or mutate external state unless the user
  authorized that action or it is an explicit step of the requested workflow.
- Never use destructive Git operations or broad filesystem cleanup without
  explicit authorization and verified targets.
- Molt process cleanup may target only a live-proved Molt-owned child or worker.
  Never target Codex, Claude, app/renderer/server helpers, MCP/plugin processes,
  shell hosts, Git pollers, ancestors, or ambiguous host control-plane
  processes. Preserve evidence and repair custody when identity is unclear.
- Before a risky or long-running proof, leave a recoverable command/cwd/status/
  evidence capsule under the established guard/incident machinery. Prefer
  detached project custody for expensive or contention-heavy work; see
  `docs/agent/PROOF_QUEUE.md` when that machinery is actually needed.

## Useful authorities

- Live multi-agent/Pact state: `docs/agent/ORCHESTRATION.md`
- Canonical architecture and documentation map: `docs/CANONICALS.md`,
  `docs/INDEX.md`, `docs/spec/README.md`
- Proof-queue operations: `docs/agent/PROOF_QUEUE.md`
- TIR facts and generation: `runtime/molt-ir/src/tir/op_kinds.toml`,
  `tools/gen_op_kinds.py`
- Runtime and ABI manifests: `runtime/molt-runtime/src/intrinsics/manifest.pyi`,
  `runtime/molt-backend-wasm/src/wasm_abi_manifest.toml`

These are pointers, not mandatory reading lists. Load only what the current task
needs, and update the owning authority when its facts change.
