# Molt Roadmap

This is Molt's one forward plan. It states direction and order, not current
state. Current support lives in [STATUS.md](docs/spec/STATUS.md) and its
generated matrices. Open findings, release blockers and the burn-down order
live in the [findings ledger](docs/agent/V1_HANDOFF_FINDINGS.md). Live code,
executable tests and generated evidence win when prose drifts.

## Product contract

- Molt compiles a verified subset of Python ahead of time into standalone
  native and WASM programs. The
  [verified-subset contract](docs/spec/areas/compat/contracts/verified_subset_contract.md)
  owns the intended semantics.
- The target is full CPython `>=3.12` parity for that subset, with
  deterministic behavior by default.
- Compiled programs never use a host Python interpreter. There is no
  host-CPython fallback, now or as a future compatibility path.
- Unrestricted `exec`/`eval`/`compile`, runtime monkeypatching and unrestricted
  reflection stay outside the subset. The
  [dynamic-execution policy](docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md)
  owns that boundary and its expansion gates.
- Ecosystem support comes from compiler and runtime primitives, package
  custody, source-built extensions and a correct C-API/ABI. Molt does not
  reimplement NumPy, SciPy or other upstream packages.

## First Release Milestone

Molt's first public release is v0.1 (owner decision 2026-11-05). The
[public claim](docs/spec/PUBLIC_CLAIM_V0_1.md) defines the two flagship
witnesses and links the Tier 1 cells and generated known-issues list.

Version 1.0 is the stable release for real-world use. Its compatibility and
support promises require evidence of generated-program performance, user-code
compilation latency, correctness, conformance and security. The
[findings ledger](docs/agent/V1_HANDOFF_FINDINGS.md) owns blocker status and
execution order. Release acceptance follows the
[packaging authority](packaging/PACKAGING.md) and the
[performance acceptance scope](tools/PERF_AUTHORITY.md#v10-acceptance-scope).

### Engineering sequence for Version 1.0

Advance the closed-world compiler and the installed release proof together.
The [findings ledger](docs/agent/V1_HANDOFF_FINDINGS.md#active-integration)
owns the current work and evidence.

- **Build one usable program model.** Track callable identity, argument binding,
  possible writers, object layout, lifetime and observable effects through typed
  SSA. Start demand at executable and escaping roots. Reuse each proven context
  for reachability, specialization, inlining and shared lowering. Apply the same
  model to application and stdlib code, so a proven call, field access or
  allocation can execute directly and unreachable code can leave the image.
  Use independent CPython controls for mutation, exceptions, identity, lifetime
  and extension boundaries. Measure emitted code against the C reference.
  See [whole-program type flow](docs/design/foundation/78_whole_program_type_flow.md)
  and [call anatomy](docs/design/foundation/call_anatomy.md).
- **Reuse the work that survives an edit.** Keep the dependencies of a semantic
  summary, optimized body, target object and linked product explicit. Changing a
  callee's body invalidates callers that inline it. A caller that reads only an
  unchanged return or effect summary keeps that analysis. Carry these identities
  through installed builds, worktrees and CI. Measure cold builds, representative
  leaf edits and the immediate unchanged rebuild independently of the runtime
  performance of the resulting programs.
  See [build early cutoff](docs/design/build_early_cutoff.md) and
  [performance measurement](tools/PERF_AUTHORITY.md#finding-the-limiter).
- **Exercise the real release path early.** Drive one representative standalone
  native workload and one Wasm workload through the installed compiler, runtime
  selection, link/publication, execution and authenticated evidence receiver.
  Prove that the receiver rejects omitted or substituted evidence. Extend that
  same path across the declared matrix, with source-bound measures of runtime,
  build latency, resource use and artifact size. This is the route to the
  [phase exit](docs/design/CENTURY_SYSTEMS_PLAN.md#5-horizons-and-machine-checkable-phase-exits), alongside
  closure of the compatibility and security obligations.

### Typed identity comes first

String-literal identity is absolute poison. Op kinds, exceptions, classes,
decorators and attribute keys come from one declarative source that generates
closed types, so the compiler rejects a stale consumer. The order:

1. The closed `OpKind` in Rust (step 4a, landed). See the
   [op-kind registry](docs/design/foundation/25_op_kind_registry.md).
2. Python `Literal` op kinds checked under `ty` (step 4b, in progress).
3. The generated runtime-call contract (HF-485). It shares each op's meaning
   and facts across backends. Each backend may specialize how it realizes an
   op only behind a declared precondition, checked against the runtime-call
   reference.

### Backends

| Tier | Backends | Rule |
| --- | --- | --- |
| P0 | Native (Cranelift by default, LLVM opt-in), WASM (LLVM's wasm32 target), LLVM, JIT | Release cells come from these backends. |
| Experimental | MLIR | No release claim until admitted cells exist. |

Every backend compiles against the same closed typed contract: it implements
an op or returns a typed refusal before it produces an artifact. The
experimental MLIR backend keeps contract parity, so a new op cannot silently
skip it.

### Guardrails

- No import-system redesign lands unless the `test_importlib` cell and the
  import finding tests stay green.
- A correctness or performance claim names its exact cell: Python version,
  OS, architecture, backend, profile and optimization level.
- A compiler pass never trades observable behavior for speed. Speed comes from
  proven facts on typed SSA values, not from name-matched lowering.

## After v1.0

These are directions, not a schedule. Each one has an owning design document.

- **Broader compatibility.** Grow the verified subset through the
  [type coverage matrix](docs/spec/areas/compat/surfaces/language/type_coverage_matrix.md),
  the [stdlib surface matrix](docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md)
  and the [compatibility indexes](docs/spec/areas/compat/README.md).
  Portable extensions grow through the
  [libmolt ABI contract](docs/spec/areas/compat/contracts/libmolt_extension_abi_contract.md).
- **Generated-code speed.** Hold CPython as the floor on every claimed cell,
  then approach PyPy on dynamic code and Codon on static code. The
  [compression ladder](docs/design/foundation/65_perf_compression_ladder.md)
  retires one class of slowness per rung, on the
  [semantic fact plane](docs/design/foundation/59_semantic_fact_plane.md).
- **Footprint.** Ratchet cold start toward 50 ms and binary size toward 2 MB
  through [binary size](docs/design/foundation/61_binary_size_and_output_optimization.md)
  and [cold start](docs/design/foundation/62_startup_cold_start.md).
- **GPU and ML.** Extend the tinygrad-conformant primitive stack through the
  [GPU primitive architecture](docs/architecture/gpu-primitive-stack.md) and
  the [tinygrad/DFlash contract](docs/design/foundation/67_compat_tinygrad_dflash.md).
  Unsupported dialect paths stay fail-closed.
- **Concurrency.** Design for free-threaded and GIL-less execution while the
  default stays deterministic, per the
  [concurrency contract](docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md).
- **Execution lanes.** An interpreter lane (Monty) or a protobuf audit format
  (Buffa) stays research until it proves parity. Neither may grant host
  authority; the [capability system](docs/CAPABILITIES.md) owns that rule.

The long-horizon engineering plan, its horizons and its machine-checked phase
exits live in the [century systems plan](docs/design/CENTURY_SYSTEMS_PLAN.md).
The product vision lives in [the vision spec](docs/spec/areas/core/0000-vision.md).

## Where each fact lives

| Fact | Authority |
| --- | --- |
| What works today | [STATUS.md](docs/spec/STATUS.md) and its generated matrices |
| Open findings, blockers, burn-down order | [Findings ledger](docs/agent/V1_HANDOFF_FINDINGS.md) |
| Release acceptance and shipped cells | [Packaging authority](packaging/PACKAGING.md) |
| Performance claims and budgets | [Performance authority](tools/PERF_AUTHORITY.md) |
| Long-horizon plan and phase exits | [Century systems plan](docs/design/CENTURY_SYSTEMS_PLAN.md) |
| Documentation map | [Canonical reading list](docs/CANONICALS.md) |
