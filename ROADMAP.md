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

Version 1.0 is Molt's first public release. It must be useful for real work,
and measured correctness, conformance, performance and security evidence must
back every claim it makes. Release acceptance follows the
[packaging authority](packaging/PACKAGING.md) and the
[performance acceptance scope](tools/PERF_AUTHORITY.md#v10-acceptance-scope).
A missing measurement is a gap, not a pass. A demonstration or one green cell
does not qualify the release.

The approved v1.0 plan runs in this order:

1. **Cut line.** Write the compatibility promise and the support matrix. The
   owner signs it. Every open ledger row is classified as release-blocking or
   post-1.0.
2. **Typed contracts over harness checks.** A fact the type system can enforce
   moves into a generated, closed type. Source-scraping audits and harness
   checks retire when the type enforces the same fact (see below).
3. **One environment.** Development, the Linux VMs and CI run one environment
   definition.
4. **CPython's own tests as the oracle.** CPython's regrtest becomes the
   acceptance oracle (HF-488). The ledger becomes executable: every open
   finding has a test keyed by its typed ID.
5. **The fast loop.** Cut the test-suite time (HF-263) and make compile time
   scale with program size (HF-549).

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
| P0 | Native (Cranelift by default, LLVM opt-in), WASM, LLVM, JIT | Release cells come from these backends. |
| Experimental | Luau, the Rust source emitter, MLIR | No release claim until admitted cells exist. |

Every backend compiles against the same closed typed contract: it implements
an op or returns a typed refusal before it produces an artifact. The
experimental backends keep contract parity, so a new op cannot silently skip
them. The [Luau matrix](docs/spec/areas/compiler/luau_support_matrix.generated.md)
shows which Luau rows are admitted.

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
