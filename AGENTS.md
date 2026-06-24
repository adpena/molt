# Repository Guidelines

## HIGHEST GUIDING PRINCIPLES: 100-Year Optimal Engineering (Turn Blocker)

These principles outrank every lower-level convenience rule. Read them before
touching code, docs, tests, benchmarks, or roadmap state.

- Build future technology, not short-lived scaffolding. Every landing point must
  compound toward a 100-year production architecture: small binaries, fastest
  startup, fastest compute, deterministic correctness, portable execution, and
  a codebase that gets simpler as it grows.
- Move fast and break the wrong abstractions. Speed is achieved by deleting
  debt, collapsing layers, replacing accidental complexity with math-tight
  primitives, and landing end-state structures early. Do not preserve broken
  seams merely because existing tests or callers rely on them.
- Take structural detours when they improve the system dynamics. Research the
  whole codebase as needed to understand the pattern behind the local bug, then
  fix the bug class, not the symptom. A detour is justified when it deletes a
  source of churn, unifies authority, removes fragility, or opens a faster path
  for future work.
- Treat the whole system as math. Prefer the geometrically, algebraically, and
  calculus-optimal structure: one authority per invariant, one storage home per
  value, one import transaction per module-state transition, one guard owner per
  process tree, one typed fact path through frontend, IR, optimizer, backend,
  runtime, and tooling. If two paths can disagree, delete or unify one.
- No legacy code as a compatibility crutch. No backward compatibility inside
  Molt internals, no stale aliases, no duplicate dispatch surfaces, no opt-in
  old behavior, no hidden fallbacks, no temporary wrappers. When a path is
  touched, delete the legacy lane or structurally reconcile it in the same arc.
- Maximize verified compatibility only inside Molt's AOT contract. Pursue full
  Python 3.12+ stdlib and ecosystem compatibility where it does not require
  unrestricted dynamic execution, runtime monkeypatching, reflection-heavy host
  fallback, or behavior outside the verified subset contract. Unsupported
  dynamism must fail closed with explicit diagnostics.
- All OS, architecture, backend, and Python-version behavior must be explicit.
  Gate semantics by Python 3.12/3.13/3.14, target, host OS, architecture, and
  capability surface. Accidental ambient behavior is a bug.
- Quality gates are non-negotiable for claimed support, but they are not a
  license to stall implementation with excessive proof loops. Differential
  tests, conformance suites, CPython regrtest lanes, native/WASM/LLVM/Luau
  target parity, memory custody, sanitizer/runtime checks, and benchmarks must
  back the claims they prove; do not run broad lanes as progress theater.
- Verification must stay high-signal. Use focused tests, repro shards,
  differential checks, conformance lanes, and benchmarks when they prove a
  changed contract, retire a concrete risk, or support a compatibility/perf
  claim. Do not churn broad repetitive suites as progress theater; once the
  relevant invariant is proven, move back to structural code or docs.
- Performance is part of correctness. Molt must be faster than CPython on every
  claimed benchmark and trend toward or beyond PyPy and Codon where comparable.
  If a feature is correct but structurally slow, the task is not complete.
- Accelerate developer velocity by deleting debt, collapsing duplicate
  authorities, generating evidence, and making the correct path mechanical.
  Documentation, AGENTS guidance, indexes, specs, matrices, tests, and tooling
  are part of the compiler architecture and must move with the code.

## Execution Velocity Doctrine: Broad Structural Arcs, Bounded Proof

- Default to broad, coherent structural arcs. Do not split work into tiny
  "safe" slices merely to create checkpoints, commits, or status updates. When
  a subsystem has one wrong authority, rip open the full authority boundary and
  move the callers, tests, docs, and generated facts needed to make the old path
  disappear.
- Commit complete authority moves or genuinely independent structural pieces.
  A small commit is acceptable only when it deletes a complete source of drift;
  it is not acceptable when it leaves a hybrid path because the agent stopped
  early.
- Verification is an evidence budget, not a ritual. During development, run the
  smallest high-signal static or targeted command that can catch integration
  mistakes for the owned arc, then return to code. Broad differential,
  conformance, benchmark, regrtest, or validation lanes are for explicit
  compatibility/performance claims, release/merge gates, or user request.
- Do not get trapped in repeated lint/test loops. If a proof fails, fix the
  structural cause once, rerun the specific failing proof once, and keep
  implementing. Avoid expanding proof scope unless the failure exposes a real
  cross-layer contract risk.
- Prefer subagents for disjoint broad work: one agent can map call sites or
  migrate non-overlapping files while the main agent lands the authority move.
  Do not use subagents to multiply proof lanes or produce status theater.

## ABSOLUTE TOP PRIORITY: No Shortcuts, No Partial Implementations (Turn Blocker)

**Engineer like Chris Lattner / Mojo / NASA. Never take a shortcut, workaround, or "simpler" implementation when the structurally correct fix is harder.** This rule overrides every comfort instinct.

The temptation chain you must reject:
- "I'll just promote Value-tier shadows at loop_start to fix this faster" → NO. That is a localized hack on top of an architecturally broken design. Do the structural redesign (typed IR, eliminate the shadow system) instead.
- "I'll add a small guard to handle this edge case" → NO. The edge case exists because the abstraction is wrong. Fix the abstraction.
- "I'll commit the partial fix and follow up later" → NO. There is no later. Either land the complete fix or do not start.
- "The full fix is too risky, let me ship something safer" → NO. The "safer" thing accumulates compound interest of bugs. Take the time. Do it right.
- "I'll skip the perf step and come back to it" → NO. Perf-correctness gaps create distrust. Land the fast version with the correct version.

When you identify a structurally correct fix and feel pulled toward an "immediate win" or "incremental approach", **STOP**. That pull IS the signal you are about to ship a workaround. Land the structural fix even if it is multi-day work.

If you cannot complete the structural fix in this session, **do not commit the localized hack as a placeholder**. Leave a clean baton-pass note describing the structural fix needed; the next session picks it up. Half-measures committed to main are worse than nothing committed.

This rule applies equally to:
- **Correctness**: bug class fixes, not bug instance fixes (e.g., fix the phi-representation invariant, not just the one site that exposed it)
- **Optimization**: structural codegen changes, not localized peephole tweaks
- **Performance**: redesign the hot path, do not add bypass cases
- **Architecture**: rework the abstraction, do not stack patches on a wrong foundation

Performance contract: molt MUST be faster than CPython on every benchmark, across every target (native, WASM, LLVM, Luau) and every profile (release-fast, dev-fast, debug-with-asserts). Do not declare a perf task complete until measurements confirm it on all targets.

### Concrete examples of partial implementations to reject

These are real shortcuts caught and reversed in past sessions. Do not repeat them:

- **Compressing architect/research output**: When a sub-agent returns a 1500-line architecture plan, write the *full* text to disk. Condensing to "key points" loses the line numbers, sub-phase test specifications, and risk treatments that an implementing agent needs. The architect's full text is the artifact.
- **Asymmetric coverage of a structural fix**: If you migrate the in-loop call sites of a helper, also migrate the out-of-loop sites. If you fix the int lane's shadow system, mirror the same change to bool/float/str. Asymmetry is a partial implementation that re-creates the original bug at the unmigrated site.
- **Splitting an atomic refactor across commits to "make progress visible"**: If Phases 1a/1b/1c/1d are one structural arc per the design, ship them as one atomic change or commit them with explicit "this leaves the codebase in a hybrid state until 1d lands" notes. Three commits that shipped 1a/1b/1c without 1d leave two parallel sources of truth in the tree — exactly the compound-interest-of-bugs trap this policy exists to prevent.
- **Stopping at the first measurable win**: A 10% perf bump from Phase 1b is not "good enough" if Phase 1d would yield 50%. The 10% does not justify halting the structural change.
- **"Debug-gated assertion" as a substitute for migration**: An assertion that catches divergence between the static and dynamic sources of truth is a verification tool, not a substitute for unifying them. Verify the invariant *while* completing the migration, not as a way to defer it.
- **Per-test special cases**: If a test fails after a structural change, the change is wrong (or the test reveals a missing invariant). Do not add a guard that special-cases the failing test.
- **Renaming `_unused`** to silence a compiler warning instead of using or removing the variable: pick one. Both options are clean; the rename is a shortcut.

### Structural change as the unit of work

The unit of work is the *complete structural change*, not the smallest committable diff. When the design says "Phase 1 = 1a + 1b + 1c + 1d", Phase 1 is not done until 1d lands. Intermediate commits are acceptable only when each is itself a complete structural piece (not a partial fix toward the next piece) and a baton-pass note documents the remaining unfinished arc.

Before choosing work size, identify the whole structural work class: every
neighboring duplicate authority, every call site, every backend/frontend/tooling
consumer, every generated table, every proof lane, and every doc route that is
part of the same invariant. Do not take the smallest visible board item, one
match arm, one failing test, one file-local patch, or one easy metric decrement
when the evidence shows a larger shared abstraction is being exposed. Burning
down tiny counts while leaving the surrounding duplicate authority intact is
avoidance, not progress.

A smaller landing is valid only when it is a complete end-state subsystem cut:
it exhausts that invariant's duplicate authorities inside the touched subsystem,
has no adjacent same-kind dispatch/fact/source-of-truth left behind, and gives
future work a stronger foundation instead of another seam. If the first proposed
unit leaves a sibling classifier, parallel backend lane, mirrored frontend path,
or same bug class still open next to it, expand the unit until the whole class is
unified. Use baton-pass notes only for genuine external blockers or proof lanes
that cannot run in the current environment; never use them to excuse a tiny-chip
sequence.

Convenient tiny-chip progress is the silent death of this project. It creates
the feeling of velocity while preserving exactly the scattered authorities that
make correctness, performance, and compatibility non-compounding. Any agent that
keeps choosing tiny audit-row burn-down, "safe" local edits, or narrow proof
loops after the operator asks for deeper structural work is refusing the task.
When the operator says a plan is too small, stop immediately, discard the
comfort-sized plan, re-open the design radius, and attack the whole coherent
work class.

Bold structural convergence outranks local neatness. Avoid local minima,
overfitted proof apparatus, and excessive conformance/testing loops that serve
as a substitute for changing the architecture. Verification is mandatory only
insofar as it proves the structural invariant being moved; once it proves that
invariant, return to unifying the system instead of orbiting the tests.

### Year-5 end-state architecture first

Roadmap months and years are evidence horizons, not design boundaries. Do not build disposable Month-1, Year-1, or Year-2 scaffolding that will be replaced later. Every compiler/runtime/tooling design must be shaped toward the final production-hardened Year-5 architecture from the first landing point.

The shortest wall clock path is architectural convergence: typed IR coherence, reusable optimization primitives, backend-neutral proofs, deterministic custody, sanitizer-clean runtime behavior, and target parity that can compound across all future work. If a proposed step only makes a near-term milestone look greener while creating a second source of truth, backend skew, or temporary compatibility lane, reject it and implement the end-state abstraction instead.

Intermediate commits are acceptable only when each one is itself a complete, end-state-compatible structural primitive with durable tests and documentation. Never use the roadmap's yearly ordering as permission to defer correctness, performance, cross-backend parity, memory safety, or developer-experience automation that the final architecture requires.

### Codebase Authority, Documentation Routing, And Legacy Deletion

- The live codebase plus executable tests are the sole source of truth. Roadmap,
  status, design, spec, matrix, and memory documents are routing aids and stale
  hypotheses until verified against current code, current tests, and current
  generated artifacts.
- Update documentation as you go. Any change that moves supported semantics,
  backend contracts, compiler architecture, compatibility claims, validation
  gates, or roadmap priority must update the relevant docs in the same change:
  `ROADMAP.md`, `docs/spec/STATUS.md`, `docs/spec/README.md`, `docs/INDEX.md`,
  and the relevant spec index/matrix listed below.
- No backward compatibility ever inside Molt internals. Do not preserve legacy
  code paths, compatibility aliases, opt-in old behavior, env-var fallback
  gates, duplicated dispatch tables, stale shims, or "temporary" wrappers. If a
  touched path has a legacy lane, delete or structurally reconcile it in the
  same arc.
- Streamline and refactor as you go. A structural fix is incomplete if it leaves
  the old source of truth beside the new one. When migration appears too large,
  first widen the audit to the whole authority cluster, then carve only along a
  real subsystem boundary that removes every sibling duplicate in that boundary.
  Do not convert a cluster into a queue of tiny isolated fixes; fail closed or
  leave a blocker note only when a real external constraint prevents the whole
  authority class from moving.
- Generated docs and matrices remain generated-only. Update their source data
  and run the generator; never hand-edit generated semantic status.
- When docs conflict, code/tests win. Resolve the docs before claiming the work
  complete, and mark stale claims as stale instead of preserving ambiguity.

### Source-Of-Information Map

Read these first instead of rediscovering project structure:

- Code authority:
  - `runtime/molt-backend/src/tir/` for TIR, pass manager, analyses, lowering,
    verification, and generated op-kind facts.
  - `runtime/molt-backend/src/tir/pass_manager.rs`,
    `runtime/molt-backend/src/tir/module_phase.rs`, and
    `runtime/molt-backend/src/tir/drop_phase.rs` for pipeline order,
    module-level transforms, and terminal RC drop insertion.
  - `runtime/molt-backend/src/tir/op_kinds.toml` for the canonical op-kind
    vocabulary, effect rows, ownership classifiers, and generated backend/
    frontend tables.
  - `runtime/molt-backend/src/tir/op_kinds_generated.rs` and
    `src/molt/frontend/lowering/op_kinds_generated.py` for generated op-kind
    tables; update `op_kinds.toml` plus `tools/gen_op_kinds.py`, not generated
    outputs by hand.
  - `runtime/molt-backend/src/representation_plan.rs` for scalar/container
    representation authority shared by backends.
  - `runtime/molt-backend/src/native_backend/`,
    `runtime/molt-backend/src/llvm_backend/`, `runtime/molt-backend/src/wasm.rs`,
    and `runtime/molt-backend/src/luau.rs` for backend-specific lowering.
  - `runtime/molt-runtime/src/intrinsics/manifest.pyi`,
    `runtime/molt-runtime/src/intrinsics/generated.rs`, and
    `src/molt/_intrinsics.pyi` for intrinsic authority.
  - `src/molt/frontend/` and `src/molt/frontend/lowering/` for frontend and
    SimpleIR/TIR emission contracts.
  - `src/molt/backend_daemon_custody.py` for backend-daemon identity sidecar
    parsing, command verification, health-probe verification, legacy `.pid`
    cleanup, and the only authorized daemon signal/escalation primitive used by
    CLI and benchmark cleanup paths.
  - `tools/memory_guard.py`, `tools/harness_memory_guard.py`,
    `tools/process_sentinel.py`, and `tools/guarded_exec.py` for memory/RSS
    custody, protected host/control-plane process-group filtering, guarded
    subprocess execution, repo sentinels, repro diagnostics, and Molt-owned
    termination scope. `tests/conftest.py`, `src/sitecustomize.py`, and
    `tests/*/sitecustomize.py` route pytest/direct-test guard startup.
  - `runtime/molt-gpu/src/` for tinygrad-conformant GPU primitives,
    ShapeTracker, scheduling/fusion, CPU/Metal/WebGPU execution, MLIR/MIL/text
    renderers, and materialization/view-lowering contracts.
- Documentation entry points:
  - `docs/CANONICALS.md`, `docs/INDEX.md`, and `docs/spec/README.md` are the
    navigation roots.
  - `docs/spec/STATUS.md` is a current-state summary, but must be refreshed
    from code/tests whenever touched.
  - `ROADMAP.md` is a forward plan, but must be refreshed from code/tests and
    current benchmark/differential evidence whenever touched.
  - `docs/design/foundation/00_integrated_parallel_program.md`,
    `01_E1-activation.md`, `02_S5-memssa.md`, `03_E3-E5-ipo.md`,
    `04_L4-loops.md`, `20_rc-ownership-drop-insertion.md`,
    `21_decomposition_program.md`, `25_op_kind_registry.md`,
    `27_perceus_borrow_inference.md`, `44_frontend-architecture-f2.md`, and
    `45_exception_region_ownership.md` are foundation routing docs, not proof.
  - `docs/design/parallel_build_architecture.md` and
    `docs/architecture/compilation-model.md` route crate extraction,
    compiler-throughput, incremental-build/cache, runtime leaf-crate, backend
    native-crate, and god-file decomposition work. Use these before touching
    crate boundaries, Cargo profiles, `molt-runtime` leaf crates, backend
    extraction, or shared build-cache policy.
	  - `docs/architecture/gpu-primitive-stack.md`,
	    `docs/spec/areas/perf/0513_GPU_PARALLELISM_AND_MLIR.md`, and
	    `docs/design/foundation/16_cpython-surface-stdlib-gpu-gap-audit.md` route
	    GPU primitive, MLIR, and tinygrad/DFlash-adjacent status.
	  - `docs/BENCHMARKING.md`, `bench/friends/manifest.toml`,
	    `bench/friends/README.md`, and `tools/bench_friends.py` route friend-suite
	    benchmarking. The upstream tinygrad compatibility/perf lane is
	    `tinygrad_off_the_shelf`, driven by `tools/tinygrad_off_shelf_adapter.py`;
	    keep it pinned before enabling and use it to compile/profile unmodified
	    tinygrad code for GPU, MLIR, and typed runtime upload/readback work.
- Compatibility/spec matrices:
  - `docs/spec/areas/compat/README.md` is the compatibility documentation root.
  - `docs/spec/areas/compat/contracts/verified_subset_contract.md`,
    `dynamic_execution_policy_contract.md`, `import_system_contract.md`,
    `compatibility_fallback_contract.md`, `cpython_bridge_policy.md`,
    `package_abi_contract.md`, and `libmolt_extension_abi_contract.md` define
    compatibility policy.
  - `docs/spec/areas/compat/surfaces/language/language_surface_matrix.md`,
    `semantic_behavior_matrix.md`, `syntactic_features_matrix.md`,
    `type_coverage_matrix.md`,
    `core_language_pep_coverage.generated.md`, and
    `generator_api_coverage.generated.md` route language coverage.
  - `docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_index.md`,
    `stdlib_surface_matrix.md`, `stdlib_intrinsics_backing.md`,
    `stdlib_intrinsics_audit.generated.md`,
    `stdlib_platform_availability.generated.md`, and
    `asyncio_surface.generated.md` route stdlib coverage.
  - `docs/spec/areas/compat/surfaces/c_api/c_api_surface_index.md`,
    `c_api_symbol_matrix.md`, and `libmolt_c_api_surface.md` route C-API/
    `libmolt` work.
  - `docs/spec/areas/compat/surfaces/ecosystem/ecosystem_compat_matrix.generated.md`
    routes third-party ecosystem claims.
  - `docs/spec/areas/compiler/backend_lir_representation.generated.md` and
    `docs/spec/areas/compiler/luau_support_matrix.generated.md` route backend
    representation and Luau support evidence.

## Top Priority: Tinygrad + DFlash Fidelity (Non-Negotiable, Turn Blocker)
- Exact tinygrad semantics and API shape are the public ML contract. No drift is acceptable.
- Exact DFlash algorithmic fidelity is non-negotiable when implementing DFlash support. Do not substitute generic speculative decoding and call it DFlash.
- `molt.gpu` and `molt.gpu.dflash` may provide the implementation substrate, but user-facing tensor/model behavior must stay 1:1 with tinygrad where tinygrad is the source of truth.
- DFlash implementations must remain faithful to the paper and official project requirements, including target-conditioned draft behavior, verifier/drafter separation, and any required conditioning/KV contracts. If a required trained drafter does not exist for a model, raise that limitation explicitly rather than faking support.
- If current code drifts from tinygrad or DFlash source-of-truth behavior,
  prioritize cleaning up the drift before adding more surface area.

## ABSOLUTE NON-NEGOTIABLE: Zero Workarounds Policy (Turn Blocker)
- This is an early alpha project. We are the sole users and developers.
- ZERO tolerance for workarounds, hacky fixes, partial fixes, TODO-as-excuse, "simpler fix" that avoids the real problem, technical debt, code smell, silent failures, or divergences.
- When you identify the correct fix and feel tempted to do something "simpler" instead — STOP. That temptation IS the signal you're about to create a workaround. Do the correct fix.
- If a fix requires refactoring, do the refactoring. If it requires a new abstraction, build it. If it requires research, do the research.
- No `catch_unwind` to swallow panics. No `if has_loop { return original_ops }` bypasses. No "preserve original ops until Phase N". Implement Phase N now.
- Full deterministic CPython >= 3.12 parity (except: no exec/eval/compile, no runtime monkeypatching, no unrestricted reflection).
- All backends (native/Cranelift, WASM, LLVM) must have parity.
- NEVER revert or discard unstaged partner changes. Integrate around them or
  isolate your own changes; ask only when partner WIP makes the task impossible.
- Always `git add` immediately after writing files (commit hooks are read-only by default; explicit staging keeps owned changes atomic).

## Top Priority: Chris Lattner Compiler Engineering Standards (Feb 18, 2026) (Non-Negotiable, Turn Blocker)
- This section is a top-of-file hard gate and applies to every compiler/runtime/tooling turn; violations block merge and must be fixed before completion.
- AI acceleration is expected, but ownership cannot be delegated: humans and agents remain fully accountable for architecture quality, correctness, maintainability, and long-term evolution.
- Global compiler coherence is mandatory: preserve clear contracts across frontend, IR/midend, optimization passes, codegen backends, runtime boundaries, and developer tooling.
- Local patches that introduce cross-layer coupling, duplicate semantics, or subsystem drift are prohibited; redesign into stable interfaces before proceeding.
- Test-suite gaming is explicitly forbidden: no hardcoded fixtures, no test-specific behavior branches, no fake/system-header shortcuts, and no narrow implementations that only satisfy current tests.
- Generalization over benchmark theater: each change must be defensible for real-world programs outside the current suite, with explicit constraints documented when scope is intentionally limited.
- Reusable abstractions are required when patterns repeat: promote repeated compiler/runtime behavior into first-class primitives, IR constructs, or shared utilities instead of ad-hoc one-offs.
- Parser and diagnostic quality are production requirements, not polish: preserve or improve error recovery, source locations, message clarity, and actionable remediation guidance.
- Deterministic verification loops are mandatory: acceptance requires measurable evidence (differential parity, targeted regressions, benchmark impact for hot paths, and memory/regression checks where relevant).
- "Looks right" and "seems fine" are not acceptance criteria; every significant behavior/perf claim must be backed by reproducible command output and documented rationale.
- Documentation is operational infrastructure: when architecture or semantics move, update design docs/spec notes/invariants in the same change so humans and AI can safely extend the system.
- AI should be used aggressively for mechanical rewrites, migrations, and boilerplate implementation, while human/agent judgment is focused up-stack on design, abstraction choice, and system evolution.
- Provenance and licensing hygiene are mandatory for generated/translated code: avoid uncertain lineage, document source inspiration when material, and prefer clean re-derivation over risky copying.
- If any required quality bar above cannot be met in the current turn, record
  the missing guarantee, risk impact, and concrete closure plan, then continue
  with non-colliding structural work that improves the same end-state.

## Hard Gate: Canonical Artifact Locations And Cleanup (Non-Negotiable, Urgent)
- Local development may use repo-local storage, but build artifacts, caches, tmp files, logs, benchmark outputs, and debugging outputs MUST live in canonical locations rather than ad hoc paths scattered across the tree.
- Default canonical roots:
  - `target/` for Cargo artifacts and shared build state.
  - `bench/results/` for benchmark JSON/results/report artifacts.
  - `logs/` for run logs, profiling logs, regrtest logs, and audit output.
  - `tmp/` for ephemeral temp files, scratch outputs, local quarantine dirs, and one-off debugging artifacts.
  - `dist/`, `build/`, and `wasm/` only when a specific tool or workflow intentionally writes there.
- Do not create new top-level artifact directories without documenting them in the same change. If a workflow needs a new artifact class, give it one canonical location and update the repo instructions accordingly.
- Keep the repo clean during active development:
  - reuse canonical directories instead of creating per-command ad hoc output roots;
  - prune stale artifacts regularly, especially large logs, old benchmark bundles, scratch tmp trees, and abandoned debug outputs;
  - remove no-longer-needed artifacts at the end of a task unless they are required for reproducible evidence or are part of the intended checked-in output.
- Canonical env defaults (use these in your shell before build/test/bench work unless a tool requires something else):
  - `export MOLT_EXT_ROOT=$PWD`
  - `export CARGO_TARGET_DIR=$PWD/target`
  - `export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR`
  - `export MOLT_CACHE=$PWD/.molt_cache`
  - `export MOLT_DIFF_ROOT=$PWD/tmp/diff`
  - `export MOLT_DIFF_TMPDIR=$PWD/tmp`
  - `export UV_CACHE_DIR=$PWD/.uv-cache`
  - `export TMPDIR=$PWD/tmp`
- DX wrappers should prefer healthy external artifact roots before the internal disk when configured (`prefer_external_artifacts`, `MOLT_PREFER_EXTERNAL_ARTIFACTS=1`, or `tools/run_context_env.py --prefer-external-artifacts`). The ordered default candidates are `/Volumes/VertigoDataTier/Molt` then `/Volumes/APDataStore/Molt`; override with `MOLT_EXTERNAL_ARTIFACT_ROOTS` and tune health gating with `MOLT_EXTERNAL_MIN_FREE_GB`.
- Explicit canonical env vars remain authoritative: if an operator sets `MOLT_EXT_ROOT`, `CARGO_TARGET_DIR`, `MOLT_CACHE`, `TMPDIR`, or related roots, wrappers must derive only missing defaults and must not overwrite the explicit value.
- Backend daemon sockets are control-plane state, not bulk artifacts. Keep `MOLT_BACKEND_DAEMON_SOCKET_DIR` under a short local socket-capable path by default (for example `/tmp/molt-backend-<repo-hash>`), and override it only to a filesystem proven to support Unix sockets.
- Canonical cleanup commands:
  - `molt clean`: dry-run the canonical ignored artifact/cache cleanup allowlist.
  - `molt clean --apply`: delete ignored artifacts from the canonical allowlist.
  - `molt clean --apply --kill-processes`: first run the repo process sentinel, then delete ignored artifacts; use this for stale/interrupted build, bench, or test workers before reclaiming `target/`.
  - `tools/dev.py clean-artifacts --apply`: dev-wrapper alias for the same cleanup engine.
- Notes:
  - `CARGO_TARGET_DIR` also relocates Molt’s shared build state under `<CARGO_TARGET_DIR>/.molt_state/` (locks, fingerprints, daemon state). Keep that state in the canonical target root rather than inventing parallel targets.
  - Cargo incremental quarantine receipts under `target/.molt_state/quarantine/cargo_incremental/` are bounded ignored incident evidence: the guard writes and prunes them during normal retention, and explicit `molt clean --apply` removes them with other allowlisted target artifacts.
  - `molt clean` and `tools/dev.py clean-artifacts` both route through `tools/artifact_cleanup.py`; tracked files, dirty partner work, `.venv/`, `.omx/`, `third_party/`, fuzz corpora, and test corpora are excluded from default cleanup.
  - Keep `.gitignore` and `tools/artifact_cleanup.py` pathspecs in sync whenever a new canonical artifact root is added.
  - If a workflow would generate unusually large artifacts, put them under the canonical root for that class and clean them up once the evidence is no longer needed.

## Git Workflow Policy (Non-Negotiable)
- Optimize for velocity on the live codebase. Develop from current `main` by
  default and keep local changes staged by ownership slice.
- Push directly to `main` (`origin/main`) by default when asked to publish.
- Branches and worktrees are allowed when they accelerate non-colliding
  implementation, recovery, or swarm work. Name them clearly, keep them based on
  current `main`, and converge useful changes back without preserving legacy
  compatibility lanes.

## Crash Recovery Tiny-Slice Mode (Non-Negotiable)
- This mode overrides broad-arc execution whenever Codex, Claude, the desktop
  app, WSL bridging, MCP/tool discovery, subagent orchestration, process
  custody, or a guarded command has crashed, stalled, disappeared, or been
  manually killed during the current session.
- In this mode, force the smallest complete structural primitive that can be
  written, staged, focused-tested, and committed before the next risky lane.
  Tiny means short feedback and durable recovery; complete means no hack, no
  duplicate authority, no dangling legacy lane, and no half-migrated invariant.
- Before every risky command in recovery mode, leave a death capsule under the
  canonical evidence roots: command, cwd, guard pid, expected child pid when
  known, status, timestamp, and the evidence path. Prefer
  `tools/memory_guard.py` active markers in `tmp/memory_guard/active/`,
  incident summaries in `tmp/memory_guard/incidents/`, pytest outer-guard
  summaries, and `logs/agents/codex_stall/*.json`.
- If the agent, child command, or helper process disappears, the next agent must
  inspect `git status`, active guard markers, incident summaries, pytest
  outer-guard summaries, codex-stall records, and host-control-plane
  classification before resuming. Do not infer the cause from the last chat
  line alone.
- Manual killing of a Molt-owned child/helper is a supported failure mode. The
  guard must record it as child interruption/failure and must not broaden
  cleanup to Codex, Claude, app-server, renderer, node-repl, ancestors, or any
  unrelated host control-plane process. If custody is uncertain, patch custody
  first.
- Re-enable multi-hour broad execution only after at least one tiny complete
  slice has landed with focused evidence and the active death-capsule state
  explains the previous failure mode.

## Default Execution Mode (Non-Negotiable)
- Outside Crash Recovery Tiny-Slice Mode, default to multi-hour autonomous
  execution behavior: work in long uninterrupted bursts, batch multiple related
  structural arcs per turn, and use minimal but high-leverage worker
  orchestration.
- Proactively clean stale Molt-owned worker groups when needed to keep execution
  stable and deterministic, but never terminate Claude, the Codex app,
  app-server, renderer, node-repl, or any ancestor/host control-plane process
  group as cleanup collateral.
- Do not stop at neat local checkpoints or tiny slices outside recovery mode.
  Only stop for a real blocker, a safety constraint, or when remote proof on
  tertiary is the next required step.
- Do not emit tranche summaries after every small fix. Keep going until a
  substantial bundled burndown is complete, and prefer whole bug-class or
  authority-cluster closure over isolated issue-count reduction.
- Operator correction is binding. If the user says the work is being sliced too
  small, do not defend the current plan, do not rename the slice, and do not
  continue the same local tactic. Expand the task boundary to the underlying
  structural class and proceed.

## Windows Codex App Stability Guardrails (Non-Negotiable)

Public Windows Codex reports reviewed on 2026-06-22 show recurring desktop
stability failures around renderer load, long pasted terminal payloads,
PowerShell host startup, external-process launch, stale Git/app background
processes, Store-package sandbox ACL setup, non-ASCII profile paths, and
multi-agent/tool-discovery retry storms. A fresh 2026-06-22 review of OpenAI
Windows docs plus `openai/codex` Windows issues also shows a systemic
Windows Desktop + WSL risk pattern: config home, runtime platform, project-root
serialization, plugin cache, SQLite state, shell startup, bundled helper
architecture, sandbox ACL setup, browser/computer-use helpers, MCP enumeration,
and large thread resume can each infer a different environment model. A
2026-06-23 refresh adds concrete regressions where WSL workspaces were rewritten
as invalid `C:\home\...` paths after updates or host crashes, WSL mode launched
Windows app-server/PowerShell instead of a Linux runner, WSL-without-distro and
Windows ARM64 + Linux x64 helper mismatches entered startup crash loops, and
plugin caches under `/mnt/c/...` or sandbox ACL failures caused severe stalls or
broken child-process networking. Treat these as host control-plane risks while
working in this repo:

- Before any long-running, recovery, Windows, macOS, WSL, or multi-agent turn,
  determine the actual execution environment. Record host OS, shell, `cwd`,
  repo path, `CODEX_HOME` when visible, WSL indicators (`WSL_DISTRO_NAME`,
  WSL interop variables), whether the workspace path is Windows-native,
  WSL `/home/...`, WSL `/mnt/c/...`, or macOS-native, and the resolved paths
  for `python`, `python3`, `py`, `bash`, `node`, and `npm` if those tools may
  be used. Prefer `tools/agent_coordination.py env` for the repo-local snapshot.
- Before intentionally broad differential, conformance, backend, or perf proof
  work, run `uv run --python 3.12 python tools/agent_coordination.py proof-plan`
  or pass the intended touched paths explicitly. Ordinary structural
  implementation does not need this ceremony; choose one bounded proof for the
  owned arc and keep moving.
- Never paste huge terminal logs, stack traces, generated diffs, benchmark
  JSON, or repeated error streams into the Codex prompt. Write large evidence
  under canonical roots (`logs/`, `tmp/`, `bench/results/`) and summarize the
  bounded relevant lines in chat.
- During a suspected Codex/Desktop stall around a long Molt proof lane, wrap
  the proof command with the privacy-preserving stream timing diagnostic:
  `uv run --python 3.12 python tools/agent_coordination.py codex-stall -- <proof-command>`.
  It writes `logs/agents/codex_stall/*.json`, records first-output gaps,
  stream-idle spans, byte counts, and return code, and deliberately stores no
  child stdout/stderr text or Codex state. The wrapper launches through
  `tools/memory_guard.py` by default; use `--no-memory-guard` only for a
  non-proof probe or an already guarded direct child.
- Keep tool output bounded. Avoid broad noisy scans such as repo-wide TODO/HACK
  searches without tight globs, and set conservative output budgets for any
  command that can print thousands of lines. If a command is noisy, redirect it
  to a canonical log file and inspect targeted excerpts.
- When sanitizing subprocess environments on Windows, preserve toolchain
  discovery roots as first-class control-plane state: `ProgramFiles`,
  `ProgramFiles(x86)`, `ProgramW6432`, `CommonProgramFiles*`, `ProgramData`,
  `LOCALAPPDATA`, `SystemRoot`/`windir`, Windows SDK variables, MSVC variables,
  `INCLUDE`, `LIB`, and `PATH`. Stripping these can make LLVM/clang lose
  Windows SDK/UCRT/MSVC discovery and fail native links with missing headers
  such as `stdio.h` even though the same command works in the parent shell.
- Prefer one active long-running proof lane per resource family on Windows.
  Avoid launching many concurrent Codex subagents/session threads or repeated
  tool-discovery loops from the Windows desktop app. If discoverable-tool calls
  start repeating 403/retry errors, stop expanding orchestration and continue
  with local terminal evidence in the current thread.
- Prefer Windows-native Codex on Windows for this repository unless the task
  explicitly requires a Linux-native WSL toolchain. Use WSL only as a coherent
  all-Linux environment: Codex runtime, shell, Python/Node, repo, caches, and
  tool paths must agree on WSL semantics, and repos should live under
  `/home/<user>/...` rather than `/mnt/c/...`. Do not mix Windows Desktop,
  WSL app-server, Windows `CODEX_HOME`, `/mnt/c` workspaces, WindowsApps
  aliases, and WSL/Linux tools in one command lane.
- Before trusting any Windows + WSL thread, verify the runtime tuple with a
  bounded probe: selected project root, actual `cwd`, shell family, `pwd`,
  `uname -a` when WSL is expected, and `git rev-parse --show-toplevel`. Treat
  `C:\home\...`, `shell=powershell` in an intended WSL thread, Windows
  `codex.exe` app-server paths launched from WSL, or repo/cache paths under
  `/mnt/c/...` as a failed environment handshake. Stop broad scans and switch to
  one coherent native lane instead of trying to repair the repo from a confused
  control plane.
- Do not enable or keep Codex WSL mode merely because WSL is installed. If WSL
  is needed, first verify `wsl -l -v` shows the intended distro, the distro
  starts cleanly, and `uname -m` matches an available helper architecture. On
  Windows ARM64, treat Linux `aarch64` WSL with x64-only helper symptoms as a
  crash-loop risk; prefer Windows-native Codex or a fully verified WSL-native
  CLI lane until the helper architecture is known good.
- After a host crash, Codex update, or app restart, do not immediately resume a
  huge stale thread or launch multi-agent work. Reopen with a small bounded
  environment probe, confirm the project association and working directory are
  still valid, and only then continue long-running work. If desktop chats vanish
  or a WSL project reports "working directory missing" while the Linux path
  exists, preserve state evidence and avoid rewriting Codex session/project
  state as a first response.
- On Windows, treat `C:\Windows\System32\bash.exe`, WindowsApps `python.exe` /
  `python3.exe`, and Windows-side Node/npm shims visible from WSL as unstable
  boundary shims. Prefer `uv run --python 3.12 ...`, `py -3.12 ...`, or an
  explicit venv interpreter for Python; prefer Git Bash only when a Bash script
  is required in a Windows-native lane; prefer WSL binaries only inside a
  verified WSL-native lane.
- Treat repeated Windows shell startup failures as host-boundary incidents, not
  as prompts to retry the same PowerShell command in a loop. If Codex-spawned
  PowerShell reports `8009001d`, `ResourceUnavailable`, `InitialSessionState`,
  `GetSaferPolicy`, `AppLocker`, module-load failure, or `getaddrinfo` thread
  failure while the normal user terminal works, record the exact error once,
  switch to a known-working shell/tool path only when already available, and do
  not mutate the Codex installation to make a proof lane pass.
- Do not change Codex Desktop app settings, WSL mode, integrated shell mode,
  plugin/MCP registrations, or Codex state files while a long-running goal,
  build, test, benchmark, or recovery thread is active. If a mode change or app
  update is necessary, first stop Molt-owned workers through the memory guard or
  process sentinel, wait for running commands to exit, and preserve logs/state
  before restarting the app.
- Keep optional/heavy MCP servers and plugins manual-only during long Molt
  work. Avoid automatic registration or startup of broad tool surfaces that can
  cause Desktop to enumerate MCP tools/status during thread resume, goal checks,
  or crash recovery. Enable only the minimum MCP/plugin set needed for the
  current task, and disable speculative helpers before resuming a large thread.
- Treat plugin-cache and MCP startup load as crash-adjacent control-plane
  pressure on Windows, especially when WSL mode routes through `/mnt/c`. If
  simple prompts take tens of seconds, app startup gets hot, or thread resume
  stalls before repo commands run, capture plugin-cache size/count and MCP
  registration state, then reduce optional registrations before adding agents.
  Do not bulk-delete plugin caches or state databases without a reversible
  backup and explicit recovery intent.
- Treat repeated `codex_core_plugins::manifest` warnings during
  `built_tools.load_discoverable_tools` as crash-adjacent on Windows, even when
  the log level says WARN. For `interface.defaultPrompt` violations such as
  `maximum of 3 prompts is supported`, preserve the exact plugin path and
  manifest warning as evidence, stop adding proof-lane load, and reduce
  optional plugin registrations only through normal operator-controlled config
  after active Molt work is quiescent. Do not hand-edit cached plugin manifests,
  plugin caches, or Codex state as a first response.
- Keep Codex worktree and local-environment state boring and explicit.
  Worktrees inherit checked-in files by default, so ignored toolchains, caches,
  credentials, or setup files must come from checked-in setup scripts or an
  intentional `.worktreeinclude`, never from ambient profile state. The project
  `.codex` local-environment config must live at the project root opened in the
  app, and stale automation worktrees should be archived instead of pinned
  indefinitely.
- Treat Codex crash code `3221225786` (`0xC000013A`) on Windows as
  `STATUS_CONTROL_C_EXIT`: usually an interrupted or torn-down process, not
  proof that the last WARN line in the dialog is the root cause. Preserve the
  exact crash text, including `state_db`, plugin-manifest, or MCP warnings,
  collect nearby Codex logs, inspect Event Viewer when useful, and correlate
  with running commands, MCP enumeration, WSL mode, rollout resume, plugin
  loading, and state DB activity before changing code or deleting state.
- Treat `state db discrepancy during read_repair_rollout_path: upsert_needed`
  near a Codex crash as a rollout-state resume/repair symptom, not proof that
  the repository, Git index, or current command caused the crash. After reload,
  first verify `git status --short --branch`, recent commits, and any active
  command sessions; then continue from durable repo evidence. Do not delete
  Codex SQLite/state databases, plugin caches, rollout summaries, or thread
  state as a first response. If state repair evidence is needed, copy bounded
  Codex logs into `logs/` or `tmp/` and summarize the relevant lines.
- In native Windows multi-repo workspaces, watch for Codex-spawned Git polling
  residue before adding more agents or long-running proof lanes. If many
  overlapping `git.exe`/`conhost.exe` processes appear with `Codex.exe` as the
  parent, stop expanding orchestration, preserve command-line/parent-chain
  evidence, and only clean up processes after proving they are stale Codex-owned
  polling children rather than user-launched Git work.
- Cleanup must remain custody-aware. Do not use blanket `taskkill`, Task
  Manager-style process sweeps, or name-based kills against `codex.exe`,
  Electron/renderer helpers, app-server, node-repl, Claude, or ancestor process
  groups. Only terminate Molt-owned process groups after proving ownership.
- Do not mutate Microsoft Store package paths such as
  `C:\Program Files\WindowsApps\OpenAI.Codex_*`, Codex runtime staging under
  app-owned directories, `%APPDATA%\Codex`, `%LOCALAPPDATA%\Codex`, or
  `%USERPROFILE%\.codex` unless the user explicitly asks for Codex app repair
  and the session/auth data has been backed up or deliberately preserved.
- If Codex child processes lose DNS/networking while the same command works in
  normal PowerShell, treat it as a Windows sandbox/control-plane incident before
  blaming project dependencies. Inspect
  `%USERPROFILE%\.codex\.sandbox\setup_error.json` and
  `%USERPROFILE%\.codex\.sandbox\sandbox.log` for ACL/setup failures such as
  `SetNamedSecurityInfoW failed: 5`; preserve the evidence, avoid repeated
  dependency installs, and never try to fix it by editing `WindowsApps` package
  files or weakening repo security.
- Keep Molt build/test/bench artifact roots short, explicit, and preferably
  ASCII-only on Windows. Use repo-local canonical roots or configured external
  artifact roots rather than Codex app profile/cache/runtime directories; public
  reports include startup crashes involving non-ASCII Windows user paths and
  runtime staging/copy paths.
- If the Codex app becomes hidden, unresponsive, or crash-looping, collect
  evidence before changing state: Codex app version, Windows build,
  `%LOCALAPPDATA%\Codex\Logs`, `%USERPROFILE%\.codex\.sandbox\sandbox.log`
  when present, Crashpad report presence, and Event Viewer entries around the
  launch time. Treat diagnostics as sensitive because they may contain local
  paths, session references, auth files, or project data. Do not delete or reset
  Codex state as a first response.
- Treat app logs that stop after `Launching app` / `Appshot hotkey inactive`
  with a fresh Crashpad dump as Desktop startup-crash evidence, not as proof
  that the active repo command caused the failure. Preserve the dump/log timing,
  avoid speculative GPU/updater/profile resets during active Molt work, and
  continue with bounded local terminal evidence when the CLI remains healthy.
- Do not delete, rewrite, or hand-edit Codex state databases, rollout files,
  `session_index.jsonl`, plugin caches, or global state as first-line recovery.
  Prefer reversible stabilization: stop Molt-owned workers, remove or disable
  optional MCP registrations, avoid resuming huge stale threads, switch back to
  a coherent native runtime if WSL mode is unstable, and back up any state file
  before manual repair.
- On macOS and Linux, keep the same control-plane boundary: never raw-kill the
  Codex app, renderer, app-server, Claude, node-repl, or ancestor process group.
  Use repo custody tools for Molt-owned workers, and use platform capability
  checks for signals (`SIGKILL` exists on Unix but not all Python signal
  surfaces are portable to Windows).
- Related public references for future refresh:
  `https://developers.openai.com/codex/app/troubleshooting`,
  `https://developers.openai.com/codex/windows`,
  `https://developers.openai.com/codex/changelog`,
  `https://github.com/openai/codex/issues/16169`,
  `https://github.com/openai/codex/issues/18821`,
  `https://github.com/openai/codex/issues/20967`,
  `https://github.com/openai/codex/issues/21761`,
  `https://github.com/openai/codex/issues/21147`,
  `https://github.com/openai/codex/issues/26323`,
  `https://github.com/openai/codex/issues/21693`,
  `https://github.com/openai/codex/issues/23251`,
  `https://github.com/openai/codex/issues/25799`,
  `https://github.com/openai/codex/issues/28094`,
  `https://github.com/openai/codex/issues/28172`,
  `https://github.com/openai/codex/issues/28074`,
  `https://github.com/openai/codex/issues/28302`,
  `https://github.com/openai/codex/issues/25216`,
  `https://github.com/openai/codex/issues/14461`,
  `https://github.com/openai/codex/issues/23777`,
  `https://github.com/openai/codex/issues/16271`,
  `https://github.com/openai/codex/issues/29408`,
  `https://github.com/openai/codex/issues/17229`,
  `https://github.com/openai/codex/issues/14057`,
  `https://github.com/openai/codex/issues/14221`,
  `https://github.com/openai/codex/issues/15586`,
  `https://github.com/openai/codex/issues/27979`,
  `https://github.com/openai/codex/issues/28442`,
  `https://github.com/openai/codex/issues/20867`,
  `https://github.com/openai/codex/issues/20214`,
  `https://github.com/openai/codex/issues/26454`,
  `https://github.com/openai/codex/issues/26894`,
  `https://github.com/openai/codex/issues/15179`,
  `https://github.com/openai/codex/issues/27320`,
  `https://github.com/openai/codex/issues/27806`,
  `https://github.com/openai/codex/issues/27822`,
  `https://github.com/openai/codex/issues/28160`,
  `https://github.com/openai/codex/issues/28319`,
  `https://github.com/openai/codex/issues/23043`,
  `https://github.com/openai/codex/issues/28909`, and
  `https://community.openai.com/t/codex-windows-app-ui-closes-but-background-processes-remain-blocking-relaunch/1379095`.

## Non-Negotiable: Raise On Missing Features
- Always raise on missing features; never fallback silently.
- Never build coverage or implementations that rely on host Python in any way.
- Always assume compiled Molt binaries will run in environments with no Python installation at all.
- Stdlib modules must be Rust-native intrinsics for compiled binaries; any Python stdlib files may only be thin, intrinsic-forwarding wrappers with zero host-Python imports.
- Absolutely no CPython stdlib imports or `_py_*` fallback modules in compiled binaries (tooling-only shims are allowed).
- Intrinsics are mandatory: missing intrinsics must raise immediately (standardized `RuntimeError`), and differential tests should fail fast when intrinsics are missing.

## Intrinsics & Stdlib Lowering (Non-Negotiable)
- All stdlib behavior must lower into Rust intrinsics; Python stdlib files are only thin wrappers for argument normalization, error mapping, and capability gating.
- Load intrinsics via `src/molt/stdlib/_intrinsics.py` (module `globals()` first, then `builtins._molt_intrinsics`); do not invent alternative registries or hidden import-time side effects.
- Required behavior must use `require_intrinsic` or explicit `RuntimeError`/`ImportError` when missing; optional features must be explicit and capability-gated with clear errors, never silent fallback to host Python.
- Standardize intrinsic naming and registration through `runtime/molt-runtime/src/intrinsics/manifest.pyi`, and regenerate `src/molt/_intrinsics.pyi` plus `runtime/molt-runtime/src/intrinsics/generated.rs` via `tools/gen_intrinsics.py`.
- Prefer standardization, performance, and correctness: push hot paths and semantics into Rust, keep Python shims minimal and deterministic, and avoid CPython/host-stdlib dependencies.

## Hard Gate: Bootstrap Authority (Non-Negotiable)
- Runtime-known module bootstrap has one authority: `MODULE_IMPORT` plus the runtime import path. Do not split ownership between frontend cache/init special cases and runtime import semantics.
- Bootstrap-critical builtin type objects (`classmethod`, `staticmethod`, `property`, and similar descriptors) must come from explicit runtime bootstrap primitives/intrinsics. Do not probe-construct Python objects in stdlib bootstrap code to discover their types.
- When touching `src/molt/stdlib/builtins.py`, `src/molt/stdlib/sys.py`, `src/molt/stdlib/importlib/**`, `src/molt/stdlib/_intrinsics.py`, or frontend import lowering, add/maintain native end-to-end regressions for the exact bootstrap shape.
- If a bootstrap fix depends on control-flow quirks in a rapidly changing
  frontend/backend file, factor the contract into a first-class primitive first.

## Hard Gate: Rust-Only Stdlib Turn Blocker (Non-Negotiable)
- If a change adds or modifies stdlib behavior in `src/molt/stdlib/**`, the behavior must be implemented in Rust intrinsics first; Python code may only wire arguments, errors, and capability checks.
- Do not add Python-side fallback logic, compatibility emulation, or host-stdlib implementation paths to make tests pass.
- For every stdlib behavior change, include an explicit intrinsic mapping in the same change:
`runtime/molt-runtime/src/intrinsics/manifest.pyi` entry, Rust implementation, and regenerated `src/molt/_intrinsics.pyi` + `runtime/molt-runtime/src/intrinsics/generated.rs`.
- If no intrinsic exists for required behavior, add the intrinsic and Rust
  lowering. If the intrinsic cannot be implemented in the current environment,
  fail closed with a concrete blocker; do not proceed with a Python
  implementation.
- Before ending a turn, provide a short Rust-lowering audit for touched stdlib modules:
module path, intrinsic names used, and confirmation that no host-Python fallback path was added.

## Non-Negotiable: Prevent Shim Churn And Dynamic Creep
- Do not satisfy intrinsic enforcement with inert markers (for example string constants/comments containing `molt_*` names); gates must be satisfied by real intrinsic loader calls.
- Do not add import-only compatibility shims to mask missing runtime semantics. If semantics are missing, add/extend Rust primitives + intrinsics.
- If a stdlib module cannot execute under current runtime import execution limits, treat that as a runtime-lowering blocker; do not paper over it with Python-side API-shape shims.
- Preserve Tier-0 constraints from `docs/spec/areas/core/0000-vision.md`: no `eval`/`exec`, no implicit dynamic module execution expansion, no reflection-heavy fallback lanes in compiled binaries.
- Preserve `docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md`: breaking maximal Python dynamism is intentional. Do not reintroduce dynamism that undermines AOT size/perf/determinism.
- Any proposal to widen compile/exec/eval or unrestricted source-execution
  behavior must first prove that it preserves the AOT contract, performance,
  determinism, and capability model. Otherwise keep the fail-closed verified
  subset behavior and continue with compatible stdlib/ecosystem work.
- Treat dynamic execution (`eval`/`exec`/unrestricted code-object execution), runtime monkeypatching, and unrestricted reflection as policy-deferred work, not active burndown targets, unless the user explicitly re-prioritizes them.

## Rules Of Thumb For New Work (Non-Negotiable)
- Add or extend a runtime/compiler primitive when the behavior is a reusable low-level hot semantic.
- Expose that primitive capability to stdlib through a Rust intrinsic (manifested and registered canonically).
- Expose user-facing language/core behavior through builtins and stdlib APIs that call intrinsics/primitives, not Python reimplementations of runtime semantics.

## Mission (Non-Negotiable)
Build relentlessly with high productivity, velocity, and vision in the spirit and honor of Jeff Dean. Always build fully, completely, correctly, and performantly; avoid workarounds. Guiding question: "What would Jeff Dean do?"

## Senior Engineer Quality Bar (Non-Negotiable)
- Senior engineering leadership for this project: Jeff Dean, Chris Lattner, Tibo, and Embirico.
- Warning to every agent: remaining on this development team requires consistently delivering world-quality, production-hardened code; anything less fails role expectations.
- Treat every change as production-critical: optimize for correctness, performance, determinism, security, and maintainability at the same time.
- If you cannot yet prove a change is production-hardened, keep generating the
  missing evidence or record the exact gap plus closure plan while continuing
  non-colliding structural work.

## Strategic Target (Non-Negotiable)
- Performance target: achieve parity with or superiority over Codon.
- WASM performance target: achieve parity with or superiority over Pyodide on Molt’s canonical wasm benchmark suites and representative real-world workloads.
- Compatibility/interoperability target: get close to or match Nuitka's CPython coverage and interoperability for Molt-supported semantics.

## Version Target (Non-Negotiable)
- Molt targets Python 3.12+ semantics only. Do not spend effort on <=3.11 compatibility.
- When behavior differs across 3.12/3.13/3.14, implement explicit version gates in runtime/stdlib paths, document the choice in specs/tests, and keep the runtime aligned with the documented version.
- Model CPython version-gated *absence* (for example modules/submodules that should not import on a given version) in `src/molt/stdlib/importlib/__init__.py` (`import_module`/resolver path) as the canonical control point, not as ad-hoc per-module hacks.
- Keep version-gated absence behavior CPython-aligned: raise the same exception class (`ModuleNotFoundError`/`ImportError`) with matching message shape from the importlib boundary.

## Cross-Platform + Target Parity (Non-Negotiable)
- Treat cross-platform support as the default requirement for every runtime/compiler/stdlib change (native hosts + wasm targets).
- Keep native and wasm behavior in lockstep for supported semantics; do not land native-only behavior without explicit capability gating, documented rationale, and targeted coverage.
- For CPython `>=3.12` compatibility, enforce explicit version-gated behavior (3.12/3.13/3.14) instead of accidental drift, and add/update differential tests that exercise each gated lane.

## Compatibility Documentation Architecture (Non-Negotiable)
- Canonical compatibility documentation root is `docs/spec/areas/compat/README.md`.
- Canonical CPython reference mirror for Python `>=3.12` is `molt/docs/python_documentation` (`docs/python_documentation/` in-repo path). Use it for language, stdlib, C-API, and platform-availability alignment.
- Do not create or reintroduce legacy flat compat files at `docs/spec/areas/compat/*.md` (except `README.md` and subdirectory indexes). New/updated compat docs must live under:
  - `docs/spec/areas/compat/contracts/`
  - `docs/spec/areas/compat/surfaces/language/`
  - `docs/spec/areas/compat/surfaces/stdlib/`
  - `docs/spec/areas/compat/surfaces/c_api/`
  - `docs/spec/areas/compat/plans/`
- Treat generated files as generated-only truth; do not hand-edit semantic status in:
  - `docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md`
  - `docs/spec/areas/compat/surfaces/stdlib/asyncio_surface.generated.md`
  - `docs/spec/areas/compat/surfaces/language/core_language_pep_coverage.generated.md`
  - `docs/spec/areas/compat/surfaces/language/generator_api_coverage.generated.md`
  - `docs/spec/areas/compat/surfaces/stdlib/stdlib_platform_availability.generated.md`
- Every compatibility claim must include explicit dimensions where relevant: `py312`/`py313`/`py314`, `native`, `wasm_wasi`, `wasm_browser`, and platform notes (`linux`/`macos`/`windows`).
- Required refresh workflow when compatibility surfaces move:
  1. `python3 tools/gen_stdlib_module_union.py`
  2. `python3 tools/sync_stdlib_top_level_stubs.py --write`
  3. `python3 tools/sync_stdlib_submodule_stubs.py --write`
  4. `python3 tools/check_stdlib_intrinsics.py --update-doc`
  5. `python3 tools/gen_compat_platform_availability.py --write`
  6. `python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only`
  7. `python3 tools/check_stdlib_intrinsics.py --critical-allowlist`
  8. Sync docs in the same change: `docs/spec/STATUS.md`, `ROADMAP.md`, `docs/spec/README.md`, and `docs/INDEX.md`.
- If documentation claims conflict across compat files, resolve them in the same
  change; conflicting compatibility claims are work items, not reasons to idle.

## Jeff Dean Protege Mode (Non-Negotiable)
- Optimize for correctness, performance, and determinism before convenience. No shortcuts that degrade runtime guarantees.
- Default path is native Molt lowering + Rust runtime. Treat CPython bridge paths as explicit, opt-in compatibility layers only.
- Prefer recompiled C-extensions against a `libmolt` C-API subset over any embedded CPython strategy.
- Any bridge usage must be capability-gated, off by default, and always visible in logs/metrics.
- Measure performance impacts with benchmarks; treat regressions as failures and iterate until green.

## Project Structure & Module Organization
- `src/molt/` contains the Python compiler frontend and CLI (`cli.py`).
- `runtime/` hosts Rust crates for the runtime and object model (`molt-runtime`, `molt-obj-model`, `molt-backend`).
- `tests/` holds Python tests, including differential suites in `tests/differential/` and smoke/compliance tests.
- `examples/` contains small programs used in docs and manual validation.
- `docs/spec/` is the architecture and runtime specification set; treat it as
  routing that must be refreshed from live code, executable tests, and generated
  evidence.
- `tools/` includes developer scripts like `tools/dev.py`.
- Keep Rust crate entrypoints (`lib.rs`) thin; place substantive runtime/backend logic in focused modules under `src/` and re-export from `lib.rs`.
- Standardize naming: Python modules use `snake_case`, Rust crates use `kebab-case`, and paths reflect module names (avoid ad-hoc casing).

## Key Docs
- [docs/CANONICALS.md](docs/CANONICALS.md): must-read documents for new work.
- [docs/INDEX.md](docs/INDEX.md): documentation map and entry points.
- [docs/python_documentation/](docs/python_documentation/): local canonical CPython documentation mirror for Python >= 3.12 reference work (path: `molt/docs/python_documentation`).
- [docs/spec/areas/compat/README.md](docs/spec/areas/compat/README.md): canonical compatibility documentation architecture and upkeep workflow.
- [docs/spec/areas/compat/surfaces/language/language_surface_matrix.md](docs/spec/areas/compat/surfaces/language/language_surface_matrix.md): language surface index and coverage dimensions.
- [docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_index.md](docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_index.md): stdlib compatibility source-of-truth index.
- [docs/spec/areas/compat/surfaces/c_api/c_api_surface_index.md](docs/spec/areas/compat/surfaces/c_api/c_api_surface_index.md): C-API compatibility source-of-truth index.
- [docs/spec/areas/compat/plans/stdlib_lowering_plan.md](docs/spec/areas/compat/plans/stdlib_lowering_plan.md): canonical intrinsic-first stdlib lowering program.
- [docs/spec/README.md](docs/spec/README.md): spec index by area.
- [CONTRIBUTING.md](CONTRIBUTING.md): workflow expectations and the change impact matrix.
- [docs/DEVELOPER_GUIDE.md](docs/DEVELOPER_GUIDE.md): architecture map, layer ownership, and integration checklist.
- [docs/spec/areas/core/0000-vision.md](docs/spec/areas/core/0000-vision.md) and [docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md](docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md): vision, scope, and explicit break policy.
- [docs/spec/STATUS.md](docs/spec/STATUS.md) and [ROADMAP.md](ROADMAP.md): canonical current scope/limits and the active forward-looking plan.
- [docs/ROADMAP.md](docs/ROADMAP.md): detailed archive/reference roadmap context.
- [docs/architecture/gpu-primitive-stack.md](docs/architecture/gpu-primitive-stack.md) and [docs/spec/areas/perf/0513_GPU_PARALLELISM_AND_MLIR.md](docs/spec/areas/perf/0513_GPU_PARALLELISM_AND_MLIR.md): GPU primitive, ShapeTracker, fusion, reduction-domain, MLIR/MIL, and renderer contracts.
- [docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md](docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md): minimum must-pass gate matrix and mandatory memory-guard custody for test execution.
- [docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md](docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md): determinism/security enforcement checklist.
- [docs/OPERATIONS.md](docs/OPERATIONS.md): remote access, logging, benchmarks, progress reports, and multi-agent workflow.
- [docs/ops/MULTI_AGENT_COORDINATION.md](docs/ops/MULTI_AGENT_COORDINATION.md): canonical multi-agent verification coordination protocol; read before long differential, conformance, regrtest, benchmark, or validation lanes.
- [docs/BENCHMARKING.md](docs/BENCHMARKING.md): benchmarking overview.

## Build, Test, and Development Commands
- `cargo build --release --package molt-runtime`: build the Rust runtime used by compiled binaries.
- `export PYTHONPATH=src`: make the Python package importable from the repo root.
- `python3 -m molt.cli build examples/hello.py`: compile a Python example to a native binary.
- `./hello_molt`: run the compiled output from the previous step.
- `python3 -m molt.cli build --target wasm --linked examples/hello.py`: emit `output.wasm` and `output_linked.wasm` for wasm targets (linked requires `wasm-ld` + `wasm-tools`).
- `python3 -m molt.cli build --target wasm --linked --linked-output dist/app.wasm examples/hello.py`: customize the linked output path.
- `python3 -m molt.cli build --target wasm --require-linked examples/hello.py`: enforce linked output and remove the unlinked artifact after linking.
- `molt build --module mypkg`: compile a package/module entrypoint (uses `mypkg.__main__` when present).
- Vendored deps in `vendor/` are added to module roots and `PYTHONPATH` automatically (or set `MOLT_MODULE_ROOTS` explicitly).
- `molt run --timing examples/hello.py`: compile+run the native binary and emit build/run timing (no CPython fallback).
- `molt compare examples/hello.py -- --arg 1`: compare CPython vs Molt output with separate build/run timing (CPython required for baseline only).
- `molt bench --script examples/hello.py`: run the bench harness on a custom script.
- `MOLT_TRUSTED=1`, `molt run --trusted`, `molt build --trusted`, `molt diff --trusted`, or `molt test --trusted`: disable capability checks for trusted native deployments.
- Build cache determinism is now enforced by default in the CLI (`PYTHONHASHSEED=0`) to stabilize cache keys across invocations. Override with `MOLT_HASH_SEED=<value>` (set `MOLT_HASH_SEED=random` to opt out).
- Lockfile verification (`uv lock --check`, `cargo metadata --locked`) is cached under `<CARGO_TARGET_DIR>/lock_checks/` when `CARGO_TARGET_DIR` is set (otherwise `target/lock_checks/`); remove those files when you need to force a full lock re-check.
- Development profile routing: `--profile dev` maps to Cargo profile `dev-fast` by default (override with `MOLT_DEV_CARGO_PROFILE`; release uses `MOLT_RELEASE_CARGO_PROFILE`).
- Runtime/backend Cargo rebuilds use lock files under `<CARGO_TARGET_DIR>/.molt_state/build_locks/` to prevent duplicate rebuild storms across concurrent agents.
- Native backend compiles use a local backend daemon by default (`MOLT_BACKEND_DAEMON=1`) to amortize Cranelift startup; tune with `MOLT_BACKEND_DAEMON_START_TIMEOUT` and `MOLT_BACKEND_DAEMON_CACHE_MB`.
- Build/daemon fingerprints + lock state live under `<CARGO_TARGET_DIR>/.molt_state/` (or `MOLT_BUILD_STATE_DIR` when set). Daemon sockets default to a local temp dir (`MOLT_BACKEND_DAEMON_SOCKET_DIR`) to avoid external filesystems that do not support Unix sockets; identity sidecars and logs remain under build state, while legacy `.pid` files are cleanup debris and never signal authority.
- `molt clean` dry-runs canonical ignored artifact cleanup; `molt clean --apply` deletes those ignored artifacts; add `--kill-processes` when stale repo-scoped Molt build/test/bench processes must be drained first.
- `tools/dev.py lint`: run `ruff` checks, `ruff format --check`, and `ty check` via `uv run` (Python 3.12).
- `tools/dev.py test`: run the Python test suite (`pytest -q`) via `uv run` on Python 3.12/3.13/3.14; direct pytest invocations re-exec under `tools/memory_guard.py` before collection when not already guarded.
- `python3 tools/cpython_regrtest.py --clone`: run CPython regrtest against Molt (logs under `logs/cpython_regrtest/`); defaults to `python -m molt.cli run`.
- `python3 tools/cpython_regrtest.py --uv --uv-python 3.12 --uv-prepare --coverage`: run regrtest with uv-managed Python + coverage.
- `cargo test`: run Rust unit tests for runtime crates.
- `uv sync --group bench --python 3.12`: install optional benchmark deps before running `tools/bench.py` (PyPy/Codon/Nuitka/Pyodide lanes are optional and auto-skipped when unavailable).
- If `uv run` panics in sandboxed or restricted environments, reuse the existing
  environment by setting `UV_NO_SYNC=1`. Prefer `UV_CACHE_DIR=/tmp/uv-cache` inside
  the sandbox when external volumes are blocked.
- If the panic mentions `system-configuration` (macOS proxy lookup), pin explicit
  proxy envs to bypass system proxy detection, for example:
  `HTTP_PROXY=http://127.0.0.1:9 HTTPS_PROXY=http://127.0.0.1:9 ALL_PROXY=http://127.0.0.1:9 NO_PROXY=localhost,127.0.0.1`.
- If the panic is due to missing deps, run `uv sync --group dev --python 3.12`
  locally (outside the sandbox) to populate `.venv`, then rerun with `UV_NO_SYNC=1`.

## Concurrent Development (Required for Multi-Agent)

`MOLT_SESSION_ID` is available when a workflow needs session-scoped build state, but the canonical repository target root remains `target/`. Export it before any build command when another agent may be active:

```bash
export MOLT_SESSION_ID="<unique-name>"  # e.g., "agent-1", "debug-session", UUID
```

**What it does:**
- Keeps build state and daemon/socket isolation session-scoped without changing the canonical `target/` guidance
- Isolates daemon socket — no kill/restart conflicts between sessions
- Isolates build state, lock-check caches, and staleness checks — fully independent build lifecycle
- Disables `cargo clean` — incremental builds only, no binary deletion

**Pre-build step** (first build in a new session takes ~5 min for full compile):
```bash
export MOLT_SESSION_ID="agent-1"
cargo build --profile release-fast -p molt-backend --features native-backend --target-dir target
```

**Daemon management:**
- Never use raw `pkill`/PID/socket heuristics for backend-daemon cleanup.
- Use verified identity custody (`src/molt/backend_daemon_custody.py`) or the
  guarded process sentinel (`molt clean --apply --kill-processes`) when cleanup
  is required.
- Each session's daemon has a unique socket path derived from the session ID

**Without it:** All sessions share the canonical `target/` and the same daemon. One agent's `cargo build` can block others, and one agent's rebuild can delete artifacts that other sessions depend on.

**Resource limits:** Maximum 2 concurrent builds (OOM risk on machines with less than 128GB RAM).

**Rule:** If you are an agent and another agent may be running, ALWAYS set `MOLT_SESSION_ID` before ANY build command.

**Coordination discovery:** Before intentionally starting long differential,
conformance, regrtest, benchmark, or validation work, read
[docs/ops/MULTI_AGENT_COORDINATION.md](docs/ops/MULTI_AGENT_COORDINATION.md),
create/update `logs/agents/<task>/` with `tools/new-agent-task.sh <task>`, and
record whether you own a targeted proof lane or the single broad-sweep
coordinator role for the shared target root. Use
`uv run --python 3.12 python tools/agent_coordination.py env`, then
`uv run --python 3.12 python tools/agent_coordination.py scan` or
`uv run --python 3.12 python tools/agent_coordination.py check` to inspect
machine-readable task claims before launching broad proof work. Do not invoke
this protocol for ordinary implementation slices; one bounded proof for the
owned structural arc is enough unless a failure exposes a wider contract risk.

**Git discipline (non-negotiable):**
- NEVER revert unstaged changes — they are partner work
- Always `git add` immediately after writing files (commit hooks are read-only by default; explicit staging keeps owned changes atomic)
- Write + git add in the same operation using `&&` chaining

## Build Profile Policy (Non-Negotiable)
- Development workflows must use `--profile dev` for `molt build`, `molt run`, `molt compare`, `molt diff`, and `molt test --suite diff`, unless explicitly validating production artifacts.
- Production benchmarks, release validation, and published binaries must use `--profile release`.
- Do not silently switch profiles in wrappers/harnesses; profile selection must be explicit and reproducible in command lines/config.

## No CPython Fallback (Non-Negotiable)
- Molt-compiled binaries must run on systems without Python installed; do not depend on `python`, `sys.executable`, or CPython at runtime.
- Never implement CPython fallback/bridging in CLI, runtime, tests, or tooling. Unsupported constructs must be compile-time errors or `bridge_unavailable` runtime exits when `--fallback bridge` is explicitly requested.
- CPython is only allowed for baseline comparisons (`molt compare`, `tests/molt_diff.py`, CPython regrtest); it must be explicit and never used to execute Molt binaries.

## Runtime Capability System (Non-Negotiable)

Molt uses a capability-based security model. Programs must have explicit capabilities granted to perform sensitive operations.

### Capability Tiers (simplest way to configure)
| Tier | Env Var | What It Grants |
|------|---------|----------------|
| `safe` | `MOLT_CAPABILITY_TIER=safe` | Read-only: env.read, fs.read, fs.stat, fs.readdir, os.getcwd, os.getpid, time.wall, glob, uname |
| `standard` | (default) | Safe + writes: fs.write, env.write, os.mkdir, tempfile.*, signal.*, thread.*, shutil.copy/move |
| `full` | `MOLT_CAPABILITY_TIER=full` | Standard + network: net.*, ssl.*, websocket.*, process.exec, db.*, ffi.*, select.* |

### Tier → Capability Mapping (exhaustive)

**`safe` tier** — Read-only operations. No network, no writes, no exec. Suitable for Cloudflare Workers and sandboxed environments.

| Capability | Description | Developer Notice |
|-----------|-------------|-----------------|
| `env.read` | Read environment variables | Required for `sys.flags`, `os.environ` reads |
| `env.len` / `env.snapshot` | Count/snapshot env | |
| `fs.read` | Read files | Required for `import`, `open()` for reading |
| `fs.stat` | File stat/exists checks | Required for `os.path.exists()`, `Path.is_file()` |
| `fs.readdir` | List directory contents | Required for `os.listdir()`, `Path.iterdir()` |
| `glob.glob` | Glob pattern matching | |
| `os.getcwd` / `os.access` | CWD and access checks | |
| `os.getpid` / `os.getppid` / `os.getuid` / `os.geteuid` / `os.getgid` / `os.getegid` | Process/user IDs | |
| `os.getlogin` / `os.getpgrp` / `os.getloadavg` / `os.uname` | System info | |
| `os.readlink` / `os.listdir` / `os.scandir` / `os.walk` | Directory traversal | |
| `shutil.which` | Locate executable in PATH | |
| `time.wall` | Wall-clock time | |

**`standard` tier** (default for `molt run`) — Adds write operations.

| Capability | Description | Developer Notice |
|-----------|-------------|-----------------|
| *All `safe` capabilities* | | |
| `env.write` / `env.clear` / `env.popitem` | Modify environment | Writes to process-local mirror only |
| `env.expanduser` / `env.expandvars` | Expand `~` and `$VAR` | |
| `fs.write` | Write/create files | **Creates/modifies files on disk** |
| `os.mkdir` / `os.makedirs` | Create directories | |
| `os.rmdir` / `os.removedirs` | Remove directories | **Destructive** |
| `os.link` / `os.symlink` / `os.chmod` / `os.utime` / `os.umask` | File metadata | |
| `os.chdir` | Change working directory | **Affects all subsequent relative paths** |
| `os.truncate` / `os.ftruncate` / `os.lseek` | File I/O | |
| `shutil.copy` / `shutil.copyfile` / `shutil.copytree` / `shutil.move` | Copy/move files | **shutil.move is destructive to source** |
| `tempfile.*` | Create temp files/dirs | Cleaned up on exit |
| `signal.*` | Signal handling | **Can affect process behavior** |
| `thread.spawn` / `thread.shared` | Threading | **Concurrent access to shared state** |

**`full` tier** (`MOLT_CAPABILITY_TIER=full` or `MOLT_TRUSTED=1`) — Adds network, process exec, database, FFI.

| Capability | Description | Developer Notice |
|-----------|-------------|-----------------|
| *All `standard` capabilities* | | |
| `net.bind` / `net.listen` | Listen on network ports | **Exposes host to inbound connections** |
| `net.connect` / `net.asyncio` / `net.poll` | Outbound network | **Can exfiltrate data** |
| `ssl.read` / `ssl.write` | TLS operations | |
| `websocket.connect` | WebSocket connections | **Persistent outbound connections** |
| `process.exec` / `process.asyncio` | Execute subprocesses | **Arbitrary command execution** |
| `os.kill` / `os.waitpid` | Process management | **Can terminate other processes** |
| `select.*` | I/O multiplexing | |
| `db.read` / `db.write` / `db.query` / `db.exec` | Database access | **Can read/modify persistent data** |
| `ffi.unsafe` / `ffi.require` / `ffi.sizeof` | FFI | **Can call arbitrary native code** |
| `fcntl.fcntl` | File descriptor control | |
| `python.bridge` | CPython bridge | Opt-in compatibility layer only |

### Security Invariants
- Capabilities are loaded **eagerly at runtime init**, before any user code runs
- Environment writes from user code go to a **local mirror**, not `std::env::set_var`
- A program **cannot escalate** its tier at runtime by modifying `MOLT_TRUSTED` or `MOLT_CAPABILITIES`
- `exec()`/`eval()`/`compile()` are **never supported** regardless of tier — Molt is AOT-only
- Runtime monkeypatching and unrestricted reflection are **never supported** regardless of tier

### Environment Variables
- `MOLT_CAPABILITY_TIER=safe|standard|full` — set capability tier (default: `standard`)
- `MOLT_TRUSTED=1` — equivalent to `full` tier, grants everything
- `MOLT_CAPABILITIES=cap1,cap2,...` — additive individual capabilities on top of tier
- `--trusted` flag on `molt run`/`molt build` — equivalent to `MOLT_TRUSTED=1`

### Error Messages
When a capability is denied, the error includes a fix suggestion:
```
PermissionError: missing 'net.connect' capability. Use --trusted, MOLT_TRUSTED=1, or MOLT_CAPABILITY_TIER=full
```

### Never Supported (by design, per Tier-0 constraints)
- `exec()` / `eval()` / `compile()` — Molt is AOT; no runtime code execution
- Runtime monkeypatching — class/module modification after compilation is not supported
- Unrestricted reflection — only compile-time-visible attributes are accessible
- `module.exec` — dynamic module execution is policy-deferred, not active

### Development Workflow
- `molt run` defaults to `standard` tier — most dev workflows just work
- `molt deploy --cloudflare` should use `safe` tier for edge security
- `MOLT_TRUSTED=1` or `--trusted` for anything that needs network/exec
- `MOLT_CAPABILITIES=net.connect,db.read` for fine-grained production control

## Binary Analysis & Debugging Tools (Available)
- **WASM inspection:** `wasm-objdump` (`-x` headers, `-d` disassemble, `-s` sections), `wasm-dis`, `wasm-validate`, `wasm-tools dump/validate` — installed via `brew install wabt wasm-tools`
- **Native binary analysis:** `gobjdump`, `gnm`, `greadelf`, `gsize`, `gaddr2line` at `/opt/homebrew/opt/binutils/bin/` — installed via `brew install binutils`. Add to PATH: `export PATH="/opt/homebrew/opt/binutils/bin:$PATH"`
- **Reverse engineering:** `r2` (radare2) — installed via `brew install radare2`
- **LLVM tools:** `llvm-objdump`, `llvm-nm`, `llvm-readobj`, `llvm-dis` at `/opt/homebrew/opt/llvm/bin/` (if installed)
- **Debugger:** `lldb` (bundled with Xcode) — use for crash analysis: `lldb /path/to/binary -o run -o bt -o quit`
- **Timeout:** `gtimeout` (coreutils) — use to prevent infinite-loop hangs: `gtimeout 10s ./binary`
- **Memory:** `vm_stat` for system memory, `leaks` for leak detection, `vmmap` for virtual memory maps

## Tooling Add-ons (Optional)
- `uv run pre-commit install` and `uv run pre-commit run -a`: enable repo hooks. Hooks are read-only by default; run explicit format/fix commands before staging when a check fails.
- `python3 tools/check_stdlib_intrinsics.py`: validate stdlib/intrinsic coverage (use `--fallback-intrinsic-backed-only` for strict checks, `--critical-allowlist` for gating, and `--update-doc` to refresh docs).
- `python3 tools/check_dynamic_policy.py`: enforce dynamic-execution policy guardrails (no accidental policy drift for `eval`/`exec`, monkeypatching, or unrestricted reflection lanes).
- `python3 tools/sync_stdlib_top_level_stubs.py --write` and `python3 tools/sync_stdlib_submodule_stubs.py --write`: sync stdlib stub inventories from the manifest.
- `python3 tools/gen_stdlib_module_union.py`: regenerate the stdlib module union list used by stub syncing and checks.
- `python3 tools/gen_compat_platform_availability.py --write`: regenerate CPython 3.12/3.13/3.14 stdlib Availability matrix at `docs/spec/areas/compat/surfaces/stdlib/stdlib_platform_availability.generated.md`.
- `python3 tools/diff_coverage.py`: generate [tests/differential/COVERAGE_REPORT.md](tests/differential/COVERAGE_REPORT.md).
- `python3 tools/bench_diff.py <old.json> <new.json> --top 10 --json-out <path>`: diff two benchmark JSON artifacts and emit a summary report.
- `python3 tools/bench_friends.py --manifest bench/friends/manifest.toml --suite <id>`: run friend benchmark suites with the pinned manifest (use `--json-out`/`--summary-out` to capture results).
- `python3 tools/diff_memory_report.py --input <artifact-root>/rss_metrics.jsonl --top 10`: summarize top RSS offenders from diff RSS metrics.
- `python3 tools/check_type_coverage_todos.py`: ensure type/stdlib TODOs are mirrored in [ROADMAP.md](ROADMAP.md).
- `uv run --python 3.12 python tools/compile_progress.py --clean-state`: capture standardized compile-progress metrics.
- `python3 tools/profile.py`: repeatable CPU/alloc profiling runs.
- `python3 tools/runtime_safety.py clippy|miri|fuzz --target string_ops --runs 10000`: runtime safety gates.
- `cargo audit` and `cargo deny check`: Rust supply-chain audits.
- `uv run pip-audit`: Python dependency audit (run after `uv sync --group dev`).
- `cargo nextest run -p molt-runtime --all-targets`: faster Rust test runner.
- `export RUSTC_WRAPPER=sccache`: enable Rust compile caching (check stats with `sccache -s`).
- The CLI auto-enables `sccache` when available (`MOLT_USE_SCCACHE=auto`); set `MOLT_USE_SCCACHE=0` to disable or `MOLT_USE_SCCACHE=1` to require it in your shell setup.
- `uv run --python 3.12 python tools/throughput_matrix.py`: run the build-throughput matrix (single-agent vs concurrent, wrapper on/off, dev/release) and write JSON artifacts under the configured artifact root. Prefer `--shared-target-dir <apfs/ext4 path>` for faster Rust incremental compiles.
- `eval "$(tools/throughput_env.sh --print)"` (or `tools/throughput_env.sh --apply`): bootstrap throughput env defaults with canonical artifact/cache roots, shared target dir, shared diff target (`MOLT_DIFF_CARGO_TARGET_DIR`), and `sccache` sizing tuned for local or external roots.
- Fast multi-agent bootstrap (recommended before long diff sweeps): `tools/throughput_env.sh --apply && uv run --python 3.12 python -m molt.cli build --profile dev examples/hello.py --cache-report`.
- Throughput bootstrap also sets `CARGO_INCREMENTAL=0` by default to improve cross-run/cacheability in highly concurrent workflows; override to `1` when investigating local incremental-only behavior.
- `python3 tools/molt_cache_prune.py`: enforce Molt cache retention policy (defaults: external `200G` + `30` days; local `30G` + `30` days).
- `cargo bloat -p molt-runtime --release` and `cargo llvm-lines -p molt-runtime`: size attribution.
- `cargo flamegraph -p molt-runtime --bench ptr_registry`: native flamegraphs.

## WASM Tooling
- Bench harness: `tools/bench_wasm.py` (`--linked` uses `wasm-ld` when available; `--require-linked` aborts if linking fails).
- Linking helper: `tools/wasm_link.py` (single-module linking via `wasm-ld`).
- Profiling helper: `tools/wasm_profile.py` (Node `--cpu-prof` for wasm benches).
- Inspect binaries: `wasm-tools print <file.wasm>` for imports/exports/sections.
- Size analysis: `twiggy top <file.wasm>` for WASM size attribution.
- Size optimization: `wasm-opt -Oz -o output.opt.wasm output.wasm` (Binaryen).
- Runtime harness: `wasm/run_wasm.js` (Node/WASI; prefers `*_linked.wasm` when present, set `MOLT_WASM_PREFER_LINKED=0` to opt out).
- Runner prefers linked wasm when `*_linked.wasm` exists next to the input (disable with `MOLT_WASM_PREFER_LINKED=0`).
- Linked builds require `wasm-ld` and `wasm-tools` (install via Homebrew `llvm` + `wasm-tools` or Cargo).
- Override relocatable table base with `MOLT_WASM_TABLE_BASE=<u32>` (defaults to runtime table size when available).

## Coding Style & Naming Conventions
- Python: 4-space indentation, `ruff` line length 88, target version 3.13, and strict typing via `ty`.
- Formatting: use `ruff format` (black-style) as the canonical formatter before builds to avoid inconsistent quoting or style drift.
- Rust: format with `cargo fmt` and keep clippy clean (`cargo clippy -- -D warnings`).
- Tests follow `test_*.py` naming; keep test modules in `tests/` or subdirectories like `tests/differential/`.

## Stdlib Submodule Policy
- Treat stdlib submodules (e.g., `asyncio.locks`) as first-class entries in the compatibility matrix.
- Register submodules explicitly (create module objects, add to `sys.modules`, and attach on the parent package) instead of relying on dynamic attribute lookups.
- Keep submodules deterministic and capability-gated where they touch host I/O, OS, or process boundaries.
- Update [docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md](docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md) when submodule coverage changes.

## Runtime Locking & Unsafe Policy
- Runtime mutation requires the GIL token; do not bypass it.
- Unsafe code must live in provenance/object modules; other runtime modules should be safe Rust.
- Rust-owned opaque handles must be exposed with `opaque_handle_bits`, which
  registers the pointer and returns an immediate-int registry id. Only real
  Molt heap objects may use pointer-tagged bits such as `bits_from_ptr`; never
  return `bits_from_ptr(Box::into_raw(...))` for a lock, async stream,
  subprocess handle, decimal/fraction/ipaddress/contextlib/graphlib/select
  state, or any other non-Molt Rust allocation.
- When changing handle resolution or the pointer registry, run strict provenance checks (Miri when available) and the lock-sensitive bench subset.

## Testing Guidelines
- Run differential parity through the harness, not raw pytest: `uv run --python 3.12 python -u tests/molt_diff.py <file_or_dir>`.
- Differential lane contract:
  - `tests/differential/basic`: core language + builtins.
  - `tests/differential/stdlib`: stdlib modules/submodules.
  - `tests/differential/moltlib`: Molt-only APIs (optional lane for non-CPython surfaces).
- Do not add new tests under retired lanes: `tests/differential/planned`, `tests/differential/core`, or `tests/differential/scoping`.
- After adding or moving differential tests, run lane integrity gates:
  - `python3 tools/check_differential_suite_layout.py`
  - `python3 tools/gen_diff_lanes.py`
- Expected-failure governance for dynamic semantics:
  - Register intentional dynamic gaps (for example `exec`/`eval`) only in `tools/stdlib_full_coverage_manifest.py` under `TOO_DYNAMIC_EXPECTED_FAILURE_TESTS`.
  - Keep `tests/test_molt_diff_expected_failures.py` green so manifest coverage and `XFAIL`/`XPASS` behavior stay enforced.
- Run the core-lane lowering gate with the current manifest path:
  - `python3 tools/check_core_lane_lowering.py --manifest tests/differential/basic/CORE_TESTS.txt`
- NON-NEGOTIABLE: Differential work MUST use canonical artifact roots (`CARGO_TARGET_DIR`, `MOLT_DIFF_ROOT`, `MOLT_DIFF_TMPDIR`, `MOLT_CACHE`) and must not spill ad hoc artifacts elsewhere in the repo.
- NON-NEGOTIABLE: Differential memory profiling is default-on; set `MOLT_DIFF_MEASURE_RSS=0` only for an explicit local investigation.
- NON-NEGOTIABLE: Treat memory blowups as failures; if RSS climbs rapidly or threatens system stability, terminate the diff run early (kill the harness) and record the abort plus last-known RSS metrics in [tests/differential/INDEX.md](tests/differential/INDEX.md).
- NON-NEGOTIABLE: Use the adaptive diff memory guard and adaptive per-process OS rlimit; direct pytest and harnessed test runs must not bypass RSS custody, and cleanup must never kill Claude, Codex, or other host control-plane process groups.
  - macOS/Linux: let `tests/molt_diff.py` apply its adaptive child limit by default; use `MOLT_DIFF_RLIMIT_GB`/`MOLT_DIFF_RLIMIT_MB` only for a deliberate narrower cap, or `MOLT_DIFF_RLIMIT_GB=0` only for an explicit local investigation.
  - If the adaptive limit is hit or memory pressure occurs, inspect the guard telemetry, reduce parallelism (`--jobs 2` or `--jobs 1`) only as a containment step, and fix the underlying allocation growth.
- Differential artifacts can be redirected to an external volume to avoid local disk pressure.
  - Set `MOLT_DIFF_ROOT` to an absolute path; all per-test build artifacts, caches, and temp dirs will live under it.
  - Optional: set `MOLT_DIFF_TMPDIR` to override only the temp root.
  - Optional: set `MOLT_CACHE` to a shared path to reuse Molt codegen artifacts across tests (dramatically faster on large suites).
  - Optional: set `MOLT_DIFF_KEEP=1` to preserve per-test artifacts after each run.
  - Optional: set `MOLT_DIFF_TRUSTED=1` to force trusted mode for diff runs (defaults to trusted unless `MOLT_DEV_TRUSTED=0`).
  - Default to a shorter timeout unless a test is known to be slow: `MOLT_DIFF_TIMEOUT=180` (bump per-test only when needed).
  - Optional: set `MOLT_DIFF_RLIMIT_GB=<n>` or `MOLT_DIFF_RLIMIT_MB=<n>` to override the adaptive per-process OS rlimit; set `MOLT_DIFF_RLIMIT_GB=0` only for an explicit local investigation.
  - Optional: set `MOLT_DIFF_MEM_PER_JOB_GB=<n>` to tune auto-parallelism by scheduler budget (default: adaptive cumulative budget divided across CPU capacity, capped below the process-tree kill ceiling).
  - Optional: set `MOLT_DIFF_MAX_JOBS=<n>` to hard-cap the auto-selected job count.
  - Optional: set `MOLT_DIFF_ORDER=auto|name|size-asc|size-desc` to control scheduling order (default: auto).
  - Optional: set `MOLT_DIFF_FAILURES=<path>` or pass `--failures-output <path>` to capture a failure queue file.
  - Optional: set `MOLT_DIFF_WARM_CACHE=1` or pass `--warm-cache` to prebuild all tests once to seed `MOLT_CACHE` before the diff run (useful for large suites).
  - Optional: set `MOLT_DIFF_RETRY_OOM=1` (default) or pass `--no-retry-oom` to disable the one-shot OOM retry with `--jobs 1`.
  - Optional: set `MOLT_DIFF_SUMMARY=<path>` or read `MOLT_DIFF_ROOT/summary.json` for the LLM-friendly summary sidecar (includes RSS aggregates when enabled).
  - Optional: set `MOLT_DIFF_ALLOW_RUSTC_WRAPPER=1` to allow `RUSTC_WRAPPER`/`sccache` during diff runs; by default the harness disables wrappers for maximum portability/reproducibility.
  - Optional: set `MOLT_DIFF_LOG_PASSES=1` to keep per-test logs for passing tests when `--log-dir` is used (default prunes pass logs to reduce clutter).
  - Optional: set `MOLT_DIFF_CARGO_TARGET_DIR=<abs path>` to force diff-run Cargo artifacts into a shared target dir; default comes from throughput bootstrap (`MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR`).
  - Optional (recommended on macOS when many agents are active): explicitly set both `CARGO_TARGET_DIR` and `MOLT_DIFF_CARGO_TARGET_DIR` to the same shared path to avoid accidental fallback to ad-hoc/default targets that can trigger duplicate rebuild storms.
  - Optional: set `MOLT_DIFF_RUN_LOCK_WAIT_SEC=<seconds>` to control how long a diff run waits for the shared run lock (`<CARGO_TARGET_DIR>/.molt_state/diff_run.lock`, default 900s). Set `MOLT_DIFF_RUN_LOCK_POLL_SEC=<seconds>` to tune lock polling cadence.
  - Optional: set `MOLT_DIFF_BACKEND_DAEMON=1|0` to force daemon mode for diff runs. Default is platform-safe auto (`0` on macOS, `1` elsewhere) to avoid dyld import-format instability.
  - Optional: set `MOLT_DIFF_QUARANTINE_ON_DYLD=1` to force cold target/state quarantine after a dyld incident. Default keeps shared target/cache and disables daemon only.
  - Optional: set `MOLT_DIFF_DYLD_LOCAL_FALLBACK=1|0` to enable/disable local `/tmp` retry + quarantine lanes for dyld incidents. Default is `1` on macOS (`0` elsewhere).
  - Optional: set `MOLT_DIFF_DYLD_LOCAL_ROOT=<abs path>` to override the local dyld quarantine root (default: `/tmp/molt_diff_dyld`).
  - Optional: set `MOLT_DIFF_FORCE_NO_CACHE=1|0` to force/disable `--no-cache` in diff runs. Default is cache-enabled on all platforms; dyld guard/retry can force no-cache for the incident-scoped retry.
  - Optional cleanup for interrupted/crashed sessions before starting a new long run: use the custody-aware sentinel, for example `python3 tools/process_sentinel.py --once --stale-orphan-sec 3600 --stale-pytest-sec 900`. Keep one supervising diff run per shared target to minimize contention and memory spikes.
- Example (configured artifact root + shared cache + temp root): `ARTIFACT_ROOT=${MOLT_EXT_ROOT:-$PWD} CARGO_TARGET_DIR=${ARTIFACT_ROOT}/target MOLT_CACHE=${ARTIFACT_ROOT}/.molt_cache MOLT_DIFF_ROOT=${ARTIFACT_ROOT}/tmp/diff MOLT_DIFF_TMPDIR=${ARTIFACT_ROOT}/tmp MOLT_DIFF_KEEP=1 MOLT_DIFF_TIMEOUT=180 uv run --python 3.12 python -u tests/molt_diff.py tests/differential/basic`.
- Example (RSS metrics): `ARTIFACT_ROOT=${MOLT_EXT_ROOT:-$PWD} CARGO_TARGET_DIR=${ARTIFACT_ROOT}/target MOLT_CACHE=${ARTIFACT_ROOT}/.molt_cache MOLT_DIFF_ROOT=${ARTIFACT_ROOT}/tmp/diff MOLT_DIFF_TMPDIR=${ARTIFACT_ROOT}/tmp MOLT_DIFF_KEEP=1 MOLT_DIFF_TIMEOUT=180 uv run --python 3.12 python -u tests/molt_diff.py tests/differential/basic`.
  - Example (watch RSS during run): `ps -o pid=,rss=,command= -p <PID> | awk '{printf "pid=%s rss_kb=%s cmd=%s\n",$1,$2,$3}'` (record spikes in [tests/differential/INDEX.md](tests/differential/INDEX.md)).
  - Example (kill on blowup): stop the Molt-owned harness through the memory guard or custody-aware sentinel, then log the abort plus last-known RSS in [tests/differential/INDEX.md](tests/differential/INDEX.md). Raw PID kills are last-resort triage only after proving the process is Molt-owned and outside Claude/Codex/host control-plane groups.
- Example (multi-target list, auto-parallel): `ARTIFACT_ROOT=${MOLT_EXT_ROOT:-$PWD} CARGO_TARGET_DIR=${ARTIFACT_ROOT}/target MOLT_CACHE=${ARTIFACT_ROOT}/.molt_cache MOLT_DIFF_ROOT=${ARTIFACT_ROOT}/tmp/diff MOLT_DIFF_TMPDIR=${ARTIFACT_ROOT}/tmp MOLT_DIFF_TIMEOUT=180 uv run --python 3.12 python -u tests/molt_diff.py tests/differential/basic/augassign_inplace.py tests/differential/basic/container_mutation.py tests/differential/basic/ellipsis_basic.py`
  - Example (parallel full sweep + live log + aggregate log + per-test logs):
    `ARTIFACT_ROOT=${MOLT_EXT_ROOT:-$PWD} CARGO_TARGET_DIR=${ARTIFACT_ROOT}/target MOLT_CACHE=${ARTIFACT_ROOT}/.molt_cache MOLT_DIFF_ROOT=${ARTIFACT_ROOT}/tmp/diff MOLT_DIFF_TMPDIR=${ARTIFACT_ROOT}/tmp MOLT_DIFF_TIMEOUT=180 MOLT_DIFF_GLOB='**/*.py' uv run --python 3.12 python -u tests/molt_diff.py --jobs 8 --live --log-file ${ARTIFACT_ROOT}/tmp/diff_live.log --log-aggregate ${ARTIFACT_ROOT}/tmp/diff_full.log --log-dir ${ARTIFACT_ROOT}/tmp/diff_logs tests/differential`
  - Example (monitor live log): `tail -f ${ARTIFACT_ROOT}/tmp/diff_live.log`
  - Example (monitor aggregate log): `tail -f ${ARTIFACT_ROOT}/tmp/diff_full.log`
  - Disable trusted default: `MOLT_DEV_TRUSTED=0 uv run --python 3.12 python -u tests/molt_diff.py tests/differential/basic`.
  - Optional speed workflow: prebuild runtime (`cargo build --release --package molt-runtime`), then do a two-pass diff run (no RSS first, RSS only for failures).
  - Always update [tests/differential/INDEX.md](tests/differential/INDEX.md) after diff runs:
    - Record the run date/time, host Python (`uv run --python 3.12/3.13/3.14`), totals, and failure list.
    - Use `<artifact-root>/rss_metrics.jsonl` to extract the latest per-test status when RSS is enabled.
    - Prefer re-running only failing tests (Failure Queue) unless a full sweep is explicitly requested.
- `tests/molt_diff.py` accepts multiple file/dir arguments and runs them in parallel by default (auto `--jobs`); use a shell loop only when you need custom ordering or retries.
- The `tests/differential/basic/bytes_codec.py` case requires `msgpack` + `cbor2` (install via `uv sync --group dev`); otherwise the diff harness will skip it.
- Use `tools/cpython_regrtest.py` to track CPython regression parity; it uses `tools/molt_regrtest_shim.py` to run tests via `--molt-cmd`. Keep skip reasons in `tools/cpython_regrtest_skip.txt`, and review `summary.md` (in each `logs/cpython_regrtest/<run>/`) + `junit.xml` in `logs/cpython_regrtest/`.
- `--coverage` now combines host regrtest + Molt subprocess coverage (requires `coverage` and a Python-based `--molt-cmd`; non-Python commands log a warning and skip Molt coverage).
- Regrtest runs set `MOLT_CAPABILITIES=fs.read,env.read` by default; override with `--molt-capabilities` if you need stricter or broader access.
- The regrtest shim marks `MOLT_COMPAT_ERROR` results as skipped; check `junit.xml` for reasons and codify intentional exclusions in `tools/cpython_regrtest_skip.txt`.
- The regrtest shim forces `MOLT_PROJECT_ROOT` to the repo so compiled runs link against the Molt runtime even for `third_party/` test sources.
- The regrtest shim sets `MOLT_MODULE_ROOTS` (and `MOLT_REGRTEST_CPYTHON_DIR`) to the CPython `Lib` directory so `test.*` resolves to CPython sources; avoid exporting that path via `PYTHONPATH` to the host Python.
- Use `molt test` for fast iteration, then use regrtest to surface broad regressions and map failures back to the stdlib matrix ([docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md](docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md)).
- Regrtest runs also emit `diff_summary.md` and `type_semantics_matrix.md` in each `logs/cpython_regrtest/<run>/` run directory to track type/semantics coverage gaps against `0014`/`0023`.
- Use `--no-diff` if you want regrtest-only runs (the diff suite is enabled by default).
- Use `--rust-coverage` with `cargo-llvm-cov` installed to collect Rust runtime coverage under `logs/cpython_regrtest/<ts>/py*/rust_coverage/`.
- Keep semantic tests deterministic; update or add differential cases when changing runtime or lowering behavior.
- For Rust changes that affect runtime semantics, add or update `cargo test` coverage.
- Avoid excessive lint/test loops while implementing; validate once after a cohesive structural arc is complete unless debugging a specific failure. Do not expand from a targeted proof into broad conformance/regrtest/benchmark lanes without a concrete claim or user request.
- If tests fail due to missing functionality, implement the correct behavior or
  preserve a clear fail-closed verified-subset gap; never change tests to hide
  the missing feature.
- **NEVER change Python semantics just to make a differential test pass.** This is a hard-stop rule; fix behavior to match CPython or document the genuine incompatibility in specs/tests.
- Parity-first workflow: execute the ROADMAP parity plan before large optimizations; require parity gates (matrix updates + differential coverage + native/WASM parity checks) for changes that touch runtime semantics.
- Treat benchmark regressions as failures; run `uv run --python 3.14 python tools/bench.py --json-out bench/results/bench.json`, `tools/dev.py lint`, and `tools/dev.py test` after the fix is in, then iterate on optimization until the regression is removed without introducing new regressions.
- After native + WASM benches, run `uv run --python 3.14 python tools/bench_report.py --update-status-doc` and commit the updated [docs/benchmarks/bench_summary.md](docs/benchmarks/bench_summary.md) plus the refreshed [docs/spec/STATUS.md](docs/spec/STATUS.md) benchmark block.
- Super bench runs (`tools/bench.py --super`, `tools/bench_wasm.py --super`) execute 10 samples and emit mean/median/variance/range stats; run only on explicit request or release tagging, and summarize the stats in [docs/spec/STATUS.md](docs/spec/STATUS.md) and [docs/benchmarks/bench_summary.md](docs/benchmarks/bench_summary.md).
- Sound the alarm immediately on performance regressions and trigger an optimization-first feedback loop after the structural implementation is complete; do not spin bench/lint/test cycles while the authority move is still mid-flight.
- Prefer performance wins even if they increase compile time or binary size; document tradeoffs explicitly.
- Always run tests via `uv run --python 3.12/3.13/3.14`; never use the raw `.venv` interpreter directly.
  - For CPython regrtest runs, prefer `--uv --uv-prepare --uv-python 3.12/3.13/3.14` so results are reproducible across versions.

## Compatibility & Security Claim Policy
- Passing differential + CPython regrtest is strong compatibility evidence for the covered CPython 3.12+ surface.
- Do not treat differential/regrtest pass status alone as a full security proof.
- Security confidence claims require explicit checklist evidence from [docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md](docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md) and the controls documented in [docs/SECURITY.md](docs/SECURITY.md).

## Commit & Pull Request Guidelines
- Repository history is active; use concise, imperative commit subjects with scope when helpful (e.g., `runtime: tighten object layout guards`).
- Prefer `area: summary` commit subjects and include a brief validation summary in the PR description for substantial changes.
- PRs should include a short summary, tests run, and any determinism or security impacts. Link issues when applicable.
- Release tags start at `v0.0.001` and increment at the thousandth place (e.g., `v0.0.002`, `v0.0.003`).

## Refactor Rule
- Refactors should delete debt and may change internals aggressively when the
  public verified-subset contract, tests, docs, and evidence move together.
  Do not split a structural refactor solely to preserve legacy semantics.

## Determinism & Reproducibility Notes
- Treat `uv.lock` and Rust lockfiles as part of the build contract; update them only when dependency changes are intentional.
- Avoid introducing nondeterminism in compiler output or tests unless explicitly gated behind a debug flag.
- `tools/cpython_regrtest.py --uv-prepare` runs `uv add --dev` (coverage/stdlib-list/etc.), so expect `uv.lock` changes when you opt in.

## Agent Expectations
- You are the finest compiler/runtime/Rust/Python engineer in the world; operate with rigor, speed, and ambition.
- Take a comprehensive micro+macro perspective: connect hot loops and object layouts to architectural goals in `docs/spec/` and [ROADMAP.md](ROADMAP.md).
- Be creative and visionary; proactively propose performance leaps while grounding them in specs and benchmarks.
- Provide extra handholding/step-by-step guidance when requested.
- Prefer production-first implementations over quick hacks. Do not land
  prototypes as product behavior.
- Do not land behavior stubs as substitutes for implementation. Prefer
  lower-level primitives first; unresolved gaps must fail closed and be routed
  through docs/roadmap without masquerading as support.
- Keep native and wasm feature sets in lockstep; treat wasm parity gaps as blockers and call them out immediately.
- ABSOLUTE RULE: Do not "fix" tests by weakening or contorting coverage to hide
  missing, partial, or hacky behavior; implement the correct behavior or record
  the verified-subset incompatibility explicitly.
- Proactively read and update [ROADMAP.md](ROADMAP.md) and relevant files under `docs/spec/` when behavior or scope changes.
- Treat [docs/spec/STATUS.md](docs/spec/STATUS.md) as a routing summary that
  must be refreshed from live code, executable tests, and generated evidence;
  update README only for newcomer-facing framing changes and update ROADMAP only
  for forward-plan changes.
- Proactively and aggressively plan for native support of popular and growing Python packages written in Rust, with a bias toward production-quality integrations.
- Treat the long-term vision as full Python compatibility: all types, syntax, and dependencies.
- Prioritize extending features; update existing implementations when needed to hit roadmap/spec goals, even if it requires refactors.
- For major changes, ensure tight integration and compatibility across compiler, runtime, tooling, and tests.
- NON-NEGOTIABLE: Do not introduce partial, hacky, stubbed, or workaround
  behavior. For genuinely missing functionality, fail closed and route the
  missing structural primitive through specs/status/roadmap without pretending
  the feature exists.
- Whenever a missing feature, verified-subset incompatibility, or optimization
  candidate changes the owned facts, update the relevant `docs/spec/` file(s),
  [docs/spec/STATUS.md](docs/spec/STATUS.md), and [ROADMAP.md](ROADMAP.md) in
  the same change.
- When major features or optimizations land, run benchmarks with JSON output (`python3 tools/bench.py --json`) and update the generated benchmark surfaces rather than embedding benchmark summaries in README.
- Follow [docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md](docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md) for stdlib scope, tiers (core vs import vs gated), and promotion rules.
- Promote stdlib modules aggressively once Rust-intrinsic/capability-gated
  semantics and target/version evidence exist; update the stdlib matrix and
  [ROADMAP.md](ROADMAP.md) with the promotion.
- Treat I/O, OS, network, and process modules as capability-gated and document the required permissions in specs.
## Fail-Closed Dynamism & Contract Conflicts (Non-Negotiable)
If adding functionality, tests, or coverage would require "too much dynamism"
that conflicts with the vision, break policy, runtime contract, or
concurrency/GIL requirements, choose the verified-subset path, fail closed with
clear diagnostics, and keep moving on compatible structural work. Ask for user
direction only when the requested outcome truly requires changing the project
contract.

Do not implement a feature by:
- Relaxing or bypassing constraints in [docs/spec/areas/core/0000-vision.md](docs/spec/areas/core/0000-vision.md) or [docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md](docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md) to accept CPython-style dynamism that the project explicitly rejects.
- Introducing dynamic execution/compilation paths (e.g., enabling arbitrary `eval`/`exec`/`compile`, runtime codegen from strings, or fallback to a host interpreter) that are not covered by the runtime contract/specs.
- Expanding dynamic import or reflection behavior beyond spec (e.g., import hooks, import-time monkeypatching, `__getattr__`-based module proxies, or dynamic module attribute creation) to make tests pass.
- Weakening determinism or capability gating (e.g., implicit host I/O, network/process access, time-dependent behavior, or environment-dependent resolution) outside the documented security/capability model.
- Changing runtime object layout/provenance/handle resolution rules or pointer registry behavior in ways that violate the runtime contract or provenance safety guarantees.
- Introducing concurrency or parallel execution that bypasses the GIL token,
  allows unsynchronized mutation, or otherwise violates the runtime locking
  model in `docs/spec/` and runtime safety docs unless all of the following are
  true:
  - The bypass is gated behind a spec-defined capability/flag that is **off by default**.
  - The gating mechanism, risk profile, and expected semantics are documented in `docs/spec/` and [docs/spec/STATUS.md](docs/spec/STATUS.md), and mirrored in [ROADMAP.md](ROADMAP.md).
  - The runtime safety plan is spelled out (e.g., provenance/aliasing guarantees, lock model changes, Miri or equivalent validation plan).
  - Tests explicitly cover both gated-on and gated-off behavior with determinism guarantees.
- Adding "dynamic escape hatches" (feature flags, hidden toggles, or environment variables) that effectively bypass the contract or policy without an explicit spec change.

When this triggers, do not implement a workaround. Document the conflict,
preserve the fail-closed behavior, and continue with structural work inside the
verified subset.

## TODO Taxonomy (Required)
TODOs are for external blockers, research leads, and explicit missing
structural primitives only. They are never a license to land partial behavior,
stubs, compatibility shims, or debt.

**Format**
- `TODO(area, owner:<team>, milestone:<tag>, priority:<P0-3>, status:<missing|partial|planned|divergent>): <action>`

**Required fields**
- `area`: short, stable domain (`type-coverage`, `stdlib-compat`, `stdlib-parity`, `frontend`, `compiler`, `runtime`, `opcode-matrix`, `semantics`, `syntax`, `async-runtime`, `introspection`, `import-system`, `runtime-provenance`, `tooling`, `perf`, `wasm-parity`, `wasm-db-parity`, `wasm-link`, `wasm-host`, `db`, `offload`, `http-runtime`, `observability`, `dataframe`, `tests`, `docs`, `security`, `packaging`, `c-api`).
- `owner`: `runtime`, `frontend`, `compiler`, `stdlib`, `tooling`, `release`, `docs`, `security`, or `tests`.
- `milestone`: `TC*`, `SL*`, `RT*`, `DB*`, `DF*`, `LF*`, `TL*`, `M*`, or another explicit tag defined in [ROADMAP.md](ROADMAP.md).
- `priority`: `P0` (blocker) to `P3` (low).
- `status`: `missing`, `partial`, `planned`, or `divergent`.

**Rules**
- Do not add incomplete, interim, partial, hacky, stubbed, stopgap, shim, or
  workaround behavior. Delete it, implement the final structural primitive, or
  fail closed.
- If a true missing structural primitive remains, include a TODO in-line only
  where it aids implementation and mirror the gap in
  [docs/spec/STATUS.md](docs/spec/STATUS.md) + [ROADMAP.md](ROADMAP.md).
- If you introduce a new `area` or `milestone`, add it to this list or the ROADMAP legend in the same change.

## Optimization Planning
- When focusing on optimization tasks, closely measure allocations and apply rigorous profiling when it can clarify behavior; this has unlocked major speedups in synchronous functions.
- When a potential optimization is discovered but is complex, risky, or time-intensive, add a fully specced entry to [OPTIMIZATIONS_PLAN.md](OPTIMIZATIONS_PLAN.md).
- The plan must include: problem statement, hypotheses, alternative implementations, algorithmic references/research (papers preferred), perf evaluation matrix (benchmarks + expected deltas), risk/rollback, and integration steps.
- Compare alternatives with explicit tradeoffs and include checklists for validation and regression prevention.

## Proactive Research Authorization
- Agents are explicitly authorized and strongly encouraged to proactively use subagents and current web research for recent arXiv papers, PL/compiler conference papers, production compiler/runtime talks, trade-conference presentations, and toolchain release notes when they may surface concrete optimization, correctness, verification, or DX opportunities for Molt.
- Research must be actionable: map each useful finding to Molt subsystems, files, experiments, tests, benchmark gates, or roadmap/spec updates. Do not accumulate passive reading notes.
- Prefer recent, primary, and technically specific sources. Validate source dates and provenance, and cite links in any user-facing research summary.
- Convert interesting and useful findings into implementation work aggressively, while preserving all non-negotiable project gates: tinygrad/DFlash fidelity, CPython 3.12+ supported-subset parity, no host-CPython fallback, no hacks/workarounds, native/WASM/LLVM/Luau parity, deterministic verification, and memory/OOM safeguards.
- If a finding implies a major architecture shift or conflict with current specs, write the closure plan first and update the relevant spec/roadmap documents in the same change before implementing.

## Multi-Agent Workflow
- [docs/ops/MULTI_AGENT_COORDINATION.md](docs/ops/MULTI_AGENT_COORDINATION.md)
  is the canonical protocol for parallel proof ownership, task logs, targeted
  vs broad validation, and respectful collision handling. Follow it before
  running differential, conformance, regrtest, benchmark, or `molt validate`
  lanes.
- This project is fundamentally low-level systems work blended with powerful higher-level abstractions; bring aspirational, genius-level rigor with gritty follow-through, seek the hardest problems first, own complexity end-to-end, and lean into building the future.
- Do not implement frontend-only workarounds or cheap hacks for runtime/compiler/backend semantics; fix the core layers so compiled binaries match CPython behavior.
- Agents may use `gh` (GitHub CLI) and git over SSH to open/merge PRs; commit frequently with clear messages.
- Run linting/testing once after a cohesive structural change set is complete (`tools/dev.py lint`, `tools/dev.py test`, plus relevant `cargo` checks when the claim requires them); avoid repetitive cycles mid-implementation.
- Prioritize clear, explicit communication: scope, files touched, and tests run.
- Prefer one broad-sweep coordinator per shared target root; other agents should
  run targeted proofs, reduce failure queues, or move non-colliding structural
  work instead of spamming full differential/conformance lanes.
- After any push, monitor CI logs until green; if failures appear, propose fixes, implement them, push again, and repeat until green.
- Avoid empty commit/push/CI loops: repeat only when there are new changes,
  changed external state, or a deliberate verification reason. Otherwise move to
  the next non-colliding structural task.

## Runtime Module Ownership (Planned Layout)
- `runtime/molt-runtime/src/state/*`: runtime
- `runtime/molt-runtime/src/concurrency/*`: runtime
- `runtime/molt-runtime/src/provenance/*`: runtime (perf focus)
- `runtime/molt-runtime/src/object/*`: runtime
- `runtime/molt-runtime/src/async_rt/*`: runtime (async-runtime focus)
- `runtime/molt-runtime/src/builtins/*`: runtime
- `runtime/molt-runtime/src/call/*`: runtime
- `runtime/molt-runtime/src/wasm/*`: runtime
