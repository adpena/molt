# STATUS

This document is a current-state summary derived from the live codebase,
executable tests, and generated evidence. The code and tests are the sole source
of truth; when this file conflicts with implementation, update this file from
the implementation. For forward-looking priorities, use
[ROADMAP.md](../../ROADMAP.md).

## Project Scope And Target

- Strategic target: full CPython `>=3.12` parity for supported Molt semantics.
- Product target: full CPython `>=3.12` parity without hidden host fallback for
  the semantics Molt claims today.
- Compiled binaries must not rely on a host Python installation.
- runtime monkeypatching, unrestricted `exec`/`eval`/`compile`, and unrestricted
  reflection remain intentional design exclusions for compiled binaries.

## Supported Today

- Native AOT compilation is real and active.
- Standalone binary workflows are a first-class product requirement.
- Differential testing against CPython is a core validation path.
- Build target semantics are explicit for Python `3.12`, `3.13`, and `3.14`:
  `molt build --python-version`, `[tool.molt.build] python-version`, and
  `project.requires-python` resolve the target version before parsing, module
  graph discovery, frontend cache lookup, backend cache lookup, and runtime
  `sys.version_info` bootstrap. The compiled bootstrap is unconditional: native,
  WASM, standalone Rust source emission, and isolate entry paths stamp the
  selected target version before user code/importlib gates run. Runtime
  version-gated stdlib decisions read that runtime state instead of ambient
  process env, and Rust source outputs materialize `sys.version_info`,
  `sys.version`, and `sys.hexversion` from the same stamped state. Rust source
  outputs also own executable module-cache get/set/delete semantics for emitted
  import bootstrap IR, with cache misses represented as `None` rather than a
  truthy sentinel. Luau source outputs materialize the same target-version `sys`
  metadata into `molt_module_cache["sys"]`, and dynamic Luau module import now
  fails closed instead of manufacturing empty table fallbacks for unsupported
  modules. Malformed or non-string target-version config fails closed instead of
  falling back to another target. The default target remains Python `3.12`.
- Rust-first stdlib lowering is the canonical direction, with generated audit
  surfaces under `docs/spec/areas/compat/surfaces/stdlib/`.
- Build-time import graph discovery now separates external-root resolution from
  external-package admission. `MOLT_MODULE_ROOTS`, `--lib-path`, respected
  `PYTHONPATH`, and auto site-packages can make a package resolvable, but
  transitive external package closure is admitted only by direct entry imports
  or an explicit `MOLT_EXTERNAL_STATIC_PACKAGES` package declaration. The graph
  cache key includes this policy, and frontend known-module checks no longer
  treat a known top-level package as authorization for every dotted child.
- The `molt_async_sleep` intrinsic now owns the public two-argument sleep-future
  constructor symbol directly. The one-argument internal poll callback is
  `molt_async_sleep_poll`; the legacy `molt_async_sleep_new` name/symbol bridge
  and backend override table are removed.
- Direct-call default filling observes live function metadata when the callee
  object is reachable: module direct calls and guarded function-object calls
  read `__defaults__` / `__kwdefaults__` for literal and dynamic default specs
  instead of baking sema-time literals. Constructor and method call paths rely
  on the same live metadata through direct padding or the runtime binder.
- `molt-gpu` schedules `Movement` operands as zero-copy views over source
  storage and schedules `Contiguous` DAG operands as first-class
  `KernelBody::MaterializeCopy` producers with fresh storage identity.
  `BufferBinding::buf_id` remains runtime storage identity, binding slot index
  remains renderer parameter identity, fusion preserves same-storage /
  different-view external inputs, copy bodies are hard fusion and constant-fold
  barriers, the CPU interpreter reads and writes through
  `ShapeTracker::expr_idx`, typed Cast/Bitcast intermediates carry raw scalar
  storage instead of falling back to a plain f64 lane, and the runtime bridge
  routes full source-storage bytes by `buf_id` instead of truncating input slots
  to logical view length.
  MSL/WGSL/CUDA/HIP/OpenCL/GLSL share one ShapeTracker index renderer for
  flips, shrinks, permutes, broadcasts, masked/padded reads, and materializing
  copies; masked/padded reads emit guarded zero semantics. CPU materialization
  copies raw dtype bytes exactly, while shader copy bodies fail closed when a
  backend would narrow the copied dtype. Missing leaf storage and missing
  kernel input storage now raise immediately instead of silently zero-filling.
  Metal e2e proof covers materialization from flipped and padded views,
  same-storage/different-view binding slots sharing one device buffer, and
  raw `UInt16` shader copy preservation. CPU materialization has byte-exact
  coverage across every current dtype element width plus padded raw zero-fill,
  cross-renderer shader text covers non-float `UInt32` copy bodies, and the
  runtime bridge has CPU and Metal tests for one `buf_id` routed through
  distinct ShapeTracker view slots. `bench_primitives` now measures the copy
	  path: contiguous, flipped, shrunk, and padded CPU materialization are raw-copy
	  class; flipped single-view materialization uses a preflighted fixed-width
	  reverse-copy path for 1/2/4/8-byte elements. For non-MXFP storage, MLIR
	  `MaterializeCopy` now emits real flat memref arguments by binding slot and an
	  `scf.for` copy body with generated ShapeTracker index arithmetic plus guarded
	  zero-fill for masked/padded reads; coverage includes contiguous, flipped,
  shrunk, padded/masked, permuted, composed-view, expanded zero-stride,
  `UInt32`, and same-storage distinct slot cases. MLIR compute now emits real
  flat memref arguments, an `scf.for` elementwise loop, typed source loads,
  typed op SSA, and a final store for pure elementwise kernels; input
  ShapeTracker views reuse the MLIR index/mask lowerer, and masked reads load
	  only inside `scf.if` valid regions before yielding typed zeros. Coverage
	  includes flipped, padded/masked, same-storage distinct slots, composed views,
	  integer-vs-float comparison typing, constants, prior-op chains, and explicit
	  non-MXFP cast conversion selection across float, integer, unsigned, and bool
	  domains; the cast target dtype is first-class in `LazyOp::Cast`, the
	  scheduler output binding, `FusedOp::dst_dtype()`, and `molt_gpu_prim_cast`, CPU
		  execution uses typed scalar Cast/Bitcast values for terminal, fused
		  intermediate, and pre-reduce cases, and old untyped unary Cast/Bitcast
		  construction rejects immediately. Runtime tensor lifecycle now has typed raw
		  upload and typed zero-fill through `molt_gpu_prim_create_tensor_raw` and
		  `molt_gpu_prim_zeros_dtype`; MXFP upload remains fail-closed until the
		  block/exponent layout is explicit. Runtime readback has an explicit split:
	  the legacy f32 API rejects realized non-Float32 tensors, while
	  `molt_gpu_prim_dtype`, `molt_gpu_prim_nbytes`, and
	  `molt_gpu_prim_read_data_raw` provide fail-closed exact storage-byte readback
		  with dtype and capacity checks. Metal e2e coverage now byte-compares
	  Float32->Int32/UInt16/UInt8 Cast and equal-width Float32<->UInt32 Bitcast
	  against the CPU interpreter instead of decoding through f32. MLIR compute
	  now lowers domain-owned `ReduceSum`/`ReduceMax` with an outer output
	  `scf.for`, an inner reduction `scf.for`, dtype-correct accumulator
		  identities, `ReductionDomain`-derived row-major input indexing, pre-reduce
		  elementwise prefixes, and same-output-shape post-reduce suffixes. MLIR
		  serialization still fails closed for non-contiguous outputs, MXFP buffer
		  storage and `MaterializeCopy` until block/exponent storage lowering exists,
		  MXFP quantized casts, unsupported vector widths, invalid post-reduce
		  references to pre-reduce temporaries, and Bool `ReduceSum` until a widened
		  accumulator contract exists. Reductions now
	  carry explicit `ReductionDomain` metadata from `LazyOp::Reduce` through scheduling,
	  fusion, kernel hashing, CPU execution, MIL ranked value lowering, and shader
	  renderers.
		  CPU, MLIR, and shader lowering consume the domain's row-major input-index
		  mapping instead of inferring `input_numel / output_numel`, so non-last-axis
		  reductions are covered by CPU tests, MLIR loop tests, and MSL/WGSL/GLSL
		  affine-index render tests. MIL compute lowering now restores flat gathered
		  ShapeTracker reads to the domain input rank before applying
		  `reduce_sum`/`reduce_max` axes and returns the ranked domain output shape.
		  Shader renderers now also reduce the explicit `reduce_op.srcs()[0]`
		  instead of assuming the last pre-reduce temporary. `FusedOp` construction is
		  constructor-only with private op/src/dtype/domain fields and accessor reads,
		  blocking post-construction op/domain drift.
	  Fusion now treats post-reduce output-shape expansion as a
	  hard boundary until broadcast-after-reduce is a first-class IR primitive.
	  MIL
	  `MaterializeCopy` now has verified logical-view materialization for Bool,
  Int8/16/32, UInt8/16/32, Float16, and Float32 storage: contiguous views return
  the binding slot directly, while non-contiguous views emit `range_1d` int32
  indices, generated ShapeTracker index arithmetic, `gather`, and post-gather
  zero-fill `select` guarded by a safe gather index with dtype-correct zero
  literals. MIL compute read bindings remain Float32-only. MIL fails closed for
  BF16, 64-bit, and MXFP materialization and for ShapeTrackers whose element
  count, view constants, or physical offset span do not fit MIL int32 index
  tensors.
- WASM remains a supported target area, but same-contract parity with native is
  still incomplete.
- Luau is a checked source-emission target for the current/future Luau surface;
  current OpIR support is generated in
  `docs/spec/areas/compiler/luau_support_matrix.generated.md`.
- Backend-facing native and WASM lowering always runs through the TIR pipeline;
  the old environment-variable opt-out has been removed so SimpleIR transport
  metadata cannot bypass typed-IR validation.
- WASM `Auto` import retention is split by output form. Non-relocatable Auto
  registers the canonical import registry, records actual import lookups during
  code emission through `TrackedImportIds`, and validates serialized-module
  stripping before replacing bytes. Relocatable Auto keeps the conservative
  pre-emission dependency frontier for linker declarations, including
  `MOLT_WASM_EXTRA_REQUIRED_IMPORTS`; that knob no longer forces unused imports
  to survive non-reloc Auto stripping.
- The TIR RC drop-insertion substrate is implemented as a terminal drop phase
  (`runtime/molt-backend/src/tir/drop_phase.rs`) backed by
  representation-filtered liveness (`tir/passes/liveness.rs`) and
  `tir/passes/drop_insertion.rs`. It is active for LLVM, WASM, and Luau through
  `target_uses_tir_drop_insertion`; native Cranelift remains gated off until the
  loop-phi representation invariant is repaired and the competing native
  value-tracking RC path can be deleted as a second source of truth.
- Finalizer dispatch is implemented through the runtime `dec_ref_ptr` /
  `maybe_run_object_finalizer` authority and the committed finalizer matrix, but
  Python-visible finalizer ordering and standalone `__del__` exception-swallow
  semantics remain open ownership-boundary defects.
- Configurable runtime memory protection is supported and opt-in. A compiled
  binary caps its own memory through a single `ResourceLimits` enforcement path:
  the human-readable `MOLT_MEMORY_LIMIT` env (e.g. `64M`, `2G`) is an alias that
  normalizes into the same `max_memory` field as the manifest-emitted
  `MOLT_RESOURCE_MAX_MEMORY`, installed via the global tracker factory so worker
  threads inherit it. Enforcement is two-layer: the precise in-VM
  `LimitedTracker` (Layer 1, cross-target, deterministic, uncatchable
  `MemoryError`) plus an OS-level `RLIMIT_AS`/`RLIMIT_DATA` backstop (Layer 2,
  native; effective on Linux, best-effort on macOS, n/a on WASM). The
  capability-manifest per-operation result caps (`max_pow_result`,
  `max_repeat_result`, `max_shift_result`, `max_string_result`) now reach the
  Rust tracker without being dropped at the env boundary. Default is unchanged
  (no limit) unless the env is set; capability-tier default-on policy is
  deferred pending tier-vocabulary disambiguation. See `docs/RESOURCE_CONTROLS.md`.
- Test execution memory custody is mandatory. Direct pytest entrypoints are
  guarded before collection by root `sitecustomize.py`, the packaged
  `molt.pytest_memory_guard_bootstrap` pytest entry point, and the
  repo-configured `molt.pytest_memory_guard_config_plugin` fallback for disabled
  plugin autoload: unguarded pytest re-execs through `tools/memory_guard.py`,
  interpreter-option and programmatic `pytest.main()` launches use pytest's
  initial hook args as the re-exec authority, forged guard markers fail closed
  unless the live ancestor chain contains this repo's memory guard, and
  `--noconftest` / unsafe `--confcutdir` are rejected before tests can run.
  Shared harness custody is also mandatory: legacy `*_MEMORY_GUARD=0` env knobs
  are ignored rather than routing to raw `subprocess.run` or PTY execution. The
  tempfile-backed capture helper is also always guarded, so build/probe lanes
  that need file-backed stdout/stderr cannot bypass RSS custody. The
  repo process sentinel excludes ancestor plus Codex app/control-plane process
  groups from violation/drain kill sets while recording skipped protected groups
  in bounded JSONL diagnostics. Sentinel violation, drain, and stale-preflight
  events now include sampled process rows, external parent pids, resolved guard
  limits, and bounded repro context with cwd, safe env, pytest identity, guard
  process lineage, and sentinel label/argv where applicable. Cleanup JSON embeds
  parsed `sentinel_events` instead of leaving sentinel stdout as an unstructured
  side stream.
- Backend daemon custody is identity-based and centralized in
  `src/molt/backend_daemon_custody.py`. CLI startup writes `*.identity.json`
  sidecars with pid, socket path, project root, cargo profile, config digest,
  backend binary, and command snapshot; stale restart, stale cleanup,
  `tests/molt_diff.py`, `tools/bench.py`, `tools/bench_wasm.py`,
  `tools/bench_individual.py --isolate-daemon`, and request-timeout paths only
  signal after socket-health or process-command verification and revalidate
  before escalation. Native and WASM benchmark pruning now canonicalizes
  `MOLT_SESSION_ID` before cleanup and terminates only identity-verified
  current-session daemons, preserving concurrent warm daemon/cache state.
  Native and WASM benchmark builds also reuse Molt build caches by default and
  expose `--no-molt-build-cache` only for deliberate cold/no-cache studies.
  Shared exact-key stdlib cache artifacts are non-destructive on build/link/probe
  hot paths: contract mismatches skip reuse and republish under the per-entry
  lock instead of unlinking artifacts that another session may still be reading.
  Backend compile cache publication uses session-independent locks under the
  resolved cache root's `locks/` directory, while Cargo/backend rebuild locks
  are keyed by the mutable build-state root: default `MOLT_SESSION_ID` runs get
  isolated lock directories and explicit shared `CARGO_TARGET_DIR`/
  `MOLT_BUILD_STATE_DIR` runs share lock files. Persisted JSON/text/byte cache,
  diagnostics, deployment, validation, package/archive, vendor file, linker
  sidecar, and final file-artifact writers use unique atomic temp siblings plus
  replace; vendored directory tree replacement now prepares a hidden temp tree
  and preserves the previous tree for restore-on-failure, with OS-level
  directory exchange still tracked separately before universal tree-level
  atomicity can be claimed. WASM runtime
  Cargo rebuilds now accept only Cargo-reported `compiler-artifact` `.wasm`/`.a`
  outputs, preserve pre-existing shared artifacts, require `artifact_sha256`
  sidecars before hydrating candidate runtime bytes, and fail closed when Cargo
  reports no runtime artifact. Module-analysis cache identity includes
  `import_scan_mode` in both the filename key and schema payload.
  `tools/compile_progress.py` excludes backend daemons from its marker-scoped
  compiler-child cleanup. Legacy raw `*.pid` files are removable debris only,
  and `tools/verify_native_binary_valid.sh` no longer performs blanket daemon
  `pkill` because the gate builds daemon-off. `tools/check_subprocess_guard_coverage.py`
  now scans raw `os.kill` and shell kill strings in addition to subprocess
  calls, with backend daemon signals centralized in the custody module.
- Module-scope Python-visible names assigned through control-flow joins use the
  module object as their single mutable authority. The frontend prepares and
  evicts module-backed bindings for loops, `if`, `try`/`except`, and `try*`, so
  post-join loads lower to `MODULE_GET_ATTR` instead of reading branch-local SSA
  or boxed-cell shadows. Native regression coverage includes top-level
  `try/except` handler assignment read after the `try` and the import
  transaction package-entry fixture.
- Runtime import entrypoints now have one active importlib transaction
  intrinsic. The retired `molt_importlib_import_module` intrinsic, Rust export,
  generated stubs, and WASM import rows were deleted; `importlib.import_module`
  and `builtins.__import__` both route through
  `molt_importlib_import_transaction` for runtime imports. The empty
  `_MODULE_ALIASES` side table was deleted, and frontend literal/direct-call
  folding of `importlib.import_module("literal")` now emits the same
  `molt_importlib_import_transaction(name, None, None, ("*",), 0)` intrinsic
  used by the public shim whenever callable identity and an absolute literal
  name are statically stable; runtime import owns target availability,
  version-gated absence, module cache custody, provenance, fromlist behavior,
  and error shape. User rebinding of `importlib.import_module` still stays
  observable in compiled native code because the fold is disabled when the
  module attribute is syntactically rebound through `importlib` or an alias.

## Intentionally Unsupported

- Unrestricted dynamic execution (`exec`, `eval`, `compile`) in compiled binaries.
- Runtime monkeypatching as a compatibility mechanism.
- Unrestricted reflection that breaks AOT determinism and layout guarantees.
- Silent fallback to a host CPython runtime.

## Known Major Gaps / Blockers

- CPython coverage is incomplete across language, stdlib, and target-specific
  behavior.
- Native and WASM parity is still incomplete for several claimed surfaces.
- Luau parity is incomplete and must be extended through checked-build,
  static-analysis, and CPython-vs-Luau evidence rather than silent stub emission.
- Native RC ownership is not yet on the same TIR-drop authority as LLVM/WASM/Luau:
  `target_uses_tir_drop_insertion(TargetKind::NativeCranelift)` is still `false`
  because loop-carried block args can receive inconsistent raw/heap
  representations on drop-inserted phi paths. The completion criterion is the
  structural fix plus deletion of the legacy automatic temp-RC lane, not another
  compatibility gate.
- Exception handler lifetime is not yet represented as a first-class
  `ExceptionRegion` / `HandlerState` ownership boundary. The active Phase-1
  requirement is to release both creation refs and handler match refs at their
  real exception-event boundaries, not through a global SSA last-use heuristic.
- Non-escaping objects with `__del__` can still be dropped at SSA last read
  rather than the Python `del` or scope-exit boundary, and exceptions raised
  from a standalone inline `__del__` path must be isolated as unraisable instead
  of propagating out of the compiled frame.
- The runpy dynamic-lane expected failures list is currently empty because
  supported lanes moved to intrinsic support; governance for unsupported
  runpy dynamic execution remains documented rather than tracked through an
  active expected-failure entry.
- The current backend entry path still carries a stringly `SimpleIR` transport
  for legacy consumers, but scalar `fast_int` / `fast_float` / `type_hint`
  metadata is not backend-authoritative. The TIR-to-SimpleIR lowerer no longer
  accepts an external type-map channel, and opaque call returns refine only
  through structural TIR `return_type` metadata. TIR functions now own a
  persistent `value_types` map, and type refinement writes op-result facts back
  into that function-owned map; range/list devirtualization records the I64 and
  Bool facts it synthesizes for generated loop carriers and comparisons instead
  of leaving those facts solely in `_fast_int` attrs. TIR-to-SimpleIR value
  naming is now centralized in `SimpleValueNames`, keeping parameter identity
  and block-argument storage names on one reusable contract. TIR lift also
  records explicit single-output SimpleIR provenance so backends can map final
  LIR facts back to legacy names without trusting scalar transport hints.
  Backend scalar lowering consumes a final-codegen-time
  `ScalarRepresentationPlan` for semantic int/bool/float/str/None
  classifications. Native uses the plan for raw-primary carrier sets, scalar
  slot escape safety, scalar store-target discovery, and operation lane
  preference; raw-primary sets remain stricter carrier-safety subsets. Legacy
  WASM and Luau scalar fast paths now consume the same plan for
  integer-family arithmetic, comparison, truthiness, and index-key scalar
  decisions instead of trusting `fast_int`, `fast_float`, or scalar
  `type_hint` transport metadata.
  Generic container annotations now enter TIR as structured `TirType` facts
  (`list[T]`, `dict[K, V]`, `set[T]`, and fixed-arity `tuple[...]`) instead of
  remaining opaque string hints; malformed, dynamic, or unsupported compound
  hints stay `DynBox`.
  Backend semantic container dispatch now reads those facts through the shared
  representation plan for Luau, WASM import selection/emission, native
  `len`/`contains`, and LLVM `len`; `container_type` / `type_hint` strings
  alone no longer select those specialized paths. Semantic `list[int]` remains
  distinct from flat `list_int` storage proof, so direct storage optimizations
  now require a separate `ContainerStorageKind::FlatListInt` fact seeded by
  structural `list_int_new` producers and queried through the shared
  representation plan. `bce_safe` remains an independent bounds proof rather
  than storage authority.
- Native int-lane lowering now reads raw i64 values from the static
  `int_primary_vars` contract instead of a separate raw-int shadow transport.
  `int_primary_vars` is an exact-i64 representation contract, not a semantic
  `int` claim: bounded add/sub and raw-closed counted store/load loop carriers
  may enter raw-primary only after shared interval proof shows that the
  operation cannot overflow i64 or promote to BigInt. Unbounded arithmetic and
  shifts stay boxed/runtime-backed until a range/shift-count proof can show
  that the operation cannot overflow i64, promote to BigInt, or raise for
  Python shift semantics.
  Runtime integer shifts preserve the same contract directly: shift operands
  are strict integer/bool/BigInt values rather than exact-float or arbitrary
  `__index__` coercions, BigInt shift counts are not narrowed through fixed
  machine widths, huge right shifts saturate by operand sign, and left shifts
  validate allocation size before constructing wide BigInts. Native lowering
  calls those runtime shift primitives directly; raw Cranelift shift lowering
  requires a future explicit range and nonnegative shift-count proof.
  Native float-primary lowering likewise uses static `float_primary_vars` as
  the only authority for F64-primary Cranelift variables; the raw-f64 shadow
  lane has been removed, and non-primary float values are boxed immediately in
  their main I64 variables. Liveness cleanup and exception-check scrubbing are
  representation-aware: dead F64-primary slots are poisoned with an F64 zero,
  while boxed slots keep the boxed `None`/zero sentinel, so cleanup cannot
  violate Cranelift variable typing after raw-f64 shadow deletion. Native bool
  lowering now has a raw-closed
  `bool_primary_vars` subset for constants, alias/store propagation,
  comparisons, identity checks, and truthiness casts. Bool-primary escape
  boxing uses an explicit raw-bool `0/1` carrier conversion before NaN-boxing,
  so the b1-condition bool boxer is not used as a mixed raw/condition helper.
  Raw-closed bool join carriers use the same main-Variable raw `0/1` contract
  across store/load/copy and structured phi binding; join slots that are unsafe
  for scalar slot exclusion remain boxed. Proven-bool list indexing is admitted
  to bool-primary only when the index operand is raw-primary, so the inline
  list/list_bool codegen path can define raw `0/1` without conflating
  index-lowering lane selection with output representation. Unknown-list
  getitem truthiness now uses an explicit conditional list-bool carrier whose
  payload is raw `0/1` only on the runtime list_bool arm and otherwise remains
  the NaN-boxed element for the normal truthiness path. Scalar store-target
  discovery is shared across int, float, bool, and str lanes with the same
  all-sources rule; float-primary eligibility is definition-scoped, so
  unsupported producers such as `pow` keep their own outputs boxed without
  disabling unrelated proven-float locals in the same function. The raw-bool
  shadow lane has been removed: `bool_primary_vars` is the only raw-bool
  authority, and non-primary bools stay boxed in their main I64 variables.
  Native fixed-layout field stores now share a single direct-write proof for
  fresh stack and sized heap objects: `store_init` is direct for non-heap
  values, later `store` is direct only when the slot's prior direct write is
  known non-heap, and any unknown/control/escaping use drops the object from
  the direct-write set.
  Function-local loops cache same-module stable class bindings when the whole
  module proves that the class name is defined once, is not rebound or deleted,
  does not escape through `globals()`/`vars()`, and keeps a stable layout. The
  class reference is resolved once in the loop preheader and hot iterations load
  the cached local directly, removing the missing-sentinel branch and repeated
  constructor global lookup from proven-stable class loops.
  `CallArgs` builders own their argument slots independently; original argument
  temporaries are released only by normal liveness cleanup, and branch-splitting
  store paths must carry cleanup state through their merge blocks.
- Benchmark reporting and compatibility rollups are being simplified so they are
  generated from canonical evidence instead of maintained by hand in multiple docs.
- `molt-gpu` materializes `Contiguous` DAG operands through explicit copy
  kernels with fresh storage identity. The copy/materialization path now has
  repeatable benchmark evidence in `bench_primitives`: on 2026-06-11,
  `cargo bench -p molt-gpu --bench bench_primitives` measured `raw_copy_f32`
  at `53.70 us`, `materialize_contiguous_f32` at `64.68 us` (`1.20x` raw
  copy), `materialize_flip_f32` at `66.83 us` (`1.24x` raw copy),
  `materialize_flip_u8_4mb` at `66.52 us`, `materialize_flip_u16_4mb` at
  `66.43 us`, `materialize_flip_u32_4mb` at `66.60 us`,
  `materialize_flip_u64_4mb` at `66.46 us`, `materialize_shrink_f32` at
  `55.86 us` (`1.04x` raw copy), `materialize_pad_f32` at `55.26 us` (`1.03x`
  raw copy), and `same_storage_view_add_f32` at `8028.11 us` for roughly four
	  source megabytes. Non-MXFP MLIR now has positive materialization-copy lowering
	  proof for flat memrefs and ShapeTracker index/mask arithmetic. MIL has positive
  gather/select materialization proof with safe masked gather ordering and
  int32-domain guardrails, including physical offset span checks, for Bool,
  Int8/16/32, UInt8/16/32, Float16, and Float32 storage. MLIR now has positive
  pure-elementwise compute view-lowering proof for real memref loops,
  masked-safe loads, typed comparisons, constants, prior-op chains, and explicit
  non-MXFP cast conversion selection with lazy/scheduler/runtime target dtype
  custody plus CPU typed-scalar execution proof for terminal, intermediate, and
	  pre-reduce Cast/Bitcast values. Runtime raw upload/readback now exposes typed
	  storage-byte creation, typed zero-fill, dtype, logical storage byte count, and
	  exact realized storage-byte copy APIs; the old f32 readback remains
	  fail-closed for realized non-Float32 tensors. Metal
	  e2e proof now covers raw non-f32 Cast/Bitcast storage against CPU bytes.
	  Upstream tinygrad is now registered as a disabled-until-pinned friend-suite
	  benchmark lane (`tinygrad_off_the_shelf`) with CPython and Molt runners that
	  execute public API workloads through `tools/tinygrad_off_shelf_adapter.py`;
	  this is the compatibility/perf case study for compiling and profiling
	  unmodified tinygrad code. The friend-suite harness now records git source
	  custody, fails dirty or wrong-ref checkouts, accepts per-suite `--suite-root`
	  and `--repo-ref` overrides for pinned local clones, supports manifest-declared
	  runner names without a hidden allowlist, and ingests `json_stdout` workload
	  timings into structured runner metrics. The public tinygrad wrappers now
	  carry canonical dtype codes, byte tensors report `uint8`, explicit uint/int
	  constructors upload exact little-endian storage through
	  `molt_gpu_prim_create_tensor_raw`, typed zeros use
	  `molt_gpu_prim_zeros_dtype`, handle-only readback decodes
	  `molt_gpu_prim_read_data_raw` without f32 transit, and elementwise
	  unary/binary operations, ternary `where`, typed casts, explicit-axis
	  reductions, and Rust-owned all-axis reductions via
	  `molt_gpu_prim_reduce_all` carry runtime handles through the corresponding
	  GPU primitive intrinsics. The
	  tinygrad shim now keeps movement-family operations on runtime handles too:
	  `reshape`, `expand`, `permute`, zero-fill `pad`, `shrink`, `flip`, and
	  `contiguous` lower through GPU primitive intrinsics, and `matmul` composes
	  runtime-backed reshape/expand/binary/reduce/reshape instead of host
	  materialization. Root `Movement` realization is an explicit
	  `MaterializeCopy` boundary, and empty non-buffer pipelines fail closed
	  instead of fabricating zero tensors.
	  Module graph discovery now uses an explicit import scan mode:
	  entry/allowlisted modules keep full discovery, while transitive
	  dependencies use module-init closure so lazy function-body imports
	  (including upstream tinygrad backend/autogen families) stay runtime/device
	  obligations instead of compile-time graph bloat. Runtime-import support
	  detection follows the same split, graph/import-scan caches include the
	  scan policy and stdlib allowlist digest, and Darwin memory-guard sizing no
	  longer shells out for advisory available-memory data. Import graph
	  materialization now has one immutable `ImportPlan`: entry planning owns the
	  runtime-import support closure, while final materialization owns namespace
		  stubs, generated importer modules, known-module sets, allowlist snapshots,
		  and module graph metadata before frontend analysis or backend lowering can
		  observe the graph. Core stdlib closure honors the same nested-scan exception
		  set as regular stdlib discovery, so `collections` keeps its required
		  function-body `copy` import in the graph and native hello-world no longer
		  links against a missing `copy__copy` symbol. Shared stdlib cache identity now
		  seeds every explicit stdlib module init like backend DFE and requires a
		  backend-written partition manifest sidecar before reuse; the newest native
		  hello-world shared object defines `_copy__copy` / `_molt_init_copy` and has
		  no undefined `_copy__copy`.
			  Remaining GPU backend gaps are MIL BF16/64-bit/MXFP materialization proof,
			  MLIR MXFP block/exponent storage plus `MaterializeCopy` lowering, MLIR
			  MXFP quantized cast lowering, a first-class window/im2col primitive for
			  tinygrad convolution wrapper migration, and typed nonzero-pad semantics;
			  these lanes stay fail-closed rather than ignoring ShapeTracker
		  semantics.

## Validation Summary

- Canonical local DX now routes through:
  - `molt setup`
  - `molt doctor`
  - `molt validate`
- Backend completion now requires an explicit end-to-end CLI/profile/target
  matrix, not only backend-internal unit and lowering proof:
  - native `build` / `run` / `compare` on `dev` and `release`
  - LLVM release parity on the covered slice
  - linked-WASM CLI build plus Node execution
  - Luau checked emission, generated support-matrix freshness, runner
    availability, Rust backend/lowering regressions, and targeted
    CPython-vs-Luau parity smoke
  - conformance and benchmark entrypoints on the same CLI validation surface
  - honest failure surfaces for intentionally unsupported dynamic execution
- Compatibility evidence is tracked in the differential suites, generated
  compatibility docs, and proof workflows linked below.

## Compatibility Summary

<!-- GENERATED:compat-summary:start -->
- Stdlib lowering audit: `916` modules audited; `41` intrinsic-backed; `875` intrinsic-partial; `0` python-only.
- Platform availability metadata: `66` modules with explicit availability notes; `41` WASI-blocked; `37` Emscripten-blocked in CPython docs.
- Deep evidence: see the stdlib intrinsics audit and platform availability matrices under `docs/spec/areas/compat/surfaces/stdlib/`.
<!-- GENERATED:compat-summary:end -->

## Performance Summary

<!-- GENERATED:bench-summary:start -->
Latest run: 2026-05-23 (macOS arm64, CPython 3.12.13).
Top speedups: `bench_class_hierarchy.py` 6.94x, `bench_bytes_find_only.py` 6.27x, `bench_sum.py` 5.30x, `bench_bytes_find.py` 5.00x, `bench_gc_pressure.py` 1.32x.
Regressions: `bench_struct.py` 0.04x, `bench_exception_heavy.py` 0.55x, `bench_csv_parse_wide.py` 0.56x, `bench_etl_orders.py` 0.64x, `bench_parse_msgpack.py` 0.86x, `bench_csv_parse.py` 0.88x, `bench_tuple_slice.py` 0.93x, `bench_str_find.py` 0.95x, `bench_set_ops.py` 0.96x, `bench_try_except.py` 0.96x, `bench_descriptor_property.py` 0.98x, `bench_str_split.py` 0.98x, `bench_str_count_unicode.py` 0.98x, `bench_async_await.py` 0.99x, `bench_startup.py` 0.99x, `bench_bytearray_replace.py` 0.99x, `bench_str_startswith.py` 1.00x, `bench_bytes_replace.py` 1.00x.
Slowest: `bench_struct.py` 0.04x, `bench_exception_heavy.py` 0.55x, `bench_csv_parse_wide.py` 0.56x.
Molt build/run failures: none.
Comparator baseline coverage: PyPy baseline unavailable; Codon baseline unavailable; Nuitka baseline unavailable; Pyodide baseline unavailable.
WASM run: 2026-05-23 (macOS arm64, CPython 3.12.13); ok 53/56, failures: `bench_async_await.py`, `bench_channel_throughput.py`, `bench_ptr_registry.py`. Slowest: `bench_struct.py` 37.60s, `bench_gc_pressure.py` 3.94s, `bench_exception_heavy.py` 3.20s; largest sizes: `bench_channel_throughput.py` 21168.8 KB, `bench_async_await.py` 18310.4 KB, `bench_ptr_registry.py` 10415.5 KB; WASM vs CPython slowest ratios: `bench_struct.py` 376.52x, `bench_exception_heavy.py` 25.46x, `bench_deeply_nested_loop.py` 22.66x.
<!-- GENERATED:bench-summary:end -->

Focused post-summary recheck: `bench_ptr_registry.py` now builds and runs on the
current native path (`build_time_s=167.9641`, `molt_time_s=0.456952`, output
`100000`). Evidence is in
`bench/results/ptr_registry_repro_bench-ptr-registry-20260519T220445Z.json`.
The generated full-run failure list above predates this focused recheck and
should be regenerated on the next full benchmark refresh.

Focused native stale-failure recheck: the full generated failure list above now
builds and runs on the current native path after the attribute inline-cache
ownership fix: `bench_async_await.py`, `bench_channel_throughput.py`,
`bench_dict_comprehension.py`, `bench_import_time.py`,
`bench_parse_msgpack.py`, `bench_procedural_gen.py`, and
`bench_ptr_registry.py` all passed in a 7/7 focused run. Evidence is in
`bench/results/stale_failure_post_attr_ic_20260522T182839.json`. The generated
full benchmark summary should be regenerated on the next full benchmark refresh
to replace the stale failure list.

Focused JSON recheck: `bench_json_roundtrip.py` moved from `0.2109x` CPython
(`molt_time_s=0.108314`) to `3.1942x` CPython (`molt_time_s=0.007368`) after
the intrinsic parser switched to byte-indexed scanning and direct default
numeric construction. Evidence:
`bench/results/json_roundtrip_baseline_20260520.json` and
`bench/results/json_roundtrip_byte_parser_20260520.json`.

Focused Counter recheck: `bench_counter_words.py` moved from the generated
full-run `0.31x` CPython entry to `1.0341x` CPython on current `main` after
the compiler lowered exact `collections.Counter(list|tuple)` construction plus
exact Counter indexing/length to Rust intrinsics. The focused run preserved
output parity and recorded `git_rev=a5ccd8d5e`; evidence is in
`bench/results/counter_words_head_20260520.json`.

## Deep Links

- Compatibility architecture: [areas/compat/README.md](areas/compat/README.md)
- Language surface index: [areas/compat/surfaces/language/language_surface_matrix.md](areas/compat/surfaces/language/language_surface_matrix.md)
- Stdlib surface index: [areas/compat/surfaces/stdlib/stdlib_surface_index.md](areas/compat/surfaces/stdlib/stdlib_surface_index.md)
- Detailed benchmark report: [../benchmarks/bench_summary.md](../benchmarks/bench_summary.md)
- Standalone proof workflow: [../proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md](../proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md)
