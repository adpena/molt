# Molt Century Systems Plan

Status: canonical long-horizon engineering plan. This document defines the
dependency spine, invariants, horizons, and exit protocol. It does not replace
the live board, executable ledgers, generated matrices, or detailed foundation
blueprints; it points to them. Current implementation and reproducible evidence
outrank prose when they disagree.

## 1. Mission and century invariant

Molt is to remain a small, deterministic, ahead-of-time Python system that
preserves expected CPython behavior by default inside an explicit verified
subset, refuses unsupported behavior early, and produces excellent native,
WASM, browser, edge, and future backend artifacts. Correctness, performance,
security, portability, operability, and usability are one product constraint.

The governing invariant is **one typed authority per semantic fact, ownership
edge, configuration choice, artifact identity, and support claim**. Every
frontend, IR, optimizer, backend, runtime, ABI, tool, test, benchmark, and
document is a projection of that authority. A narrow failing path is only the
aperture: work continues through the complete authority family, and the replaced
lane, shim, legacy path, and backward-compatibility burden are deleted.

The durable north-star contracts are:

- Default execution is deterministic and CPython-compatible; experimental
  free-threaded, gilless, GPU, distributed, and speculative modes are explicit.
- The verified subset grows monotonically. Outside it, Molt fails early and
  precisely; there is no host-Python fallback or Molt-owned reimplementation of
  third-party package semantics.
- Every claimed target x backend x profile x optimization-level cell has
  end-to-end semantic, performance, resource, artifact, and DX evidence.
- Performance includes latency, throughput, startup, code size, allocations,
  bytes moved, peak-live memory, process-tree RSS/commit, cache behavior,
  atomics, contention, energy when measurable, and failure atomicity.
- Every owned heap value has a mechanically checked lifetime. RC, cycle GC,
  weakrefs, finalizers, ABI ownership, publication, and concurrency share one
  generated lifetime/type authority.
- Artifacts are hermetic, content-addressed, reproducible, relocatable,
  attestable, and recoverable without institutional memory.
- No report, test, benchmark, or demo substitutes for the executable product
  acceptance contract it describes.

## 2. Authority and pointer map

Do not duplicate detail from these authorities in this plan:

| Concern | Current authority |
|---|---|
| Live lane ownership, current exit portfolio, proof custody | [`docs/agent/ORCHESTRATION.md`](../agent/ORCHESTRATION.md) |
| Current supported state and known gaps | [`docs/spec/STATUS.md`](../spec/STATUS.md) |
| Active delivery sequence | [`ROADMAP.md`](../../ROADMAP.md) |
| Structural doctrine and portfolio DAG | [`foundation/DESIGN_DOCTRINE.md`](foundation/DESIGN_DOCTRINE.md), [`foundation/PORTFOLIO_INDEX.md`](foundation/PORTFOLIO_INDEX.md) |
| Autonomous operating rules | [`foundation/52_autonomous_operating_charter.md`](foundation/52_autonomous_operating_charter.md) |
| Fact plane, decomposition, compression ladder | [`foundation/59_semantic_fact_plane.md`](foundation/59_semantic_fact_plane.md), [`foundation/21_decomposition_program.md`](foundation/21_decomposition_program.md), [`foundation/65_perf_compression_ladder.md`](foundation/65_perf_compression_ladder.md) |
| CPython parity and compatibility surfaces | [`foundation/66_compat_cpython_parity.md`](foundation/66_compat_cpython_parity.md), [`docs/spec/areas/compat/README.md`](../spec/areas/compat/README.md) |
| Lifetime, RC, GC, weakrefs, finalizers | [`foundation/55_memory_safety_ownership_lattice.md`](foundation/55_memory_safety_ownership_lattice.md), [`foundation/rc_gc_redesign.md`](foundation/rc_gc_redesign.md), designs 20, 27, 48-50 |
| Free-threading and concurrency | [`foundation/concurrency_architecture_100yr.md`](foundation/concurrency_architecture_100yr.md), [`foundation/33_threading-parallelism-ladder.md`](foundation/33_threading-parallelism-ladder.md), [`foundation/54_throughput_concurrency_async.md`](foundation/54_throughput_concurrency_async.md) |
| Performance evidence | [`foundation/64_perf_scoreboards_and_harness.md`](foundation/64_perf_scoreboards_and_harness.md), [`OPTIMIZATIONS_PLAN.md`](../../OPTIMIZATIONS_PLAN.md), [`docs/BENCHMARKING.md`](../BENCHMARKING.md) |
| Footprint and cold start | designs [`60`](foundation/60_tree_shaking_whole_program_dce.md), [`61`](foundation/61_binary_size_and_output_optimization.md), [`62`](foundation/62_startup_cold_start.md) |
| Import/module bedrock | [`foundation/69_import_bedrock_frozen_module_layer.md`](foundation/69_import_bedrock_frozen_module_layer.md) |
| DX, build graph, toolchains, distribution | [`foundation/dx_doctrine_100yr.md`](foundation/dx_doctrine_100yr.md), designs [`56`](foundation/56_dx_buildspeed_tooling.md), [`73`](foundation/73_efficient_builds_toolchain_provisioning_binary_cdn.md), [`74`](foundation/74_build_work_deduplication.md), [`75`](foundation/75_oss_build_speed_mining.md) |
| Security and formalization | [`docs/SECURITY.md`](../SECURITY.md), [`docs/spec/areas/security/`](../spec/areas/security/), [`docs/spec/areas/formal/FORMALIZATION_PLAN.md`](../spec/areas/formal/FORMALIZATION_PLAN.md), [`docs/spec/areas/formal/CERTIFICATION_STATUS.md`](../spec/areas/formal/CERTIFICATION_STATUS.md) |
| Pact reports, obligations, and acceptance | [`collab/pact/README.md`](../../collab/pact/README.md), [`docs/agent/PACT_CONTRACT_LEDGER.md`](../agent/PACT_CONTRACT_LEDGER.md), [`docs/PACT_SUPPORT_MATRIX.md`](../PACT_SUPPORT_MATRIX.md) |
| Long-running proof protocol | [`docs/agent/PROOF_QUEUE.md`](../agent/PROOF_QUEUE.md) |

## 3. Dependency spine

The century program is a dependency graph, not a calendar waterfall:

1. **Truth substrate:** generated semantic facts, stable identities, schemas,
   reproducible artifacts, differential oracles, and proof custody.
2. **Safety substrate:** ownership/lifetime proof, exact finalization, cycle GC,
   weakrefs, provenance, failure atomicity, and concurrency mode semantics.
3. **Semantic bedrock:** CPython parity surfaces, import/module state, call and
   exception authority, object protocol, C-API/ABI, and ecosystem custody.
4. **Compiler compression:** typed SSA/TIR facts, specialization, devirtualization,
   escape/borrow inference, representation selection, fusion, vectorization,
   tree shaking, and backend-neutral lowering.
5. **Product matrix:** native/WASM/browser/edge and every claimed backend,
   target, profile, optimization level, and Python version.
6. **Institution:** governed releases, durable artifacts, transparent evidence,
   operational recovery, ecosystem stewardship, and succession.

Later layers may develop concurrently only when they consume the earlier layer's
single authority and cannot weaken its invariant. A performance lane may not
invent semantics; a backend may not infer a fact the frontend/IR should carry;
an ecosystem lane may not clone an upstream package.

## 4. Workstreams and permanent design rules

### 4.1 Governance and decision architecture

- The live board assigns exclusive lanes; ledgers own obligations; generated
  matrices own support; decisions with long-lived semantic effect receive a
  design record and migration/deletion plan.
- Every arc pre-registers a done contract: invariant, affected authority family,
  falsifier, exact proof commands, resource budgets, matrix cells, artifacts,
  and code/docs scheduled for deletion.
- Ratchets are monotone and machine checked. Baselines may change only with
  evidence, named ownership, and an explicit governance decision.
- Findings inside the owned class are work, not reports. External blockers are
  recorded with evidence while another valid dependency-ordered arc advances.

### 4.2 Correctness and formal methods

- CPython is the observable semantic oracle for the declared version range;
  the language surface, C-API/ABI, stdlib, import system, exceptions, finalizers,
  weakrefs, and concurrency semantics have generated coverage matrices.
- Facts cross every serialization and lowering boundary with producer,
  round-trip, consumer, and translation-validation proof.
- Use differential/property/metamorphic testing, fuzzing, model checking, Miri,
  sanitizers, race exploration, symbolic execution, and proof assistants where
  each has leverage. Formalize small load-bearing kernels before broad prose.
- Proof assumptions, solver/tool versions, seeds, counterexamples, certificates,
  and replay commands are content-addressed release artifacts.

### 4.3 Lifetime, RC, GC, weakrefs, and free-threading

- Generate one per-heap-kind descriptor for layout, child traversal, clear,
  finalizer/weakref capability, ABI projection, allocation class, and collector
  eligibility. Delete hand-maintained sibling switches.
- Compile-time ownership and borrow/escape facts remove most RC operations.
  Remaining GIL-default RC is non-atomic and inline where proven; free-threaded
  mode selects an explicitly modeled atomic/biased/deferred strategy without
  taxing the default fast path.
- Deallocation is iterative and bounded. Cycle collection follows CPython-visible
  semantics with generated traverse/clear, generations, thresholds, weakref
  ordering, PEP 442 finalization, resurrection handling, and deterministic order.
- A lifetime state machine, not scattered flags, owns strong/weak/finalized/
  resurrected/deallocated transitions. Unsafe code has explicit provenance and
  concurrency invariants; Loom/Miri/Kani-style proofs cover transition races.
- Object-header auxiliary representation is chosen before publication and its
  kind/address never moves afterward. Constructors preselect every class,
  state, poll, and sidecar lane they can require, initialize all owned edges,
  and release-publish exactly once. Published mutation changes an edge through
  that stable lane and never performs a torn representation upgrade or relies
  on a caller-specific repair.
- The legal refcount transition algebra is a small, pure, formally checked
  kernel consumed unchanged by native atomic, wasm single-thread, biased,
  deferred, GC-pin, weak-upgrade, and finalizer-revival storage strategies.
  Storage adapters choose representation and ordering; they may not duplicate
  or weaken zero, immortal, overflow, or committed-death rules.
- Free-threaded/gilless support is designed in from every touched ownership,
  container, cache, import, and ABI boundary, while CPython-compatible GIL mode
  remains deterministic by default.

### 4.4 Compiler, runtime, ABI, and backends

- The semantic fact plane is the compiler's nervous system. Frontends produce
  facts; IR transports them; optimizers transform them with proofs; backends
  lower them; runtimes implement only irreducible dynamic operations.
- Generated registries own op effects, exceptions, ownership, callability,
  layout, ABI signatures, target capabilities, and reachability. Backends do
  not carry mirrored policy tables.
- Decompose by stable architectural boundaries so changing one stdlib family,
  pass family, or backend stage recompiles only that unit. No god-files or
  cyclic ownership.
- Native, LLVM, WASM, Luau, MLIR, WebGPU, and future backends share semantics
  and differ only in explicit capability/cost/lowering policy. Unsupported
  lowering fails before artifact production.
- Import/module state and extension initialization use one transaction and one
  identity authority across static, dynamic, split-runtime, and embed surfaces.

### 4.5 Verification, benchmarks, performance, and resource engineering

- One canonical `PerfCell`-class evidence stream feeds separate projections for
  runtime, compile/build throughput, footprint, cold start, memory/allocations,
  and concurrency/energy; no dashboard is a second measurement authority.
- Profile before and after structural changes using wall and CPU time, sampled
  and instrumented profiles, instruction/branch/cache counters, allocator and
  lifetime traces, codegen/pass deltas, binary maps, process-tree memory, and
  contention/atomic telemetry.
- Benchmarks cover micro mechanisms, real applications, friend-owned suites,
  adversarial cases, long-lived services, and cold clean-room builds. Every
  result pins source, toolchain, environment, hardware, profile, inputs, and
  statistical method.
- Expert review follows source through AST, typed IR, optimized IR, machine IR,
  object, link map, and final disassembly. It inspects missed optimization and
  vectorization remarks, alias/provenance limits, inlining, register pressure,
  spills, instruction mix, branch prediction, I/D-cache and TLB behavior, page
  faults, syscalls, context switches, NUMA/false sharing, allocator fragmentation,
  tail latency, and benchmark observer effects. PGO, post-link optimization,
  LTO, layout, and hardware-specific dispatch remain reproducible projections
  of shared semantics, never handwritten semantic forks.
- Numerical work records precision, rounding, reduction order, NaN/Inf/signed
  zero behavior, conditioning, and fast-math assumptions. Deterministic
  authority paths never inherit a speed path's relaxed arithmetic implicitly.
- Optimize the whole cost model. A throughput win that regresses determinism,
  memory, startup, size, build time, tail latency, or maintainability remains a
  tradeoff requiring an explicit product decision, never a silent win.

### 4.6 Native/WASM/target/backend/profile matrix

- A generated matrix enumerates Python version x host/target x backend x
  runtime topology x profile x optimization level x concurrency mode x
  capability tier. Claims are made per cell; no cell inherits another's proof.
- Required cells include native and WASM authority lanes, browser and headless
  hosts, split and linked runtimes, debug/dev/release/size profiles, deterministic
  and explicitly nondeterministic modes, and every publicly documented target.
- Each claimed cell proves build, cache correctness, artifact audit, relocation,
  execution, parity, resource bounds, diagnostics, packaging, and upgrade path.
- WebGPU/WebNN/GPU paths are performance projections over a deterministic CPU
  authority unless a stronger cross-device contract is explicitly proven.

### 4.7 DX and UX

- `molt run/build/test/doctor/profile/audit` form one progressive surface with
  discoverable defaults, typed configuration, stable machine-readable output,
  actionable diagnostics, and copy-paste remediation.
- Toolchains, sysroots, package prerequisites, caches, and release artifacts are
  provisioned by shared custody. No manual environment folklore is a supported
  workflow.
- The same task has the same concepts, names, diagnostics, and artifact layout
  on Windows, macOS, Linux, native, WASM, browser, CI, and offline installations.
- Measure first-success time, incremental edit-build-run latency, diagnostic
  precision, cache hit rate, recovery time, artifact discoverability, and
  accessibility. Dogfood real downstream projects, not only examples.

### 4.8 Security, supply chain, and reproducibility

- Capabilities are deny-by-default and least-privilege. Compilation, build
  scripts, package custody, runtime host calls, plugins, and downloaded tools
  cross explicit trust boundaries with auditable policy.
- Dependencies, compilers, linkers, sysroots, generated sources, and native
  objects are pinned, checksummed, provenance-attested, license-audited, and
  represented in an SBOM. Builds are hermetic and reproducible across clean
  machines or fail with a precise non-reproducibility dossier.
- Signing, key rotation, revocation, vulnerability response, reproducible
  rebuild, and compromised-builder recovery are tested operations.
- Fuzz/security corpora and disclosure records remain replayable across schema
  evolution; sensitive operational data never enters public proof artifacts.

### 4.9 Ecosystem custody

- Third-party behavior comes from pinned upstream source and its build system.
  Molt owns reusable compiler/runtime/ABI/package-custody primitives, never
  package-specific semantic clones or compatibility overlays.
- Source, generated files, headers, compile commands, native objects, Python
  modules, metadata, licenses, and transitive tools form one checksummed package
  closure. Provider collisions are deterministic and fail closed.
- Ecosystem support is a generated matrix backed by unchanged upstream test and
  benchmark suites, including NumPy/SciPy/Pact and representative web, data,
  ML, GUI, async, and packaging workloads.

### 4.10 Deprecation, deletion, and complexity control

- Molt internals provide no backward-compatibility sanctuary. When authority
  moves, migrate every consumer and delete the old path in the same structural
  arc. Public transitions require a time-bounded, measured migration contract.
- Every release inventories legacy flags, aliases, shims, fallback paths,
  duplicate registries, dead feature gates, stale docs, and quarantined research.
  Each is deleted, assigned a removal release, or rejected with recorded reason.
- Complexity budgets track dependency edges, rebuild fanout, unsafe surface,
  duplicate authority count, generated/manual fact ratio, and cognitive surface,
  not raw line count. Simplicity means fewer states and authorities, not fewer
  capabilities or weaker proof.

### 4.11 Operations, recovery, and artifact longevity

- Builds/tests/benchmarks run under explicit process custody, time/memory/disk
  bounds, structured logs, crash-safe publication, and resumable proof records.
- Release artifacts include source, schemas, manifests, toolchains or recipes,
  SBOM/provenance, test vectors, proof certificates, benchmark evidence,
  migration tools, and human-readable recovery instructions.
- Formats are versioned, self-describing, checksummed, and equipped with
  deterministic migrators and independent readers. Periodic clean-room and
  offline restorations prove that no service, account, or person's memory is a
  hidden dependency.
- Disaster exercises cover lost caches, lost registries, expired credentials,
  unavailable upstreams, compromised builders, corrupted artifacts, and project
  leadership turnover.

### 4.12 Succession and institutional continuity

- Architectural authority is discoverable from repository indexes and generated
  ownership maps. Critical decisions include alternatives, evidence, invariants,
  and reversal conditions.
- Every load-bearing subsystem has at least two maintainers or a documented
  apprenticeship/recovery path. Releases cannot depend on one private machine,
  account, key, or tacit procedure.
- New maintainers prove competence by reproducing a release, repairing a seeded
  fault, interpreting a failed matrix cell, and completing a structural rip.
- Every five years, re-derive the plan from mission and evidence. Technologies,
  languages, vendors, and backends are replaceable; semantic and evidence
  contracts are the enduring asset.

## 5. Horizons and machine-checkable phase exits

Dates are review horizons, not permission to postpone prerequisites. A phase is
complete only when its exit manifest is committed and the named authorities
report green for the exact release commit. Each phase manifest is a generated,
schema-validated record containing `phase`, `commit`, `matrix_digest`,
`evidence[]`, `open_obligations`, `legacy_count`, and `signed_attestation`.
Every evidence row carries `requirement_id`, `authority`, `command`,
`artifact_sha256`, `matrix_cells`, `status`, `toolchain_digest`, and
`observed_at`. The validator's phase predicate is fixed:

```text
phase_green := schema_valid
  and manifest.commit == release_commit
  and manifest.matrix_digest == generated_matrix.digest
  and every(required_requirement has exactly one current passing evidence row)
  and every(required_matrix_cell is covered by that row)
  and open_obligations == []
  and legacy_count == 0
  and signatures_and_hashes_verify
```

Missing, stale, duplicate, waived, unevaluated, or indirectly inferred evidence
evaluates false. A future validator may change implementation language, but not
this predicate without a reviewed governance change and migration proof.

### H0 - Recover truth and close current P0s (now to 1 year)

Deliver the current live-board exit portfolio: Pact Kernel A acceptance;
ownership/finalizer/cycle correctness; single semantic/parity/perf authorities;
hermetic package/toolchain seals; honest current-state and support matrices.

Exit `H0` iff all are machine true:

- `docs/agent/PACT_CONTRACT_LEDGER.md` has no open P0 and the named Pact
  acceptance lane produced `candidate_outputs.npz` accepted by the canonical
  parity engine for the exact commit.
- Memory-safety, lifetime, weakref/finalizer, import-bedrock, C-API/ABI, and
  differential parity gates are green on required native/WASM cells.
- Structural/fail-closed audits report no newly introduced duplicate authority,
  compatibility crutch, package clone, or unowned legacy lane.
- The canonical matrix, perf stream, package seal, and proof queue emit
  schema-valid, content-addressed evidence with zero missing required fields.

### H1 - Trustworthy product system (1 to 5 years)

Complete generated parity surfaces, precise lifetime/collector semantics,
backend-neutral facts, hermetic ecosystem custody, decomposed incremental build
graph, and a coherent cross-platform CLI/artifact experience.

Exit `H1` iff:

- All documented supported CPython >=3.12 surface rows and all claimed product
  matrix cells are green or explicitly absent from the product claim.
- The lifetime verifier proves consume/release on every path; Miri/sanitizer/
  race/model checks and leak/cycle/finalizer corpora are green; no manual
  per-type traversal/deallocation authority remains.
- Clean-room builds from pinned inputs are reproducible and relocatable on every
  Tier-1 host; SBOM, provenance, signatures, and recovery drills verify.
- Performance boards contain no unexplained CPython-red core lane and publish
  compile, runtime, allocation/memory, startup, and size evidence.
- Legacy/internal compatibility inventory is zero except time-bounded public
  transitions listed in a generated removal manifest.

### H2 - Compression-ladder maturity (5 to 10 years)

Make typed semantic facts, devirtualization, shapes, borrow inference,
specialized representations, fusion, vectorization, and whole-program
reachability routinely outperform interpreter and AOT peers without semantic
forks or product-matrix cliffs.

Exit `H2` iff:

- Each compression-ladder rung has a generated fact authority, translation
  validation, adversarial corpus, and causal pass-delta evidence.
- Core and friend-owned suites meet declared noise-aware performance floors in
  every claimed cell, with no dimension hidden by aggregate scores.
- Release artifacts meet published cold-start, footprint, memory, and build-time
  budgets, and unsupported cells refuse before expensive work.
- The same source and parity oracle drive native, WASM, browser/GPU, and other
  production backends; backend-local semantic policy count is zero.

### H3 - Free-threaded and heterogeneous system (10 to 25 years)

Promote free-threaded/gilless execution, isolated interpreters, structured
concurrency, heterogeneous CPU/GPU execution, and distributed compilation from
experimental modes only after determinism, safety, and cost models are proven.

Exit `H3` iff:

- Race, memory-model, lifetime, weakref/finalizer, extension-ABI, and container
  linearizability suites pass under default GIL and claimed free-threaded modes.
- Default-mode output and ordering remain CPython-compatible; nondeterministic
  scheduling/fast math are explicit and evidence-labelled.
- Atomic/contention/cache-line telemetry meets budgets, and escape/ownership
  proofs eliminate shared-state taxes from thread-local and single-thread code.
- CPU/GPU partitioning and fallback are capability-derived, parity-gated,
  failure-atomic, and reproducible per declared device class.

### H4 - Self-hosting, proof-carrying evolution (25 to 50 years)

Molt can rebuild, validate, optimize, and migrate its own compiler/runtime and
artifact estate from durable specifications and evidence, with interpretable
automation proposing changes under immutable gates.

Exit `H4` iff:

- A clean-room environment reconstructs every supported release from archived
  source/spec/toolchain inputs and independently verifies signatures/proofs.
- Load-bearing IR, ownership, ABI, artifact, and concurrency transformations
  emit checkable certificates or translation-validation witnesses.
- Autonomous changes cannot modify their acceptance or evidence authority in
  the same trust domain; seeded reward-hacking and supply-chain attacks are
  detected in scheduled exercises.
- Two independent implementations can read/migrate core artifact and proof
  formats without project-private services.

### H5 - Century institution (50 to 100 years)

Preserve Python program meaning and Molt artifacts across multiple generations
of hardware, operating systems, maintainers, organizational forms, and
implementation languages.

Exit `H5` iff:

- Decade-spaced release samples reproduce and execute through documented
  emulation/migration paths with verified semantic and artifact identity.
- No critical authority depends on an obsolete vendor, single cryptographic
  primitive, single implementation language, single maintainer, or unavailable
  service; migration drills demonstrate alternatives.
- Governance, threat model, succession map, compatibility policy, proof roots,
  and century plan have passed two independent external audits in the preceding
  review cycle.
- A new team with no oral handoff can restore a release, explain its invariants,
  diagnose a seeded cross-matrix regression, and ship a verified successor.

## 6. Rolling execution and review cadence

The plan is continuously re-derived; it is never a frozen prediction.

| Cadence | Required review and durable output |
|---|---|
| Every arc | Read live board; pre-register done contract; inspect whole authority family; run bounded contract proof; delete replaced lanes; update owning ledger/evidence. |
| Every merge | Re-read board; drift/trample audit; generated-artifact checks; exact pathspec; commit-linked evidence and performance/resource impact. |
| Weekly | Triage red matrix cells, regressions, security findings, stale proofs, legacy inventory, Pact correspondence, and unowned obligations. |
| Monthly | Re-rank dependency graph by class-kill and measured cost; audit allocations/memory/cache/atomics/build fanout; run one disaster or clean-room micro-drill. |
| Quarterly | Full claimed matrix sampling; ecosystem crater/friend suites; formal/security/supply-chain review; deletion release; DX journey study. |
| Every release | Reproducible clean-room build; parity/perf/resource/security gates; SBOM/provenance/signing; migration/rollback/recovery proof; archived evidence bundle. |
| Annually | Re-baseline hardware/toolchains without lowering semantic gates; external design/perf/safety review; bus-factor and succession exercise. |
| Every 5 years | Re-derive horizons, replace obsolete mechanisms, rehearse offline restoration, and publish a compatibility/artifact-longevity report. |
| Every 10 years | Independent clean-room implementation/readability audit and cryptographic/schema migration rehearsal. |

## 7. Pact is the current end-to-end keystone

Pact reports 001 through the latest memo are one additive correspondence stream,
indexed by [`collab/pact/README.md`](../../collab/pact/README.md) and normalized
into [`docs/agent/PACT_CONTRACT_LEDGER.md`](../agent/PACT_CONTRACT_LEDGER.md).
The ledger, not a stale narrative status paragraph, owns obligation state.

The current shared P0 remains Kernel A: build and execute the real
`collab/pact/pact_witness_kernel/field_solve.py` through Molt WASM/browser,
produce `candidate_outputs.npz`, and pass the canonical parity engine under the
declared gates. Kernel B, the shared parity-harness intake, WebGPU/WebNN demo,
seven-kernel suite, headless/browser support matrix, contest-runtime axes, and
long-horizon training/deployment work remain ordered obligations in the ledger.
No package clone, host fallback, synthetic forward-only smoke, or one-cell
backend proof can satisfy this acceptance contract.

## 8. Completion law

This plan has no prose-only completion path. Before closing any horizon or the
century goal, derive every explicit obligation from this plan, the live board,
current status, Pact ledger, generated support matrices, formal certification,
security policy, release checklist, and proof queue; bind each to current-state
evidence; classify missing, weak, contradictory, stale, or green evidence; and
continue until every required item is green. A blocked proof, model failure,
compaction, commit, report, or turn boundary is not permission to stop.
