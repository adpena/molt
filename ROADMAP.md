# Molt Roadmap (Active)

For current supported state, use [docs/spec/STATUS.md](docs/spec/STATUS.md).
This file is forward-looking only. It is refreshed from live code, executable
tests, and generated evidence; the implementation remains the authority when a
roadmap claim drifts.

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

1. Close the ownership-correctness front before claiming broader compatibility:
   native DropInsertion activation, finalizer ordering, standalone `__del__`
   exception isolation, and ExceptionRegion Phase 1.
2. Delete legacy RC and compatibility lanes as each structural owner becomes
   authoritative; no second source of truth remains after a touched arc lands.
3. Replace hint-driven backend recovery with a shared representation-aware
   backend contract so native, WASM, and future LLVM lowering optimize from the
   same typed facts.
4. Drive native, WASM, and Luau toward the same supported contract.
5. Expand the first-class CLI/profile/target/backend validation matrix until
   every supported backend release claim is backed by end-to-end proof instead
   of backend-internal proof alone.
6. Finish consolidating setup, doctor, validate, and thin-wrapper behavior into
   one coherent CLI-first DX surface.
7. Keep Python `3.12`/`3.13`/`3.14` target-version gates explicit across CLI,
   pyproject/UV-oriented workflows, frontend caches, backend caches, and the
   unconditional runtime bootstrap/state contract used by importlib and stdlib
   gates across native, WASM, standalone Rust source emission, and isolate
   entry paths.
8. Make performance reporting and compatibility reporting generator-owned
   instead of manually synchronized across multiple docs.
9. Drive the Luau target from checked source emission to full current/future
   Luau parity coverage, with generated OpIR support evidence and no silent
   semantic stubs.
10. Add first-class output startup and binary-size evidence to the performance
    loop so regressions in minimum executable footprint, loader cost, dyld/code
    signature fixed costs, runtime feature bloat, native/WASM parity, and WASM
    payload size are caught before benchmark throughput hides them.
11. Keep intrinsic names equal to runtime symbols unless a future design proves
    a first-class alias abstraction is required; the retired async-sleep
    name/symbol bridge is not a compatibility pattern to revive.
12. Keep test execution under mandatory memory custody: direct pytest must
    enter startup custody through `sitecustomize.py`, the packaged pytest entry
    point, or the repo-configured plugin-autoload fallback before collection,
    then re-exec through the process-tree guard when unguarded; interpreter
    option forms and programmatic pytest launches must use pytest hook args as
    the authority. Legacy `*_MEMORY_GUARD=0` knobs are not custody bypasses.
    Tempfile-backed capture helpers, suite calibration, wasm diff, DX build
    timing, perf-scoreboard launchers, and CLI binary smoke probes must all use
    the same parent-side guard/profile/repro custody.
    Repo cleanup must prove host/Codex control-plane process groups are
    protected before any drain path can terminate Molt-owned workers, and every
    sentinel trip/drain/stale-preflight path must preserve sampled processes,
    external parents, resolved guard limits, and bounded repro context inline in
    JSON diagnostics. Cleanup JSON is the carrier for parsed sentinel events;
    raw sentinel streams stay verbose-only.
    Backend-daemon identity custody now lives in
    `src/molt/backend_daemon_custody.py`: CLI restart/stale-cleanup paths and
    `tests/molt_diff.py`, `tools/bench.py`, `tools/bench_wasm.py`, and
    `tools/bench_individual.py` use the shared verifier before signaling, and
    legacy `.pid` files are unlink-only debris. `tools/compile_progress.py`
	    excludes backend daemons from marker-scoped child cleanup, and
	    `tools/verify_native_binary_valid.sh` builds daemon-off without blanket
	    `pkill`. `tools/bench.py` and `tools/bench_wasm.py` now use cache-enabled
	    Molt builds by default, with `--no-molt-build-cache` reserved for deliberate
	    cold/no-cache studies. CLI rebuild/cache policy now treats shared
	    exact-key cache artifacts as immutable during build/link/probe hot paths,
	    publishes JSON/cache/text/byte/file/archive sidecars and final file
	    artifacts through atomic temp siblings, shares Cargo rebuild locks by
	    resolved build-state root, derives WASM runtime rebuild outputs from
	    Cargo `compiler-artifact` JSON instead of deleting candidate artifacts
	    before rebuild, requires byte-digest sidecars before hydrating WASM
	    runtime `.wasm`/`.a` candidates, replaces vendored directory
	    delete/copy with prepared temp-tree plus restore-on-failure
	    publication, and keys module-analysis artifacts by import scan
	    semantics. Next, extend the invalidation/retention evidence matrix and
	    wire OS-level directory exchange where available so
	    high-concurrency build lanes prove they rebuild only invalidated final
	    outputs or tool artifacts across native, WASM, LLVM, and Luau profiles.
13. Finish the large external-package compile-memory front with tinygrad as the
    canonical friend-suite driver. External roots are now resolvable without
    implying unlimited static closure; full package closure is explicit through
    `MOLT_EXTERNAL_STATIC_PACKAGES` and cache-keyed. The remaining work is to
    make the explicitly admitted tinygrad closure compile under guard by
    streaming/releasing frontend source, AST, and IR payloads instead of
    retaining the whole upstream graph through lowering/backend handoff.

## Milestone Sequence

### Near Term

- Finish the documentation-architecture cleanup and turn doc ownership into CI
  policy.
- Tighten compatibility rollups around generated evidence.
- Finish DropInsertion convergence from the live code state: repair the
  loop-phi raw/heap representation invariant that blocks native activation,
  re-run the WASM/native finalizer and RC-balance matrix, flip native only when
  the invariant is proven, then delete the competing native value-tracking RC
  lane instead of preserving a compatibility switch.
- Land ExceptionRegion Phase 1 as one complete ownership-boundary change:
  CreationRef release at the raise boundary and MatchRef release at
  `ExceptionPop` / handler-region exit. Do not land either half alone.
- Finish finalizer ownership boundaries: finalizer-bearing non-escaping objects
  drop at Python `del` / scope-exit rather than SSA last read, and exceptions
  raised from inline `__del__` paths are written as unraisable without escaping
  the compiled frame.
- Resume CallFacts only after the exception-region lane is no longer competing
  for the same no-throw/handler facts; its first landing must be a real
  generated/typed analysis surface, not comments or inert markers.
- Make typed SSA / explicit representation facts survive lowering without
  degrading into transport-only hints.
- Keep the TIR pipeline unconditional for backend-facing lowering; debugging
  uses dumps and verifier evidence rather than an environment-variable bypass.
- Close the highest-value native and WASM parity blockers.
- Burn down the tinygrad off-the-shelf blocker from the current disabled,
  pinned lane (`a83710396c991272241e40da94489747c2393851`): keep upstream
  tinygrad unmodified, keep `MOLT_EXTERNAL_STATIC_PACKAGES=tinygrad` as the
  explicit full-closure contract for the Molt runner, and reduce frontend
  compile RSS until the Molt runner reaches workload execution under the
  benchmark memory guard. The next structural landing point is path-backed
  `ModuleSourceLease`/analysis metadata so source strings and ASTs are not
  retained for the full graph, followed by byte-backed backend IR custody so
  daemon cache misses no longer embed a duplicated whole-program IR dict inside
  one giant JSON request.
- Continue the clean `molt-gpu` scheduler lane from the current code state:
  Movement/ShapeTracker binding is now a real per-kernel input-view contract
  with storage identity separated from binding identity across scheduler,
  fusion, CPU interpreter, runtime bridge, and executable renderers.
  `Contiguous` materialization now lowers to explicit copy kernels with fresh
  storage identity. CPU materialization is raw-byte exact; executable shader
  copy bodies stay fail-closed when backend dtype narrowing would break the
  copy contract. Metal e2e proof now covers flipped and padded materialization,
  raw `UInt16` copy preservation, and repeated-storage binding slots that share
  one device buffer; CPU copy proof covers every current dtype element width and
  padded raw zero-fill; runtime bridge proof covers one storage id routed
  through distinct CPU and Metal view slots; cross-renderer text proof covers
  non-float `UInt32` copy bodies. `bench_primitives` now proves contiguous CPU
  materialization at raw-copy class speed (`64.68 us` vs `53.70 us` raw copy
  for four source megabytes), flipped f32 materialization at `66.83 us`, flipped
  1/2/4/8-byte raw element rows at `66.52/66.43/66.60/66.46 us`, plus shrunk
  (`55.86 us`) and padded (`55.26 us`) single-view copies through raw-span
  plans. For non-MXFP storage, MLIR `MaterializeCopy` now emits flat memref
  arguments by binding slot plus generated ShapeTracker
  affine/mask arithmetic for contiguous, flipped, shrunk, padded/masked,
  permuted, composed, and expanded zero-stride views. MLIR compute now lowers
  pure elementwise kernels to real flat memref signatures, `scf.for` loops,
  ShapeTracker-indexed loads, masked-safe `scf.if` zero-fill, typed op SSA, and
  final stores; coverage includes flipped, padded/masked, same-storage distinct
  slots, composed views, integer-vs-float comparison typing, constants,
  prior-op chains, and explicit non-MXFP cast conversion selection. Cast target
  dtype now has first-class lazy/scheduler/runtime custody through
  `LazyOp::Cast`, `FusedOp::dst_dtype()`, and `molt_gpu_prim_cast`; CPU execution
  uses typed scalar Cast/Bitcast values for terminal, fused intermediate, and
  pre-reduce cases; old untyped unary Cast/Bitcast construction rejects.
	  Runtime tensor lifecycle now has typed raw upload and typed zero-fill through
	  `molt_gpu_prim_create_tensor_raw` and `molt_gpu_prim_zeros_dtype`, with MXFP
	  upload fail-closed until the block/exponent layout is explicit. Runtime
	  readback exposes dtype, logical storage byte count, and exact raw realized
	  storage-byte copy through `molt_gpu_prim_dtype`, `molt_gpu_prim_nbytes`, and
	  `molt_gpu_prim_read_data_raw`, while the legacy f32 readback API remains
	  fail-closed for realized non-Float32 tensors. Metal
	  e2e proof now byte-compares non-f32 Cast/Bitcast storage for
	  Float32->Int32/UInt16/UInt8 and equal-width Float32<->UInt32 against the CPU
	  interpreter.
	  Reductions now carry first-class `ReductionDomain` metadata through lazy
	  shape inference, scheduling, fusion, kernel hashing, CPU interpretation,
	  MIL rank restoration, and shader renderer codegen. CPU and shader
	  renderers lower the explicit row-major domain index instead of inferring
	  flat `input_numel / output_numel` segments, and fusion treats post-reduce
	  output-shape expansion as a hard boundary until broadcast-after-reduce is a
	  real IR primitive. MLIR now lowers `ReduceSum`/`ReduceMax` from that same
	  domain metadata with nested `scf.for` loops, dtype-correct accumulator
	  identities, pre-reduce prefixes, same-output-shape suffixes, and explicit
	  invalid-reference checks. MIL now carries ranked values through compute
	  lowering, reshapes flat gathered ShapeTracker views back to the logical
	  reduction input shape, emits domain axes against ranked tensors, and
	  returns the ranked reduction output shape. Shader renderers now reduce the
	  declared `reduce_op.srcs()[0]` rather than the last pre-reduce temporary.
		  `FusedOp`/`FusedOpDomain` construction is constructor-only with private
		  fields and accessor-based readers, so op/domain/dtype invariants cannot be
		  invalidated by post-construction mutation. MLIR MXFP buffer storage,
		  `MaterializeCopy`, constants, zero constants, element types, and quantized
		  casts stay fail-closed until explicit block/exponent storage plus conversion
		  lowering exist.
  MIL now lowers verified Bool, Int8/16/32, UInt8/16/32, Float16, and Float32
  `MaterializeCopy` views through explicit `range_1d`/`gather`/`select` tensor
  ops with safe masked gather indices, dtype-correct zero literals, physical
  offset span checks, and int32-domain guards; contiguous views return the input
	  binding directly. MIL compute read views remain Float32-only.
			  Next, add MIL BF16/64-bit/MXFP materialization only with real Core ML package
			  compile/run/raw byte-roundtrip proof, keep unsupported dialects fail-closed,
				  add MLIR MXFP block/exponent storage and `MaterializeCopy` lowering before
				  quantized cast lowering, then add a first-class window/im2col primitive
				  for tinygrad convolution wrapper migration and typed nonzero-pad
				  semantics. Constructor upload, typed zeros, dtype codes,
				  exact raw readback, public bytes/uint/int readback, elementwise
				  unary/binary operations, ternary `where`, typed casts, explicit-axis reductions,
				  Rust-owned all-axis reductions through `molt_gpu_prim_reduce_all`,
				  movement-family views, and matmul composition are now covered by
				  focused wrapper/runtime tests.
	  Off-the-shelf upstream tinygrad benchmarking is now a first-class driver:
	  `bench/friends/manifest.toml` registers the disabled-until-pinned
	  `tinygrad_off_the_shelf` suite, and `tools/tinygrad_off_shelf_adapter.py`
	  runs public tinygrad API workloads from the checked-out upstream package so
	  Molt can compile/profile unmodified tinygrad code against CPython and the
	  future official upstream runner. `tools/bench_friends.py` now fail-closes
	  git-source custody by verifying requested refs against checked-out `HEAD`,
	  requiring clean upstream checkouts, supporting per-suite `--suite-root` and
	  `--repo-ref` overrides for already-pinned clones, and ingesting adapter JSON
	  workload timings into `results.json` instead of relying only on subprocess
	  wall time.
- Import/bootstrap hardening: preserve the new module-init import-closure
  boundary as the graph-builder invariant. Plain package imports must admit only
  semantically required module-init dependencies; lazy backend/device families
  become explicit runtime/device edges. The CLI now materializes a single
  immutable `ImportPlan` after entry graph discovery and before frontend
  analysis; runtime-import support closure is owned by entry planning, while
  materialization owns namespace stubs, generated importer modules, known-module
  sets, allowlist snapshots, and graph metadata. Core stdlib closure now reuses
  the same nested-scan exception set as normal stdlib discovery, so modules such
  as `collections` admit required function-body support imports like `copy`
  without reopening third-party lazy backend families. Shared stdlib cache keys
  now seed every explicit stdlib module init exactly like backend DFE, and cache
  reuse requires backend-written partition manifests (sorted function names plus
  body hash) so stale objects cannot externalize a different stdlib partition.
  The Rust-owned import transaction now exists for the active importlib and
  `builtins.__import__` runtime paths, and the retired
  `molt_importlib_import_module` intrinsic/export/WASM row has been deleted so
  importlib cannot drift through a resolved-name-only side door. The empty
  `_MODULE_ALIASES` branch is gone, and frontend literal/direct-call folding of
  `importlib.import_module("literal")` now emits a direct
  `molt_importlib_import_transaction(name, None, None, ("*",), 0)` call
  whenever callable identity and an absolute literal name are statically proven;
  runtime import owns target availability, version-gated absence, module cache
  custody, provenance, fromlist behavior, and error shape. Folding remains
  disabled whenever the module attribute is syntactically rebound through
  `importlib` or a module alias. The next structural import step is to split
  public API validation while sharing the private resolver, implement CPython
  3.12 package-context calculation (`__package__` / `__spec__.parent` /
  `__name__` fallback), carry source syntax imports into a coherent transaction
  boundary without creating a second public API path, and complete `fromlist`
  auto-import/binding semantics.
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
  WASM must share one per-intrinsic/per-primitive reachability authority. The
  WASM non-reloc Auto path now uses emitted import lookups as the final
  retention authority and strips unreferenced imports after validation; the
  conservative dependency scan is reloc-only linker input, not a second
  non-reloc truth source.

### Long Term

- Broaden extension support through `libmolt`.
- Push native and WASM performance toward the project target.
- Make cold-start and binary-size gates as central as throughput gates across
  native, WASM browser/Node/Cloudflare, LLVM/MLIR, and Luau output surfaces.
- Continue converging on a larger practical CPython 3.12+ surface without
  regressing determinism or packaging guarantees.

## Active Blockers

- Incomplete same-contract parity between native and WASM for important surfaces.
- TIR DropInsertion is implemented and active on LLVM/WASM/Luau, but native is
  still gated off by the loop-phi representation bug in drop-inserted RC paths.
  The blocker is structural: keep a single consistent representation across all
  incoming block-arg edges or fail closed, then delete the legacy native RC
  substrate that currently owns the native path.
- Finalizer dispatch is present, but finalizer ordering and standalone
  `__del__` exception isolation are not complete. These are ownership-boundary
  defects, not benchmark-only defects.
- ExceptionRegion / HandlerState ownership is not landed; current recovered WIP
  is a baton only and must not be accepted as a partial fix.
- Incomplete compatibility coverage across language and stdlib.
- Container/list/dict semantic dispatch is now represented through the shared
  representation plan; remaining work is storage-proof precision,
  backend-parity evidence, and deletion of any residual string-hint dispatch
  lanes that bypass structured `TirType` / `ContainerStorageKind` facts.
- Benchmark suite results are not yet consistently faster than CPython across
  all tracked lanes.
- Tiny native binaries currently have too much fixed linked runtime surface and
  measurable fresh-path startup cost for the five-year <50 ms / <2 MB target.
  The active path is a measured `RuntimeSurfacePlan` that drives native
  link-root selection, WASM import/export manifests, and intrinsic resolver
  generation from the same program reachability facts, not ad-hoc linker flag
  churn.
- The molt-gpu scheduler now binds `Movement` DAG operands as zero-copy
  source-storage views, executable renderers share ShapeTracker index/mask
  codegen for padded/masked reads, the runtime bridge passes full
  source-storage bytes by `buf_id`, and `Contiguous` DAG operands materialize
  through explicit copy kernels with fresh storage identity. The copy path now
  has a `bench_primitives` evidence row; contiguous CPU materialization is
  raw-copy class speed, shrunk/padded single-view materialization is raw-span
  speed, and flipped single-view materialization now uses the preflighted
  fixed-width reverse-copy path across 1/2/4/8-byte raw elements. Non-MXFP MLIR
  materialization now has positive flat-memref ShapeTracker lowering proof. MLIR
  pure elementwise compute now has positive real-memref loop proof with
  ShapeTracker-indexed input loads, masked-safe zero-fill, and explicit non-MXFP
  cast conversion selection plus lazy/scheduler/runtime target dtype custody.
  MIL has positive gather/select materialization proof with safe masked gather
  ordering for Bool, Int8/16/32, UInt8/16/32, Float16, and Float32 storage. CPU
	  typed scalar execution now proves non-f32 Cast/Bitcast values through terminal,
	  intermediate, and pre-reduce paths; runtime upload/readback has typed raw
	  storage APIs while the f32 public readback API fails closed on realized non-Float32
		  tensors; Metal now has raw non-f32 Cast/Bitcast byte proof; and reductions
		  now carry explicit `ReductionDomain` metadata through scheduler, fusion,
		  CPU, MIL ranked reduction lowering, shader codegen, and MLIR reduction
			  loops, including affine non-last-axis renderer, MIL ranked-axis proof, and
			  MLIR loop proof. Remaining GPU blockers are MIL BF16/64-bit/MXFP
				  materialization proof, MLIR MXFP block/exponent storage plus
				  `MaterializeCopy` lowering, MLIR MXFP quantized cast lowering, a
				  window/im2col primitive for tinygrad convolution, and typed
				  nonzero-pad semantics beyond the
				  now-runtime-backed constructor/zeros/readback, unary/binary/cast,
				  ternary `where`, explicit-axis reduce, Rust-owned all-axis reduce,
				  movement-family, and matmul-composition paths.
  Unsupported dialect paths stay fail-closed rather than treating a view/copy
  contract as a raw passthrough.
  TODO(perf, owner:runtime, milestone:RT, priority:P2, status:in-progress).

## Deferred By Policy

- Unrestricted `exec` / `eval` / `compile`.
- Runtime monkeypatching as a default compatibility strategy.
- Hidden host-CPython fallback paths in compiled binaries.
- Unrestricted reflection that violates Molt's AOT constraints.
- The runpy dynamic-lane expected failures list is currently empty because
  supported lanes moved to intrinsic support; unsupported runpy dynamic
  execution remains policy-governed rather than represented by an active
  expected-failure lane.
