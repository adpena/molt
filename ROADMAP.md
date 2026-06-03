# Molt Roadmap (Active)

For current supported state, use [docs/spec/STATUS.md](docs/spec/STATUS.md).
This file is forward-looking only.

## Strategic Target

- Reach full CPython `>=3.12` parity for the supported Molt subset.
- Ship standalone binaries with no hidden host Python installation fallback.
- Outperform CPython on the benchmark suites Molt claims as core product lanes.
- Treat tiny-program cold start and output binary size as product-critical
  performance axes, not secondary packaging polish: native, browser/WASM, Luau,
  and MLIR output surfaces must have the same canonical evidence matrix, with
  release artifacts ratcheting toward <50 ms cold start and <2 MB
  gzipped/runtime payloads on the five-year arc.
- Preserve Molt's design exclusions around runtime monkeypatching,
  unrestricted dynamic execution, and unrestricted reflection.

## Current Priorities

1. Close correctness gaps in the compiler/runtime path before claiming broader
   compatibility.
2. Replace hint-driven backend recovery with a shared representation-aware
   backend contract so native, WASM, and future LLVM lowering optimize from the
   same typed facts.
3. Drive native, WASM, and Luau toward the same supported contract.
4. Expand the first-class CLI/profile/target/backend validation matrix until
   every supported backend release claim is backed by end-to-end proof instead
   of backend-internal proof alone.
5. Finish consolidating setup, doctor, validate, and thin-wrapper behavior into
   one coherent CLI-first DX surface.
6. Keep Python `3.12`/`3.13`/`3.14` target-version gates explicit across CLI,
   pyproject/UV-oriented workflows, frontend caches, backend caches, and the
   unconditional runtime bootstrap/state contract used by importlib and stdlib
   gates across native, WASM, standalone Rust source emission, and isolate
   entry paths.
7. Make performance reporting and compatibility reporting generator-owned
   instead of manually synchronized across multiple docs.
8. Drive the Luau target from checked source emission to full current/future
   Luau parity coverage, with generated OpIR support evidence and no silent
   semantic stubs.
9. Add first-class output startup and binary-size evidence to the performance
   loop so regressions in minimum executable footprint, loader cost, dyld/code
   signature fixed costs, runtime feature bloat, native/WASM parity, and WASM
   payload size are caught before benchmark throughput hides them.

## Milestone Sequence

### Near Term

- Finish the documentation-architecture cleanup and turn doc ownership into CI
  policy.
- Tighten compatibility rollups around generated evidence.
- Make typed SSA / explicit representation facts survive lowering without
  degrading into transport-only hints.
- Keep the TIR pipeline unconditional for backend-facing lowering; debugging
  uses dumps and verifier evidence rather than an environment-variable bypass.
- Close the highest-value native and WASM parity blockers.
- Establish baseline and ratchet artifacts for tiny hello-world output rows
  across native, linked-WASM, Luau, and MLIR: release/dev size, backend/profile
  dimensions, same-path startup where runnable, fresh-path startup where
  runnable, CPython process baseline, and tiny C baseline. WASM already proves
  that tiny payloads are possible under the right linked/profile discipline;
  native must converge by making runtime reachability as precise as the WASM
  import/export path rather than relying on coarse domain features alone.
- Keep the generated Luau support matrix current and use it to prioritize
  checked CPython-vs-Luau feature-gap closure.
- Keep the canonical local validation matrix green across:
  - `molt validate --suite smoke`
  - `molt validate`
  - native `build` / `run` / `compare` on `dev` and `release`
  - linked-WASM build plus Node execution
  - honest unsupported-semantics failures (`exec` / `eval`)

### Medium Term

- Expand language and stdlib coverage under the Rust-first lowering model.
- Keep retiring `fast_int` / `fast_float` / `type_hint` transport hints and raw
  scalar shadow lanes as the architectural center of backend optimization.
  TIR-to-SimpleIR lowering no longer accepts an external type map for scalar
  hint reseeding; remaining performance work must flow through shared
  representation-aware TIR/LIR contracts. Native int codegen has retired the
  raw-int shadow transport in favor of `int_primary_vars`, and native
  float-primary codegen now treats
  `float_primary_vars` as the single static authority for F64-primary variables.
  TIR functions now own a persistent `value_types` map, and type refinement
  writes op-result facts back into that map instead of leaving them as a
  transient extraction side table; range/list devirtualization also records the
  I64/Bool facts it synthesizes for loop-carried values. Backend scalar
  lowering now builds a final-codegen-time `ScalarRepresentationPlan` from
  refined TIR/LIR facts, `SimpleValueNames`, and explicit single-output
  provenance, then derives semantic scalar/None classifications from that plan
  instead of recomputing them from SimpleIR op strings. Native uses the plan for
  raw-primary carrier eligibility, scalar slot escape safety, scalar
  store-target discovery, and operation lane preference, while preserving the
  stricter exact-carrier safety predicates needed by codegen. Legacy WASM and
  Luau scalar fast paths now consume the same plan for integer-family
  arithmetic, comparison, truthiness, and index-key scalar decisions instead of
  trusting `fast_int`, `fast_float`, or scalar `type_hint` transport metadata.
  Generic container annotations now parse through the same TIR type authority:
  `list[T]`, `dict[K, V]`, `set[T]`, and fixed-arity `tuple[...]` produce
  structured `TirType` facts for SSA/value maps while malformed, dynamic, or
  unsupported compound hints remain `DynBox`.
  Backend semantic container dispatch now reads those facts through the shared
  plan for Luau, WASM import selection/emission, native `len`/`contains`, and
  LLVM `len`; `container_type` / `type_hint` strings alone no longer select
  those specialized paths. Semantic `list[int]` is not treated as flat
  `list_int` storage proof; native direct storage optimizations now require a
  shared `ContainerStorageKind::FlatListInt` fact seeded by structural
  `list_int_new` producers and queried through the representation plan.
  Native bool codegen has the same raw-closed `bool_primary_vars` contract for
  constants, alias/store propagation, comparisons, identity checks, and
  truthiness casts. Bool-primary escape points now box raw `0/1` carriers
  through a dedicated raw-bool boxing helper instead of feeding I64 carriers to
  the b1-condition bool boxer. Raw-closed bool join carriers also stay on the
  main raw `0/1` Variable contract across store/load/copy and structured phi
  binding, while unsafe join slots remain boxed. Proven-bool list indexing now
  enters the same bool-primary contract when the index operand is raw-primary,
  keeping the existing index-fast-path selection separate from output
  representation. Unknown-list getitem truthiness uses an explicit conditional
  list-bool carrier for the runtime list/list_bool split. Float primary
  eligibility is now definition-scoped: unsupported producers such as `pow`
  keep only their own outputs boxed and cannot disable unrelated proven float
  locals in the same function. The native raw-f64 shadow lane is retired:
  `float_primary_vars` is the only raw-F64 authority, and non-primary floats
  are boxed immediately in their main I64 variable. Native scalar store-target
  discovery is now shared across int, float, bool, and str lanes, preserving
  the all-sources rule. The native raw-bool shadow lane is retired as well:
  `bool_primary_vars` is the only raw-bool authority, and non-primary bools
  stay boxed in their main I64 variables. Native int-primary now means exact
  i64 representation, not semantic Python `int`; bounded add/sub and
  raw-closed counted store/load loop carriers may enter raw-primary only after
  shared interval proof, while unbounded arithmetic and shifts stay
  boxed/runtime-backed until range and shift-count proofs make raw lowering
  sound.
- Harden daemon, build, and harness workflows for multi-agent development.
- Move more hot semantics into runtime primitives and intrinsics.
- Split runtime/stdout/bootstrap feature surfaces through a shared
  `RuntimeSurfacePlan` so a tiny supported program does not link async,
  logging, filesystem/tempfile, GPU, networking, UI, or compatibility
  subsystems it cannot reach. Link-time dead stripping remains necessary but is
  not sufficient for five-year binary-size and cold-start targets; native and
  WASM must share one per-intrinsic/per-primitive reachability authority.

### Long Term

- Broaden extension support through `libmolt`.
- Push native and WASM performance toward the project target.
- Make cold-start and binary-size gates as central as throughput gates across
  native, WASM browser/Node/Cloudflare, LLVM/MLIR, and Luau output surfaces.
- Continue converging on a larger practical CPython 3.12+ surface without
  regressing determinism or packaging guarantees.

## Active Blockers

- Incomplete same-contract parity between native and WASM for important surfaces.
- Incomplete compatibility coverage across language and stdlib.
- Container/list/dict dispatch still needs a backend-neutral representation
  plan; scalar fast-path authority has converged on `ScalarRepresentationPlan`
  across native, legacy WASM, and Luau.
- Benchmark suite results are not yet consistently faster than CPython across
  all tracked lanes.
- Tiny native binaries currently have too much fixed linked runtime surface and
  measurable fresh-path startup cost for the five-year <50 ms / <2 MB target.
  The active path is a measured `RuntimeSurfacePlan` that drives native
  link-root selection, WASM import/export manifests, and intrinsic resolver
  generation from the same program reachability facts, not ad-hoc linker flag
  churn.
- The molt-gpu scheduler cannot yet bind `Movement`/`Contiguous` DAG nodes as
  kernel operands: a Movement consumer needs the movement's `ShapeTracker`
  (strides/offset) threaded into its input binding, and `Contiguous` needs a
  materialization (copy) kernel. Both are scheduling passthroughs today that
  drop the view, so the path is fail-closed (`ScheduleCtx::buf_id_for` panics
  rather than mint a fresh unproduced buffer id that would silently route the
  consumer to zeros) and unreachable via the realize FFI until tinygrad-faithful
  movement-view threading and a `Contiguous` copy kernel land.
  TODO(perf, owner:runtime, milestone:RT, priority:P2, status:missing).

## Deferred By Policy

- Unrestricted `exec` / `eval` / `compile`.
- Runtime monkeypatching as a default compatibility strategy.
- Hidden host-CPython fallback paths in compiled binaries.
- Unrestricted reflection that violates Molt's AOT constraints.
- The runpy dynamic-lane expected failures list is currently empty because
  supported lanes moved to intrinsic support; unsupported runpy dynamic
  execution remains policy-governed rather than represented by an active
  expected-failure lane.
