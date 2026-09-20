# Molt Roadmap (Active)

For current supported state, use [docs/spec/STATUS.md](docs/spec/STATUS.md).
This file is forward-looking only. Live code, executable tests, generated
matrices, and replayable receipts remain authoritative when prose drifts. The
[canonical reading list](docs/CANONICALS.md) routes detailed engineering plans;
this roadmap does not duplicate their implementation history.

## First Release Milestone

The immediate P0 is an evidence-backed `v0.0.1` initial release; the next major
release objective is the stable `v1.0` contract. Rank work by release-blocker
closure and measured reduction of the release critical path. Broader expansion
must not displace either objective. The exact version and tag follow the existing
[packaging authority](packaging/PACKAGING.md), not a separate roadmap version.
Consolidate and land the owned compiler work on `origin/main` first, preserving
unique WIP and retiring replaced authorities. Close known correctness failures
in the advertised subset before promoting release artifacts.

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

## Strategic Target

- Reach full CPython `>=3.12` parity for the supported Molt subset.
- Ship standalone binaries with no hidden host Python installation fallback.
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

1. Consolidate before expanding. Land the current compiler/runtime authority
   work, preserve unrelated donor WIP, and delete replaced classifiers,
   compatibility lanes, and backend-local semantic copies. Generated tables and
   shared typed facts must be the only authorities after migration.
2. Close ownership and exception correctness through the complete shared
   `DropInsertion`, `ExceptionRegions`, `HandlerState`, and finalizer boundary.
   Native, WASM, LLVM, and Luau consumers must use the same facts; stale native
   value-tracking release lanes can be removed only after the wider boundary is
   proved. Detailed ownership plans live in
   [RC ownership and drop insertion](docs/design/foundation/20_rc-ownership-drop-insertion.md)
   and
   [exception-region ownership](docs/design/foundation/45_exception_region_ownership.md).
3. Keep representation and optimization decisions in typed IR. Continue
   removing `fast_int`, `fast_float`, string `type_hint`, raw-scalar shadow, and
   backend-reconstructed container lanes in favor of shared TIR/LIR value,
   representation, range, storage, and effect facts. The portfolio route is
   [the semantic fact plane](docs/design/foundation/59_semantic_fact_plane.md).
4. Prove backend and target claims at the real consumer. Native and linked-WASM
   guest execution come first; LLVM, Luau, Rust-source, browser, and other
   profiles remain limited to the exact cells with replayable end-to-end
   evidence. Use the
   [minimum must-pass matrix](docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md)
   rather than backend-internal checks as a release proxy.
5. Expand compatibility without hidden host fallback. Continue language,
   stdlib, import transaction, source-extension, C-API, NumPy, and SciPy work
   through the generated
   [compatibility indexes](docs/spec/areas/compat/README.md) and the
   [libmolt extension ABI contract](docs/spec/areas/compat/contracts/libmolt_extension_abi_contract.md).
   Pinned upstream packages remain upstream-owned; Molt owns generic custody,
   ABI, and integration.
6. Advance tinygrad and GPU support through real storage and execution
   contracts. The next material gaps are MIL BF16/64-bit/MXFP proof, MLIR MXFP
   block/exponent storage and materialization, quantized casts, a first-class
   window/im2col primitive, and typed nonzero padding. Route this work through
   the [GPU primitive architecture](docs/architecture/gpu-primitive-stack.md),
   [GPU/MLIR plan](docs/spec/areas/perf/0513_GPU_PARALLELISM_AND_MLIR.md), and
   [tinygrad/DFlash contract](docs/design/foundation/67_compat_tinygrad_dflash.md).
7. Make cold start, binary size, and build throughput first-class product
   constraints. A shared reachability/runtime-surface plan must drive native
   link roots, WASM imports/exports, and intrinsic resolution; linker flags and
   one-off measurements are not substitute authorities. See the
   [binary-size](docs/design/foundation/61_binary_size_and_output_optimization.md),
   [cold-start](docs/design/foundation/62_startup_cold_start.md), and
   [DX/build-speed](docs/design/foundation/56_dx_buildspeed_tooling.md) plans.
8. Keep CLI, build, test, benchmark, and proof execution under one deterministic
   custody model. Setup, doctor, validate, profile selection, artifact roots,
   process closure, and failure classification must stay replayable and must not
   create parallel runner or cleanup authorities. Operational detail belongs in
   [OPERATIONS.md](docs/OPERATIONS.md).
9. Keep Luau support claims generated and fail-closed. The
   [generated Luau support matrix](docs/spec/areas/compiler/luau_support_matrix.generated.md)
   is authoritative. Promote an operation only after the shared target contract
   admits it and checked emission plus CPython-vs-Luau execution prove the
   claimed semantics.

## Milestone Sequence

### Near Term

- Finish consolidation and land the current authority cohort before opening new
  compiler-frontier lanes.
- Complete shared ownership/exception/finalizer closure and delete the obsolete
  sibling lanes it replaces.
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
- Hidden host-CPython fallback paths in compiled binaries.
- Unrestricted reflection that violates Molt's AOT constraints.
- Unsupported dynamic `runpy` execution remains policy-governed rather than
  represented by a compatibility fallback lane.
