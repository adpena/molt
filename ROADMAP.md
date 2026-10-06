# Molt Roadmap (Active)

For current supported state, use [docs/spec/STATUS.md](docs/spec/STATUS.md).
This file is forward-looking only. Live code, executable tests, generated
matrices, and replayable receipts remain authoritative when prose drifts. The
[canonical reading list](docs/CANONICALS.md) routes detailed engineering plans;
this roadmap does not duplicate their implementation history.

## First Release Milestone

The highest priority is a stable `v1.0` contract and release, with an
evidence-backed `v0.0.1` as the initial milestone. Release-blocking correctness
and integration work takes precedence over broader feature expansion.
The exact version and tag follow the existing
[packaging authority](packaging/PACKAGING.md), not a separate roadmap version.
Consolidate shared compiler/runtime semantics and close known correctness
failures in the advertised subset before promoting release artifacts.

Use the existing [packaging acceptance contract](packaging/PACKAGING.md) and
[support indexes](docs/spec/areas/compat/README.md) as the release authorities:
publish replayable receipts for the exact native/WASM, Python-version,
OS/architecture, backend, and profile cells claimed by the release.
Installation, standalone execution, deterministic semantics, and measured
performance must be proved for that scope; unverified ecosystem packages or
matrix cells remain explicitly unclaimed. This milestone does not require
solving the entire long-term roadmap or imply that release acceptance has
passed.

Before publication, review and execute the complete advertised subset end to
end, including its public API, C-API/ABI, ownership, and error paths on both
native and WASM. Differential and adversarial tests must cover deterministic
observable behavior against the supported CPython versions. An archive,
unit-test shard, symbol count, or a pass on one target is not a substitute for
those receipts.

For v1.0, close measured performance and resource-budget violations across the
advertised workloads, including frontend/build scaling and native/WASM runtime
costs. Missing measurements are gaps, not passes. The
[performance authority](tools/PERF_AUTHORITY.md#v10-acceptance-scope) owns this
acceptance scope; do not infer it from an aggregate speedup or a smaller test.

## Strategic Target

- Reach full CPython `>=3.12` parity for the supported Molt subset.
- Ship standalone binaries. Compiled programs prohibit host-CPython fallback,
  including as a future compatibility path.
- Outperform CPython on the benchmark suites Molt claims as core product lanes.
- Treat tiny-program cold start and output binary size as product-critical
  axes across native, browser/WASM, Luau, and MLIR, with release artifacts
  ratcheting toward <50 ms cold start and <2 MB gzipped/runtime payloads on the
  five-year arc.
- Preserve the
  [dynamic-semantics boundary](docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md)
  around arbitrary runtime monkeypatching, unrestricted dynamic execution, and
  unrestricted reflection while expanding explicitly verified contracts.

## Current Priorities

### Qualification and blocker burndown

The current work is development integration, not a qualified release candidate.
Use the existing local coordination record for the active source identity,
integrator, owned changes, execution receipts, blocker and next complete outcome.
Reconcile worker handoffs against live source before carrying a blocker forward:
an old unintegrated proposal can have been superseded by an integrated solution.
Keep machine-specific evidence and execution history out of this roadmap.

Close release outcomes in dependency order:

| Outcome | Current barrier | Closure authority |
| --- | --- | --- |
| Stable integrated candidate | Consolidated compiler/runtime authorities need one immutable candidate with source-bound validation; unresolved semantic failures still block qualification. HEAD alone does not identify a dirty candidate. | Existing source snapshots, integration ownership and observed compiler/runtime identities |
| Useful standalone programs | Exception/class/frame boundaries and asyncio still need complete native/WASM execution coverage in the advertised subset. | Public CLI consumers and the verified-subset contract |
| Installed compile/edit/run workflow | Runtime-cell delivery, installed dependencies, LLVM availability and build identity binding must close without requiring users to build Molt. | [Packaging](packaging/PACKAGING.md) |
| Product performance and resource limits | Required runtime, compile latency, startup, size and memory cells must be measured; async idle CPU, wakeup latency and throughput remain unqualified. | [Performance authority](tools/PERF_AUTHORITY.md) |
| Qualified and publishable v1.0 | Current-candidate E1-E4 and authenticated H0, full installation/compatibility/reproducibility/supply-chain evidence, then explicit publication authorization. | [Release acceptance](packaging/PACKAGING.md) and `tools/release_exit_gate.py` |

For each active blocker, keep one owner, the earliest failing consumer, the
affected contract and the next result that would close it. Distinguish
implemented, representative execution passed, full acceptance passed and
published. Review completion, narrow checks, absent measurements and skipped
cells never advance a later state. Asyncio is a P0 product outcome; integrated
park/wake code is not closure of its lifecycle, target or performance contract.
Constructor receiver gaps, including repeat subclass preservation and foreign
class rejection, remain tracked in the
[call binding contract](docs/spec/areas/compat/contracts/call_argument_binding_contract.md).

Run one coordinated acceptance campaign after candidate stabilization. Invalidate
affected evidence when inputs change, retain valid independent results, and
diagnose the failed stage before repeating expensive work. An operator-only
blocker retains its exact resume state while independent authorized work proceeds.

### Product integration order

Correctness and performance are both release priorities. Work follows complete
user outcomes and their measured blockers; this sequence is not a schedule or
permission to defer a required target, profile, or acceptance criterion.
Every release's critical path includes the native/WASM NumPy/SciPy witness
(E1), the native/LLVM performance gate (E2), and executable all-pass
verified-subset coverage (E3). Windows networking is required by the declared
Windows-native tests. These are load-bearing product work, not tasks to defer
until packaging. Stable v1.0 additionally requires authenticated H0.

The native scientific witness needs newly admitted NumPy/SciPy extension seals
under the current source-extension contract. Preserved schema-4 native seals
are historical evidence; schema-7 admission additionally binds consumed Python
providers and Meson dependency facts. Their historical digests cannot supply
current registry entries or acceptance receipts. Reproduce the upstream
extension builds through the canonical candidate producer and admission path;
the upstream libraries remain the package source authority.

1. Complete the install, compile, and execute workflow. Released platform wheels,
   package-manager distributions, and GitHub bundles must provide the compiler
   and required runtime artifacts. Compiling Molt is a source-development task.
   Prove public CLI compilation and standalone native and linked-WASM execution
   of representative supported programs, then reuse compatible builds across
   their semantic and target-version boundaries. The
   [installation and release authority](packaging/PACKAGING.md) owns the exact
   shipped cells and installed-consumer checks. Keep compiler inputs immutable
   and user-project dependency environments and outputs under their own roots;
   public package commands must obey the same boundary. Bind dependency
   resolution to the selected version and target markers, and use the same
   admitted package roots on every host. Preserve complete bundle projections
   and make frontend-interpreter requirements actionable for installed users.
2. Finish shared frame, callable, exception, and async semantics. Runtime-owned
   bindings, expression captures, traceback retention, locals proxies,
   suspension/resumption, cancellation, and finalizer reentry must agree with
   the selected CPython version. Preserve complete metadata admission before
   cached frame-plan construction and runtime ownership of terminal binding
   retirement across every callable form. Close ownership through `DropInsertion`,
   `ExceptionRegions`, and `HandlerState` across their consumers and delete
   replaced release paths only when the shared contract owns the whole boundary.
   Detailed plans live in
   [RC ownership and drop insertion](docs/design/foundation/20_rc-ownership-drop-insertion.md)
   and [exception-region ownership](docs/design/foundation/45_exception_region_ownership.md).
3. Make every optimization preserve the source program. Fused loops must retain
   observable bindings, sequential floating-point behavior, arbitrary-precision
   integer results, and callback, destructor, exception, and operand order.
   Admission and specialization use shared typed value, range, representation,
   effect, and ownership facts. Prove physical container storage independently
   of semantic element types, and invalidate it across mutation, aliases and
   escape. Inline storage must preserve heap owners and canonical forward and
   reflected operator dispatch. Builtin subclasses must inherit admitted native
   payload layout, initialization, class slots and lifetime policy together;
   ancestry alone cannot authorize container operations on generic objects.
   One solid-layout lineage must govern base admission and public `__base__`.
   Seal physical fields, declared-slot identities and intrinsic payloads in one
   immutable class projection consumed by attribute access, allocation, GC,
   state extraction and class transfer. Legal neutral mixins must not overlap
   native payloads; redeclared slots keep their declaring owners.
   Preserve subclass overrides for source operations and direct builtin-slot
   semantics for explicit descriptors. Eliminate benchmark-specific semantic paths and
   string/name guesses in place of canonical identities. Continue retiring
   `fast_int`, `fast_float`, string `type_hint`, raw-scalar shadow, and
   backend-reconstructed container authorities. The portfolio route is
   [the semantic fact plane](docs/design/foundation/59_semantic_fact_plane.md).
4. Deliver measured improvements to compilation and generated programs. Separate
   cold, unchanged warm, and single-module-edit compilation; attribute discovery,
   analysis, lowering, code generation, runtime admission, and linking costs.
   Reuse content-bound artifacts atomically while preserving live module
   resolution and input-change detection. Profile real hot code and allocation/RC
   costs; measure fresh-process startup, executable/module and compressed
   deployment bytes, peak memory, and scaling. A shared reachability/runtime
   surface must drive native link roots, WASM imports/exports, and intrinsic
   resolution. The [performance authority](tools/PERF_AUTHORITY.md) owns claims;
   engineering detail lives in the
   [binary-size](docs/design/foundation/61_binary_size_and_output_optimization.md),
   [cold-start](docs/design/foundation/62_startup_cold_start.md), and
   [DX/build-speed](docs/design/foundation/56_dx_buildspeed_tooling.md) plans.
5. Close the remaining declared compatibility surface without hidden host
   fallback. Continue language, stdlib, import transactions, source-extension,
   C-API, NumPy, and SciPy work through the generated
   [compatibility indexes](docs/spec/areas/compat/README.md) and the
   [libmolt extension ABI contract](docs/spec/areas/compat/contracts/libmolt_extension_abi_contract.md).
   Pinned upstream packages remain upstream-owned; Molt owns generic custody,
   ABI, and integration.
6. Advance other target and ecosystem work through its existing support
   contracts. LLVM, Luau, Rust-source, browser, and GPU claims require their own
   admitted and executed cells; native execution alone proves none of them.
   Keep Luau support in its
   [generated matrix](docs/spec/areas/compiler/luau_support_matrix.generated.md).
   GPU gaps include MIL BF16/64-bit/MXFP proof, MLIR MXFP block/exponent storage
   and materialization, quantized casts, a first-class window/im2col primitive,
   and typed nonzero padding. Route these through the
   [GPU primitive architecture](docs/architecture/gpu-primitive-stack.md),
   [GPU/MLIR plan](docs/spec/areas/perf/0513_GPU_PARALLELISM_AND_MLIR.md), and
   [tinygrad/DFlash contract](docs/design/foundation/67_compat_tinygrad_dflash.md).
7. Qualify an integrated candidate at the real consumer. Bind each result to the
   actual source, compiler binary/features, selected runtime cell/generation,
   toolchain, inputs, target, and profile. Carry observed build diagnostics
   through semantic receipts into candidate admission. Build reproducibility,
   controlled-input semantic determinism, and concurrent execution guarantees
   are distinct claims. Complete the declared installation and compatibility
   matrix, E1-E4, authenticated H0, reproducibility, and supply-chain gates as one
   coordinated campaign after representative workflows converge. Use the
   [minimum must-pass matrix](docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md)
   and [release authority](packaging/PACKAGING.md). Missing, skipped, stale, or
   interrupted cells remain open; demonstrations are not release acceptance.
   Keep execution custody in the existing [operations authority](docs/OPERATIONS.md).
   Repair the existing acceptance paths before relying on them: profile and
   stderr coverage in verified-subset execution, actual compiler identities in
   performance receipts, and complete runtime-inventory inputs in packaging.
   Preserve every required cell and expected-failure obligation during repair.

Update the owning support contract and user documentation with each substantive
integration. Keep implementation, representative validation, release acceptance,
and publication distinct. Current support belongs in
[STATUS.md](docs/spec/STATUS.md) and its generated indexes; this roadmap owns
forward sequencing. Per-run logs and working handoffs are local evidence.

## Milestone Sequence

### Near Term

- Finish consolidation and land the current authority cohort before opening new
  compiler-frontier lanes.
- Complete shared ownership/exception/finalizer closure and delete the obsolete
  sibling lanes it replaces.
- Close fused-loop equivalence and exact binding/type admission before relying
  on their performance results.
- Complete installed-user workflows and correctness-checked measurements of
  compile latency, hot generated code, allocations, startup, and artifact size.
- Produce serial native and linked-WASM guest receipts for the exact supported
  Python/profile cells, then close the first-release matrix without widening its
  scope.
- Reconcile Luau prose and implementation work against the generated admission
  matrix; helper presence remains internal until target admission is real.
- Keep no-host source-extension and pinned NumPy/SciPy import/runtime work
  fail-closed while the generic artifact and ABI custody becomes complete.

### Medium Term

- Broaden language and stdlib coverage under the shared typed-fact model.
- Complete importlib transaction and namespace-package semantics without a
  second resolver or execution authority.
- Finish the listed GPU storage/materialization primitives and move the pinned
  tinygrad lane through real end-to-end workloads.
- Make per-program runtime reachability control native/WASM output surface,
  startup, and size.
- Keep performance and compatibility reporting generated from durable receipts
  rather than manually synchronized status prose.

### Long Term

- Broaden portable extension support through `libmolt`.
- Converge on a larger practical CPython `>=3.12` surface without weakening
  determinism, packaging, or no-host guarantees.
- Make cold-start, binary-size, throughput, and cross-target parity gates
  equally central across native, WASM browser/Node/Cloudflare, LLVM/MLIR, Luau,
  and future output surfaces.
- Follow the long-horizon portfolio through
  [docs/CANONICALS.md](docs/CANONICALS.md) instead of copying its plans here.

## Active Blockers

The [type coverage matrix](docs/spec/areas/compat/surfaces/language/type_coverage_matrix.md)
and [stdlib surface matrix](docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md)
own the exact compatibility gap records. Roadmap summaries link to those records;
any duplicated TODO must match its complete canonical record. In particular,
memoryview character stores after a key releases the view still reject safely,
even when another export keeps the original storage alive. Exact parity needs
shared storage-liveness observation that does not block release/resize callbacks
or defer native exporter release; the type matrix tracks that remaining gap.

- The release workflow does not yet produce and pass both runtime-cell
  inventories required by candidate assembly. The packaging contract remains
  mandatory; source implementation of an assembler is not a successful release.
- Verified-subset policy declares both `dev` and `release` guest profiles and
  execution binds the requested profile to its coordinate. Complete passing
  coverage remains unqualified. The default stderr comparison and expected-failure
  policies still need reconciliation with the all-pass release law without
  excluding tests or weakening observable exception, warning, or traceback behavior.
- Preserved E1 witness attempts include toolchain and process-custody failures
  before semantic comparison. Recover those exact stages through the existing
  queue and require native and linked-WASM product verdicts; historical failures
  alone do not establish the current product frontier.
- Performance measurement pins host and guest profiles separately and records
  selected compiler content from build publication observations. That selection
  does not attest the loaded daemon or its complete feature/runtime identity.
  LLVM remains required by E2 while the prebuilt compiler feature set omits it;
  neither fact authorizes dropping LLVM evidence.
- Semantic execution receipts do not yet bind the actual Molt compiler
  digest/profile/features or runtime cell/generation. Candidate smoke checks
  and a common source SHA cannot establish that qualified binaries ship.
  Reuse the existing diagnosed compiler identity and runtime inventories.
- Project dependencies currently resolve for the CLI interpreter and host;
  unmanaged POSIX `.venv` roots can shadow `.molt-venv`. Target-version/platform
  admission and the target-dependent WASM oracle rule remain open. Source parsing
  requires a frontend at least as new as the target minor. Homebrew now binds
  Python 3.14 and preserves the full bundle projection; actual POSIX installation
  and older-target/newer-frontend acceptance still need execution.
- The dependency-command project boundary is implemented and has a real
  offline Windows CLI workflow. `test`, `bench`, `clean`, and the preview
  `lint`/`profile` commands still select compiler inputs for their working root.
  Their public roles and mutable-output ownership need reconciliation without
  re-tiering the stable commands or weakening the installed contract.
- Source-checkout measurements using the production compiler identify native
  IR growth/lowering, WASM linking, cache publication and runtime identity as
  compile-latency bottlenecks. The guest/runtime profiles were developmental;
  guarded scheduling and required Cargo rebuilds limit extrapolation to installed
  users. Shared exceptional cleanup now factors repeated release suffixes by
  exact SSA identity and continuation, using the shared ownership and point
  availability authorities. Handler argument custody, retain multiplicity and
  reverse temporary-release order remain explicit. The implementation and
  focused ownership-path check are followed by native and linked-WASM ownership
  guests matching the CPython 3.12 stdout oracle without Python on PATH. Native
  stderr also matches; the public Node host warning remains visible on WASM.
  Production SSA verification now checks exceptional captures at the actual
  observation position through shared program-point dominance, excluding
  non-executing region registrations. Split-continuation remapping uses that
  same authority for downstream operands, including exception-only entries.
  Focused regressions and public native/linked-WASM stdout replay pass on the
  integrated verifier/remapper change. Native stderr matches; independent WASM
  guest-stderr attribution and the wider declared matrix remain open.
  Native lowering now retains actual emitted immutable SSA values where shared
  unique-definition and execution-dominance facts prove availability. Mutable
  joins and stack/frame/resume custody retain explicit transport. Liveness,
  transport and snapshots use one canonical name-ID table with sparse operation
  sets, eliminating repeated string-set materialization and lookup. The legacy
  inferred phi-home write lane is deleted. Focused liveness and native codegen
  checks and public native/linked-WASM stdout replay of the transport implementation pass
  against CPython 3.12 with Python absent from guest PATH. Native guest stderr
  matches the oracle; independent WASM guest-stderr attribution and the wider
  declared matrix remain open.
  Coalesce shared cleanup runs with no alternate entries and measure the
  installed cold/warm/edit path; component results do not close those budgets.
- Warm cache hits still materialize and validate substantial frontend IR before
  final-artifact reuse. Optimize this through content-bound input receipts and
  live dependency resolution, retaining package-shadow and changed-policy
  invalidation. Historical source-checkout timings are not installed-user costs.
  The frontend also retains optimization passes beside the Rust midend, with
  conditional diagnostics and an unverified conservative retry after failed
  cross-block verification. Unify the shared fact/pass authority and preserve
  structural lowering; failed transformations must not disappear silently.
- Verified-subset coordinate jobs have finite six-hour budgets, and runtime-cell
  build jobs have ninety-minute budgets. Current costs expose campaign-capacity
  risks; representative timing is not proof that every coordinate is executable.
- The registry installation route has no PyPI publication stage. Wheel assets,
  package-manager publication and native binary signing/notarization need their
  distinct delivery evidence; H0 authentication remains a separate requirement.
- Shipped-runtime and WASM startup budgets are unseeded or absent. Complete the
  declared numeric compile, runtime, size and resource budgets through the
  existing performance authority.
- The public Node WASI runner emits a host experimental warning on stderr.
  Guest stderr parity and host diagnostics need explicit, lossless treatment.
- Frame/locals and async behavior still require complete target-version and
  target-execution coverage; an individual ownership regression is not the
  complete contract.
- Asyncio idle waiting now uses the runtime's ready/deadline authority and a
  sticky native wake route shared with signals and C pending calls. The Python
  capped-sleep lane is removed; poll-dependent WASM operations use existing
  task deadlines. This implementation is under integration: cancellation,
  timer, cross-thread and shutdown execution coverage, idle CPU, wakeup latency
  and I/O throughput remain to be qualified across the declared targets.
- Shared fusion and physical-storage corrections are integrated across the
  frontend, TIR, backends and runtime. Exact result facts replace prefix-based
  ownership; inline-list admission preserves heap owners and invalidates
  storage proofs across mutation, aliases and exposure. The representative
  bigint workflow now passes on native and linked WASM against CPython 3.12,
  including heap-sized literal fills and constant comprehensions. Strict mixed
  integer/float promotion now preserves conversion OverflowError across all nine
  arithmetic consumers; a 163-line CPython 3.12 oracle passes on native and linked
  WASM. Source-point boundness now governs plain locals, cells, coroutine slots
  and comprehension reads. The original native reduction program now confirms
  the overflow and empty-loop target fixes. Three output mismatches remain:
  numeric-subclass reflected callbacks and in-place error diagnostics. Four Python 3.13/3.14 fusion-test
  failures also reproduce with the pre-edit binding methods. These development
  results do not qualify the wider release matrix.
  The complete list native-payload, slots, lifetime and source/descriptor
  dispatch repair is now integrated. Exact builtin class provenance admits
  dispatch bypass; live physical kind selects Vec, compact-int or compact-bool
  storage through one shared native observer. Constructor result types are
  defined by shared IR semantics and consumed by both type refinement and exact
  allocator provenance. Generated alias roles preserve heap identity through
  copies, guards and owned captures without weakening ownership or escape rules.
  Focused runtime layout/ownership, retained-type and backend admission checks
  pass. The original 154-line descriptor program now matches CPython 3.12 on
  native and linked WASM with Python absent from guest PATH. Native stderr is
  empty; the same WASM manifest also passes guest/runner stderr comparison with
  Node host warnings retained separately. Shared solid-base selection, field
  projection and declaring-slot descriptors now reject incompatible layouts
  and preserve inherited physical owners. The expanded constructor program
  passes its 45-line oracle on native release. The maintained class-layout
  fixture also passes all 53 CPython 3.12 lines on native release, covering legal
  mixins, selected `__base__`, rebased fields, GC cycles, scalar-name collisions,
  redeclared/private slots, descriptor receiver validation, class transfer and
  finalizer reentry. Native stderr is empty and Python is absent from guest PATH.
  Class-private identifiers resolve before binding analysis and lowering;
  source spellings remain available for metadata and stringized annotations.
  A complementary native release program passes private parameters, closures,
  nested/global definitions, generators and coroutine completion against CPython.
  Slot admission now computes sealed dictionary/weakref capabilities for shared
  constructor and hot-query consumers. Its owning runtime checks and standalone
  native attribute workload pass; class-publication ordering, reentry and the
  remaining native dictionary consumers are still being integrated. Linked-WASM replay of the
  expanded layout/private-name workflows and the wider matrix remain required.
  Generic copy hooks/reconstruction, caller-visible deepcopy memo
  semantics and default subtype pickle reconstruction remain release blockers.
  Existing identity-return fallback is incorrect for mutable objects and must
  be replaced through the shared reconstruction protocol. The existing copy
  implementation also returns borrowed heap atoms under an owned-result
  convention and handles copied-child releases inconsistently. Repair ownership,
  failure cleanup, memo and reconstruction as one coherent protocol. Wider target/version/
  profile cells remain required. The four bounded reductions now use
  the shared compiler-only boxed ABI; the duplicate LLVM handler, symbol table,
  declaration rows and audit exemptions are removed. Other preserved Copy
  result classifications still need to agree with canonical ownership and
  representation facts without treating every runtime service as an operation.
- Installed distribution acceptance and reproducible cold/warm/edit compile,
  runtime, memory, startup, and size measurements remain incomplete.
- Important native/WASM surfaces do not yet have same-contract parity and
  complete release-cell receipts.
- Shared ownership coverage is not yet wide enough to delete every legacy
  native RC/value-tracking lane; `HandlerState` and finalizer parity remain part
  of that boundary.
- Language, stdlib, importlib, C-API, and ecosystem coverage remain incomplete.
- Windows native `stdlib_net` remains target-gated until constants, sockaddr
  layout, resolver calls, socket ownership, SSL custody, and async readiness
  share one WinSock authority.
- Benchmark results are not consistently faster than CPython, and tiny native
  binaries still carry too much fixed runtime surface and fresh-path startup
  cost for the five-year target.
- The GPU stack still lacks the MIL/MLIR MXFP and window/nonzero-pad contracts
  listed above; unsupported dialect paths must remain fail-closed.
- Luau parity remains incomplete; generated `not-admitted` rows are unsupported
  until admission and execution evidence change the matrix.

## Deferred By Policy

- Unrestricted `exec` / `eval` / `compile`.
- Runtime monkeypatching as a default compatibility strategy.
- Unrestricted reflection that violates Molt's AOT constraints.
- Unsupported dynamic `runpy` execution remains policy-governed rather than
  represented by a compatibility fallback lane.
