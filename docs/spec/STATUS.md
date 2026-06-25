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
- Native Cranelift codegen decomposition is active at function-boundary
  granularity: `native_backend/function_compiler/fc/` owns extracted op-family
  handlers for scalar builtin runtime calls (`id`, `ord`, fused `ord_at`,
  `chr`), sequence/iterator lowering (`len`, range/tuple/unpack/iterator
  operations), dict mutation (`dict_set`, `dict_update_missing`), exception
  control (`raise`, `check_exception`), and value-custody transfer (`inc_ref`,
  `borrow`, `dec_ref`, `release`, `box`, `unbox`, `cast`, `widen`,
  `identity_alias`, `binding_alias`), plus runtime probes and side-effecting
  helper shims (`env_get`, `exception_pending`, `function_defaults_version`,
  `print`, `warn_stderr`, `print_newline`, `block_on`, `bridge_unavailable`).
  The remaining inline opcode shell is limited to residual constant/literal
  materialization and hoisting pending a fresh structural contract.
- Plain-local alias rebinding now lowers through the `binding_alias` owned-alias
  lane, with generated op-kind classifier tables and TIR ownership/representation
  analyses treating it as source bits plus an independent droppable reference.
- Standalone binary workflows are a first-class product requirement.
- Differential testing against CPython is a core validation path.
- Ordinary class construction keeps runtime `type.__call__` as the semantic
  authority whenever class analysis sees custom, inherited, builtin, dynamic, or
  opaque `__new__` resolution. The frontend static constructor allocation fold
  is eligible only for classes whose MRO proves default `object.__new__`.
  Runtime constructor policy covers default-object argument rejection,
  custom-`__new__` plus object-`__init__` skip behavior, and custom-init
  forwarding. Focused evidence lives in
  `tests/test_frontend_midend_passes.py`,
  `runtime/molt-runtime/src/call/type_policy.rs`, and
  `tests/differential/basic/type_call_constructor_policy.py`.
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
  modules. Translation validation uses the same resolver, probes the selected
  CPython command for an exact minor-version match, and passes
  `molt build --python-version` through the Molt run, so validation baselines
  cannot silently inherit `sys.executable`. Malformed or non-string
  target-version config fails closed instead of falling back to another target.
  The default target remains Python `3.12`.
- Rust-first stdlib lowering is the canonical direction, with generated audit
  surfaces under `docs/spec/areas/compat/surfaces/stdlib/`.
- `os.system` is intrinsic-backed through the subprocess process boundary,
  uses explicit platform shells (`cmd.exe /c` on `nt`, `/bin/sh -c` otherwise),
  and is tracked in the stdlib surface matrix with focused CPython differential
  coverage.
- `os.utime(ns=...)` routes through the same runtime `utime_at` intrinsic used
  for dir-fd-aware updates. Unix keeps `utimensat`; Windows now handles the
  `dir_fd=None, follow_symlinks=True` subset through `SetFileTime` using the
  pre-split `(sec, nsec)` payload while unsupported dir-fd/no-follow variants
  still fail closed.
- Runtime opaque handles are pointer-registry ids encoded as immediate ints
  through `opaque_handle_bits`; only Molt heap objects use pointer-tagged bits.
  Rust-backed stdlib/runtime handles for locks, async streams, process/socket
  state, select, decimal, fractions, ipaddress, contextlib, and graphlib no
  longer expose `bits_from_ptr(Box::into_raw(...))` to Python refcount traffic.
- Runtime intrinsic metadata and resolver generation now have separate generated
  source surfaces: `runtime/molt-runtime/src/intrinsics/generated.rs` remains the
  canonical `INTRINSICS` manifest table consumed by frontend/WASM tooling, while
  `runtime/molt-runtime/src/intrinsics/generated_resolvers/` contains
  per-category resolver modules generated by `tools/gen_intrinsics.py` under
  `MOLT_GENERATOR` custody. `molt-runtime-stringprep` now owns the first
  generated per-leaf intrinsic sub-registry, with the facade resolver delegating
  through that leaf; the remaining throughput work is to move the other
  generated categories the same way.
- Runtime text leaf ownership is now active for `html` and `unicodedata`: the
  in-facade duplicate modules are deleted, `molt_html_*` and
  `molt_unicodedata_*` resolver arms are gated by the dep-backed `stdlib_text`
  feature, and the runtime profile-availability gate refuses micro-profile
  imports before link rather than relying on fallback symbols.
- Runtime zoneinfo leaf ownership is now active: the in-facade duplicate module
  is deleted, `molt_zoneinfo_*` resolver arms are gated by the dep-backed
  `stdlib_zoneinfo` feature, and the same profile-availability gate now refuses
  micro-profile `zoneinfo` imports before link.
- Native networking currently claims the Unix-family native socket ABI and the
  WASM host socket ABI only. Windows native builds route requested `stdlib_net`
  symbols through the explicit no-net intrinsic surface until the WinSock
  constants, sockaddr storage, resolver, SSL socket ownership, and async poller
  contracts land together.
- Build-time import graph discovery now separates external-root resolution from
  external-package admission. `MOLT_MODULE_ROOTS`, `--lib-path`, respected
  `PYTHONPATH`, and auto site-packages can make a package resolvable, but
  transitive external package closure is admitted only by direct entry imports
  or an explicit `MOLT_EXTERNAL_STATIC_PACKAGES` package declaration. The graph
  cache key includes this policy. Explicitly admitted external packages now
  scan package-local `.so`/`.pyd` artifacts during build admission, require a
  nearby `extension_manifest.json` with matching module, extension path,
  extension SHA-256, ABI, target triple, platform tag, and capabilities, and
  fingerprint the artifact/manifest custody facts in graph, wrapper build, and
  backend object-cache inputs. Native builds publish the validated artifact,
  sidecar, package `__init__.py` chain, and runtime extension shim candidates
  into a deterministic `external_static_packages/<plan-digest>/` runtime root;
  generated native binaries inject that staged root into canonical
  `MOLT_MODULE_ROOTS` before runtime startup, while target modes without a
  runtime-custody consumer fail closed. The final native link fingerprint hashes
  those staged bytes without treating runtime-loaded extensions as linker
  inputs, and fingerprint-write failures surface as build warnings in both text
  and JSON output. Repo-owned roots keep internal
  custody even when duplicated by respected `PYTHONPATH` (for example
  `PYTHONPATH=src`), so full-profile stdlib/importlib builds retain transitive
  runtime imports such as `abc` and `copy` instead of discovering missing
  modules at link or runtime.
  Frontend module analysis now records path-backed `ModuleSourceLease` metadata
  and releases source text/ASTs after dependency/default extraction; serial and
  parallel lowering borrow source through validated leases, and worker payloads
  carry path/stat custody rather than the full source string. This moves the
  off-the-shelf tinygrad/static-package path away from retaining the whole
  upstream graph through compile while still failing closed if a source file
  changes between analysis and lowering.
  Backend dispatch now mirrors that custody shape: daemon requests use `ir_path`
  leases and one-shot backend compiles pass `--ir-file`, with JSON IR streamed
  directly into `tmp/backend-ir-leases/` rather than materializing a second full
  byte buffer for stdin.
  Backend IR preparation now rejects direct calls to module-owned symbols whose
  modules are missing from the materialized graph before codegen and reports the
  originating function/op index, turning graph-closure drift into a fast
  build-time error while preserving lazy `MODULE_IMPORT` runtime boundaries.
  Frontend known-module checks no longer treat a known top-level package as
  authorization for every dotted child.
  Module path resolution is exact-case even on case-insensitive filesystems, so
  `pkg.Tensor` cannot resolve through `pkg/tensor.py`. Frontend fromlist
  lowering now prepares only graph-proven child modules as a runtime side effect
  and always reads the final binding through `MODULE_IMPORT_FROM`, preserving
  package exports over same-named child modules.
- The `molt_async_sleep` intrinsic now owns the public two-argument sleep-future
  constructor symbol directly. The one-argument internal poll callback is
  `molt_async_sleep_poll`; the legacy `molt_async_sleep_new` name/symbol bridge
  and backend override table are removed.
- Direct-call default filling observes live function metadata when the callee
  object is reachable: module direct calls and guarded function-object calls
  read `__defaults__` / `__kwdefaults__` for literal and dynamic default specs
  instead of baking sema-time literals. Constructor and method call paths rely
  on the same live metadata through direct padding or the runtime binder.
- Runtime intrinsic manifest defaults now participate in that same metadata
  contract. `tools/gen_intrinsics.py` parses supported concrete trailing
  defaults from `runtime/molt-runtime/src/intrinsics/manifest.pyi`, emits
  `IntrinsicSpec.defaults`, and runtime eager/lazy registration attaches
  matching `__defaults__` tuples. `operator.length_hint(obj, default=0)` is the
  first default-bearing intrinsic and preserves CPython precedence: the default
  is validated/normalized through the integer-index protocol before any length
  lookup, `__len__` wins over `__length_hint__`, and the normalized default is
  used when neither length surface applies or `__length_hint__` itself raises
  `TypeError` or a `TypeError` subclass; non-`TypeError` failures and invalid
  non-int hint returns propagate. Runtime exception matching for this fallback
  now routes through the shared exception matcher used by C-API error matching
  and attribute-error clearing.
- Deforestation fusion eligibility is generated from
  `runtime/molt-tir/src/tir/op_kinds.toml` as the exhaustive
  `fusion_barrier_opcodes` classifier. The classifier is intentionally distinct
  from side-effecting/may-throw facts: fusion preserves per-element evaluation
  order, so allocation, attribute reads, indexing, and arithmetic that may throw
  are not barriers unless they alter cross-iteration/control state or suspend.
- TIR effect classification is generated from `may_throw`, `side_effecting`,
  and `purity` rows in `runtime/molt-tir/src/tir/op_kinds.toml`. The generator
  emits exhaustive `opcode_may_throw_table`, `opcode_is_side_effecting_table`,
  generated `ALL_OPCODES`, and typed `opcode_effects_table` facts, so
  `effects.rs` no longer carries a pass-local opcode classifier.
- Deferred refcount heap exposure is generated from
  `refcount_heap_exposure_opcodes` as the exhaustive
  `opcode_is_refcount_heap_exposure_table` classifier. The classifier is
  intentionally distinct from alias heap barriers: it answers whether operands
  become heap/external roots for deferred RC, not whether an op creates a generic
  heap memory definition.
- Escape-analysis allocation roots are generated from
  `escape_alloc_site_opcodes` as the exhaustive
  `opcode_is_escape_alloc_site_table` classifier. The classifier is intentionally
  distinct from refcount heap exposure: it answers whether an opcode result is a
  fresh allocation root whose escape state should be tracked.
- Raw-i64 division-family exception custody is also registry-owned:
  `i64_zero_divisor_guard_opcodes` generates the exhaustive
  `opcode_requires_i64_zero_divisor_guard_table` classifier consumed by LIR
  lowering and `check_exception_elim`, so boxed-dispatch retention and
  nonzero-divisor exception elimination share one authority for `div`,
  `floordiv`, and `mod`.
- Raw-i64 machine-shift count safety is registry-owned:
  `i64_shift_count_guard_opcodes` generates the exhaustive
  `opcode_requires_i64_shift_count_guard_table` classifier consumed by LICM's
  throw-condition proof, so shift hoisting reuses one authority for the `[0, 63]`
  count proof requirement.
- Boxed augmented-assignment runtime dispatch is registry-owned:
  `boxed_runtime_inplace_dispatch_opcodes` generates
  `opcode_uses_boxed_runtime_inplace_dispatch_table`, consumed by LLVM lowering
  when first-class `InplaceAdd`/`InplaceSub`/`InplaceMul` reach the boxed slow
  path. That path calls `molt_inplace_*` so `__i<op>__` is tried before the
  binary/reflected dunder chain; preserved-Copy `inplace_*` spellings remain
  carried by `_original_kind`.
- GVN numbering policy is registry-owned:
  `gvn_always_numberable_opcodes`, `gvn_type_gated_numberable_opcodes`, and
  `gvn_value_keyed_constant_opcodes` plus `gvn_numberable_attr_key_opcodes`
  generate the exhaustive
  `opcode_gvn_numbering_role_table` and
  `opcode_gvn_value_key_spec_table`, so unconditional value transforms,
  primitive-gated computations, same-block literal payload keys, and
  attr-sensitive numbered ops cannot drift as private pass-local opcode or
  attribute lists.
- Type-refine result-type membership is registry-owned:
  `type_refine_attr_result_type_rules` and
  `type_refine_operand_type_rules` generate exhaustive rule tables for
  attr-derived class/call/guard/Copy-original-kind facts and operand-dependent
  arithmetic, boolean, bitwise, iterator, indexing, tuple, Copy, BoxVal, and
  UnboxVal inference. `type_refine.rs` owns only the rule semantics and live
  operand/attribute parsing, not opcode membership.
- Operand-independent result-type facts are registry-owned through
  `operand_independent_result_type` rows and generated
  `opcode_operand_independent_result_tir_type`. Type refine, block versioning,
  branchless counting, fast-math type seeding, strength-reduction type seeding,
  and GVN consume that helper instead of carrying private
  Const*/comparison/module-result opcode matches.
- Call graph and CallFacts dispatch are registry-owned:
  `call_opcode_roles` generates the exhaustive `opcode_call_role_table` for
  first-class Call/CallMethod/CallBuiltin/Copy behavior, and
  `call_graph_user_call_kinds` generates
  `simpleir_kind_is_call_graph_user_call` for Copy `_original_kind` fallbacks.
  `call_graph.rs` and `call_facts.rs` own target resolution, GPU runtime-symbol
  carve-outs, builtin no-throw proof, and fact lattice semantics, not private
  call opcode or call-kind sets.
- SCCP constant-folding membership is registry-owned:
  `sccp_constant_seed_rules` and `sccp_constant_eval_rules` generate exhaustive
  rule tables for lattice seeding and constant evaluation. `sccp.rs` owns the
  rule semantics, including attr parsing, overflow refusal, Python numeric
  error behavior, compound-size caps, and tuple-as-list lattice representation,
  not private opcode lists.
- Value-range integer rule membership is registry-owned:
  `value_range_transfer_rules`, `value_range_const_fold_rules`, and
  `value_range_cond_narrow_rules`, and `value_range_container_length_rules`
  generate exhaustive rule tables for modeled interval transfer functions,
  checked integer constant folding used by constant-mask/container-length
  derivation, loop-guard true-edge upper-bound narrowing, and fixed
  literal/list-repeat/`len(...)` length facts. `value_range.rs` owns the
  interval formulas, saturation, Python shift/mod semantics, CFG polarity
  checks, symbolic-len recording, builtin-name and operand-shape validation,
  copy resolution, and raw-lane soundness boundary, not private opcode lists.
- Range-loop devirtualization pattern roles are registry-owned:
  `range_devirt_roles` generates the exhaustive
  `opcode_range_devirt_role_table` for CallBuiltin/GetIter/IterNextUnboxed
  roles in the `range(...)` iterator pattern. `range_devirt.rs` still owns
  builtin-name checks, operand/result shape, loop-header role, dominance, and
  CFG validation.
- Polyhedral loop classification is registry-owned:
  `polyhedral_loop_header_opcodes` generates
  `opcode_is_polyhedral_loop_header_table`, and
  `polyhedral_affine_body_opcodes` generates
  `opcode_is_polyhedral_affine_body_table`. `polyhedral.rs` owns loop-body
  traversal, tiling annotation, and live Copy refinement, not private
  loop-header or affine-body opcode sets.
- SSA attr transport is registry-owned:
  `ssa_s_value_attr_keys` generates the exhaustive
  `opcode_ssa_s_value_attr_key_table`, and
  `ssa_original_kind_preserving_kinds` generates
  `simpleir_kind_preserves_original_kind_for_ssa`. `ssa.rs` owns live operand
  resolution and attr insertion, not private opcode/string sets for string
  payload routing or mapped `_original_kind` preservation.
- Vectorization opcode classification is registry-owned:
  `vectorize_opcode_facts` generates the exhaustive
  `opcode_vectorize_facts_table` consumed by `vectorize.rs`. The pass owns
  accumulator recognition, live Copy refinement, min/max pattern validation,
  lane typing, and hint emission, not private body-eligibility, loop-header,
  annotation-target, or reduction opcode lists.
- Representation-aware LIR verifier dispatch is registry-owned:
  `lir_verify_rules` generates the exhaustive `opcode_lir_verify_rule_table`
  consumed by `verify_lir.rs`. The verifier owns hook invariants and
  diagnostics, not private BoxVal/UnboxVal/arithmetic/CallBuiltin dispatch.
- TIR pass fuzzing generation shape is registry-owned:
  `fuzz_tir_opcode_shapes` generates `FUZZ_TIR_OPCODE_SHAPES` and
  `opcode_fuzz_tir_operand_count_table` plus
  `opcode_fuzz_tir_attr_payload_rule_table` for
  `runtime/molt-backend/fuzz/fuzz_targets/fuzz_tir_passes.rs`. The fuzzer uses
  that generated palette for operand counts and synthetic constant payload
  generation, while the canonical `opcode_fixed_result_count_table` owns result
  counts; variable-result opcodes such as `Copy` stay out of the fixed-shape
  palette.
- Drop-insertion suspension retain points are registry-owned:
  `drop_insertion_suspension_point_opcodes` generates the exhaustive
  `opcode_is_drop_insertion_suspension_point_table` consumed by
  `drop_insertion.rs`, keeping coroutine-frame retain placement distinct from
  broader state-machine transform legality and fusion barriers.
- Drop-insertion return-boundary deferral barriers are registry-owned:
  `drop_insertion_return_deferral_barrier_opcodes` generates the exhaustive
  `opcode_is_drop_insertion_return_deferral_barrier_table`, so explicit
  IncRef/DecRef/Free rails define one authority for finalizer-sensitive roots
  that cannot be extended to return cleanup.
- Literal payload facts are registry-owned through `literal_payload_opcodes` and
  shared by canonicalization and exception-check elimination. Commutative
  reordering, comparison swaps, and ordered binary algebraic folds use adjacent
  generated canonicalize tables. `canonicalize.rs` owns only live operand/type
  checks and rewrites, not private opcode lists.
- Exception label metadata is registry-owned: `exception_label_attr_opcodes`
  identifies ops whose `value` attr carries a SimpleIR exception label id, and
  `exception_transfer_edge_opcodes` is the generated subset that contributes
  implicit CFG transfer edges. Inliner, generator fusion, lower-to-simple, and
  dominator construction consume those generated tables. Lexical try-region
  nesting is a separate generated role fact: `exception_region_nesting_roles`
  feeds `opcode_exception_region_nesting_role_table` for TryStart/TryEnd
  Enter/Exit roles, while DCE and SCCP still own try-depth traversal and their
  conservative may-throw dead-op / constant-folding policies.
- Generator-fusion opcode roles are registry-owned:
  `generator_fusion_poll_required_yield_opcodes` and
  `generator_fusion_poll_reject_opcodes` generate the poll-eligibility role
  table, while `generator_fusion_iter_use_roles` generates the IterNext/Is
  iterator-use scanner role table. `generator_fusion.rs` owns operand-position,
  terminator-use, and CFG proof.
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
- Luau source emission now participates in the shared TIR module phase. The
  backend lifts the source-emission `SimpleIR` once to a `TirModule`, runs every
  local function through the per-function TIR pipeline, then runs
  `run_module_pipeline` (E1 inliner, generator fusion, module-slot promotion,
  terminal DropInsertion) before fail-closed back-conversion. Guarded evidence:
  `luau_tir_module_pipeline_inlines_direct_local_calls`.
- Luau `checked_add` and `checked_mul` are implemented through explicit f64
  helper contracts. `molt_checked_i64_add(a, b)` returns `(a + b, false)`,
  preserving Luau's existing number model while avoiding target-gating the
  portable TIR `CheckedAdd` transform. `molt_checked_i64_mul(a, b)` returns a
  conservative overflow/inexactness flag when the product reaches the f64
  exact-integer boundary, forcing the boxed BigInt slow loop instead of
  silently accepting a rounded product. Guarded evidence:
  `test_compile_checked_lowers_checked_add_helper` and
  `test_compile_checked_lowers_checked_mul_helper`; generated matrix statuses:
  `implemented-exact`.
- Luau `matmul` and `inplace_matmul` now lower through checked descriptor
  helpers instead of unsupported-output stubs. `molt_matmul` dispatches
  `__matmul__` / `__rmatmul__`; `molt_inplace_matmul` tries `__imatmul__`
  first and falls back to the same binary protocol with the `@=` TypeError
  spelling. Guarded evidence:
  `test_compile_checked_lowers_matmul_dunder_dispatch`,
  `test_compile_checked_lowers_inplace_matmul_dunder_dispatch`, and generated
  matrix statuses `implemented-exact`.
- Native, WASM, LLVM, and Luau backend-facing lowering now run through the TIR
  pipeline; the old environment-variable opt-out has been removed so SimpleIR
  transport metadata cannot bypass typed-IR validation.
- Frontend midend fixed-point verification fails closed: non-convergence and
  post-convergence idempotence drift record policy diagnostics and raise instead
  of accepting the last verified round or probe output behind an env-controlled
  policy switch. Each canonicalization round now runs a bounded CSE/post-CSE-DCE
  closure before convergence is measured; cap exhaustion is a non-convergence
  failure, so CSE-created dead pure definitions cannot leak into a follow-on
  proof round.
- WASM `Auto` import retention is split by output form. Non-relocatable Auto
  registers the canonical import registry, records actual import lookups during
  code emission through `TrackedImportIds`, and validates serialized-module
  stripping before replacing bytes. Relocatable Auto keeps the conservative
  pre-emission dependency frontier for linker declarations, including
  `MOLT_WASM_EXTRA_REQUIRED_IMPORTS`; that knob no longer forces unused imports
  to survive non-reloc Auto stripping.
- The TIR RC drop-insertion substrate is implemented as a terminal drop phase
  (`runtime/molt-tir/src/tir/drop_phase.rs`) backed by
  representation-filtered liveness (`tir/passes/liveness.rs`) and
  `tir/passes/drop_insertion.rs`. It is active for LLVM, WASM, Luau, and native
  Cranelift for the proven shared-drop and ExceptionRegion slices through the
  shared TIR authority. `PassStats` now records metadata-only authority changes
  through `attrs_changed`, so zero-physical-drop `drop_inserted` functions are
  not restored to stale SimpleIR without the marker. WASM runtime parity, broader
  RC/finalizer balance validation, and deletion of any stale native
  value-tracking assumptions that no longer own release placement remain the
  convergence work before this can be treated as a global RC ownership claim.
- Finalizer dispatch is implemented through the runtime `dec_ref_ptr` /
  `maybe_run_object_finalizer` authority and the committed finalizer matrix.
  Runtime execution records class-MRO finalizer sensitivity on class and
  instance headers in unit-level coverage, so non-finalizer objects avoid
  dying-instance `__del__` lookup. The native scope-exit ordering differential,
  unit-level direct-field/runtime finalizer guards, shared `DeleteVar`
  old-slot release boundary, and frontend `bound_local` carrier for
  list/tuple/dict/set/frozenset absorbing constructors are green. The
  2026-06-15 focused native differential shard now passes
  `finalizer_scope_exit_ordering.py`, `finalizer_object_attr_release.py`,
  `finalizer_matrix.py`, `finalizer_container_clear.py`, and
  `finalizer_standalone_raise_swallow.py`
  (`tmp/diff/finalizer_reaudit_after_borrowed_self.json`,
  `logs/finalizer_reaudit_after_borrowed_self.log`) after type-call dispatch
  stopped handing compiled `__init__` a second synthetic owner for borrowed
  `self`. Backend/profile parity and stale value-tracking deletion remain active
  finalizer blockers before Molt can claim complete finalizer parity.
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
  `--noconftest`, unsafe `--confcutdir`, unsafe pytest `-c`, memory-guard
  plugin disabling through argv or `PYTEST_ADDOPTS`, and autoload-disabled runs
  without the explicit repo guard config plugin are rejected before tests can
  run. Direct `tests/**.py` scripts and `python -m tests.*` module launches now
  share the same fail-closed re-exec path through path-local `tests/*/sitecustomize.py`
  routers plus project `src/sitecustomize.py` for `uv run`/editable project
  interpreters, keeping differential corpus directories free of harness files.
  Re-execed pytest and parent-side harness/standalone test commands install canonical
  `MOLT_PYTEST_CURRENT_TEST_FILE` custody under `tmp/pytest-memory-guard/`;
  serial pytest writes the bounded active-node JSON there, while xdist workers
  write bounded per-worker sidecars under the aggregate file's `.d/` sibling.
  Parent-side guard incidents reject noncanonical current-test paths, include
  all bounded worker records, and mark a worker record when the violating pid's
  sampled lineage proves it. Incident repro payloads also include bounded
  Claude/Codex/app-server/renderer/node-repl control-plane PGID/process samples so a
  parent host crash can be correlated with the guarded command without
  unbounded artifact clutter.
  Shared harness custody is also mandatory: legacy `*_MEMORY_GUARD=0` env knobs
  are ignored rather than routing to raw `subprocess.run` or PTY execution. The
  tempfile-backed capture helper is a byte-mode adapter over the same
  `memory_guard.run_guarded` authority, so build/probe lanes that need
  file-backed stdout/stderr carry the same RSS custody, repro diagnostics,
  guarded-command profile payload, and Cargo incremental quarantine receipt as
  pipe/text callers. Automatic repo process sentinels scope violation/drain kill
  sets to the guarded current process tree, exclude ancestor plus Claude/Codex
  app/control-plane process groups, protect external Claude/Codex-descendant
  process groups that are not owned by the current guard, and record skipped
  protected groups in bounded JSONL diagnostics. The raw low-level and
	  `tools/process_sentinel.py` process-group terminators now re-sample protected
	  ancestor/Claude/Codex PGIDs immediately before TERM and fallback KILL
	  signaling, so stale scanner state cannot turn into a
	  Claude/Codex/control-plane kill. The low-level `tools/memory_guard.py`
	  timeout/orphan cleanup now snapshots live PGIDs before launch, runs the direct
	  PID-lineage kill first, then drains only post-baseline orphaned process groups
	  still proven by the guard's tracker and identity snapshots. This closes the
	  tracked reparented `molt.cli build` / `molt-backend` escape path without
	  broad repo drains or Claude/Codex collateral; unsampled daemonized children
	  remain the responsibility of the explicit repo sentinel. Timeout summary JSON
	  lists the cleaned process groups when that second-stage drain fires. The guard now writes a `status: "running"`
  summary before child launch, including repro command, resolved limits, guard
  process identity, and bounded host/control-plane samples; if a long-running
  guard parent is killed before normal finalization, the requested summary path
  still contains repro context instead of disappearing. SIGTERM/SIGINT/SIGHUP
  delivered to the guard parent are caught long enough to terminate the child
  tree and rewrite the summary with `incident.reason = "guard_interrupted"` and
  `guard_signal`. The out-of-band sentinel's `--until-clean-sec`
  mode performs a final clean-window scan and processes any newly observed
  delayed launch before returning, while stale preflight ignores orphaned host
  processes that lack Molt/repo command identity even when a numeric PGID was
  reused. `tools/check_memory_guard_wiring.py` now consumes
  `tools/check_subprocess_guard_coverage.py`; the subprocess audit is clean and
  the wiring audit now fails closed on any future unexpected raw launcher,
  stale allowlist entry, or expanded allowlist count. `tools/dx_build_timer.py` has been
  migrated to the shared `MOLT_DX_BUILD` guard for Cargo build timing and
  version probes. `tools/cold_start_decompose.py` routes safe-run, no-op C
  compile, dyld timing, and Molt-probe build subprocesses through one
  `MOLT_COLD_START` guard family. `tools/gen_intrinsics.py` emits
  rustfmt-stable generated Rust, skips exact-content no-op writes before
  invoking rustfmt, lazy-loads memory-guard formatting custody only when a
  changed Rust file needs formatting, and routes changed-file formatting through
  `MOLT_GENERATOR` custody. `tools/perf_inner_repeat.py` self-tests and
  perf-scoreboard inner-repeat proof children now run through `MOLT_BENCH` /
  `MOLT_TEST` custody instead of local raw `subprocess.run` launchers.
  `tools/perf_scoreboard.py` now routes `safe_run.py --json` workload timing
  children and Codon build children through `MOLT_BENCH` custody, and collapses
  pgrep/sysctl/ps/pmset/git/version checks onto one bounded `_metadata_probe`
  raw metadata authority. Interactive `/usr/bin/sample` profiling children now
  enter one `_profiling_popen` helper with `MOLT_BENCH` process-group custody
  and shared force-close cleanup instead of scattered raw `Popen` calls.
  `tools/molt_dev.py` now routes git/interpreter probes, live manifest gates,
  toolchain marker probes, worktree cleanup, and difftest byte captures through
  shared guard helpers; detached daemon execution uses fork/exec/wait custody
  instead of `subprocess.call`, publishes `cmd.json`/`sid`/`pid`/`rc` state
  files atomically, and leaves `probe_pid` as the only allowlisted low-level
  pid liveness primitive.
  `.github/workflows/kani.yml` now runs Kani install/setup and both proof lanes
  through `tools/guarded_exec.py` with `MOLT_TEST_SUITE` custody.
  Nightly and security-hardening workflows also guard cargo-deny/cargo-audit
  install/check commands and direct Quint proof invocations through
  `tools/guarded_exec.py`.
  Explicit
  stale-preflight and `tools/process_sentinel.py` operator cleanup remain
  repo-scoped by command. Sentinel violation, drain, and stale-preflight events
  now include sampled process rows, external parent pids, resolved guard limits,
  kill scope, claim status, victim attribution, truthful killer-or-observer
  attribution, `termination.attempted`, SIGTERM/SIGKILL metadata, and bounded
  repro context with cwd, safe env, pytest identity, guard process lineage, and
  sentinel label/argv where applicable. Cleanup JSON embeds
  parsed `sentinel_events` instead of leaving sentinel stdout as an unstructured
  side stream.
- Backend daemon custody is identity-based and centralized in
  `src/molt/backend_daemon_custody.py`. CLI startup writes `*.identity.json`
  sidecars with pid, socket path, project root, cargo profile, config digest,
  backend binary, and command snapshot; stale restart, stale cleanup,
  `tests/molt_diff.py`, `tools/bench.py`, `tools/bench_wasm.py`,
  `tools/bench_individual.py --isolate-daemon`, and request-timeout paths only
  signal after socket-health or process-command verification and revalidate
  before escalation. Native, WASM, and differential benchmark pruning now
  canonicalizes `MOLT_SESSION_ID` before cleanup and terminates only
  identity-verified current-session daemons, preserving concurrent warm
  daemon/cache state.
  Native and WASM benchmark builds also reuse Molt build caches by default and
  expose `--no-molt-build-cache` only for deliberate cold/no-cache studies.
  Differential runs use the persistent diff cache root as the default
  `MOLT_CACHE` when no explicit cache is configured, while per-test output and
  temp roots remain ephemeral; `tests/molt_diff.py --stdlib-profile` forwards
  full/micro stdlib selection without env-only setup.
  Shared exact-key stdlib cache artifacts are non-destructive on build/link/probe
  hot paths: contract mismatches skip reuse and republish under the per-entry
  lock instead of unlinking artifacts that another session may still be reading.
  The shared stdlib cache key includes the sorted
  `stdlib_module_symbols` partition authority, so a changed module-symbol set
  cannot reuse a stale shared object even when function bodies and target inputs
  otherwise match.
  `MOLT_STDLIB_MODULE_SYMBOLS` has one backend parser authority and malformed
  values fail closed instead of falling back to heuristic shared-stdlib
  partitioning.
  TIR `ExceptionRegions` diagnostics now fail closed at the pass-manager
  verification boundary, so missing, ambiguous, or too-early handler-match-ref
  release facts cannot silently flow into backend lowering. Shared drop
  insertion consumes CreationRefs at the `raise` boundary and MatchRefs after
  the owning `exception_pop` by materializing ordinary TIR `DecRef` ops before
  the conservative handler-CFG bail, and native Cranelift participates in that
  same shared drop path. The old native-only CreationRef lifetime carve-out and
  exception-pop side path are deleted. Backend consumption evidence now covers
  LLVM lowering order, WASM host-EH/native-EH import behavior plus the LIR
  `dec_ref` runtime-call lane, and Luau checked lowering of shared drop
  artifacts as GC no-ops after the Luau target-info terminal drop phase. Luau
  and LLVM now also have executed runtime proof for
  `tests/differential/memory/exception_raise_catch_loop_leak.py`, whose
  generated artifacts print `500000` under `luau` and `--target llvm --release`.
  Validator fail-closed
  coverage now includes missing-pop, ambiguous-depth, path-alternative pop, loop
  re-entry close-boundary, shared `exception_pop` splitting with block-arg
  payloads, malformed Luau block structure, and terminal
  drop-pipeline diagnostics. The prior generated-WASM structural validation
  blocker is fixed: `importlib_import_transaction` now registers the same
  five-argument ABI consumed by its callable wrapper, and the linked artifact
  validates. The JS harness host map now includes the process host ABI imports
  needed by the linked runtime, including `env::molt_process_terminate_host`,
  and the WASM leak-loop differential now passes for
  `tests/differential/memory/exception_raise_catch_loop_leak.py`. Broader WASM
  `HandlerState` parity and authoritative `bench_exception_heavy` speed evidence
  still remain open. The 2026-06-20 direct release-fast WASM backend replay of
  the saved full-stdlib handoff IR also closes the false-owner/RSS cliff in the
  proven ExceptionRegions slice: implicit `TryStart`/`CheckException` transfers
  create handler-owned state only when the target label is active in the lexical
  exception frame stack, so inactive universal handler targets remain
  depth-zero observers. The formerly fatal `_collections_abc__Sequence_index`
  analysis now reports zero MatchRef release facts at 259.1 MiB, and the
  guarded replay exits 0 under the same 12 GiB process cap with a 0.324 GiB
  rusage peak.
  The 2026-06-15 local parity rerun proved the Rust/backend consumption slice:
  all-backend `exception_region` tests passed (31), `shared_drop` tests passed
  (3), `exception_pop` tests passed (2), the focused WASM LIR
  `DelBoundary`-to-`dec_ref_obj` regression passed, and
  `cargo build -p molt-backend --profile dev-fast` passed. The same rerun also
  proved marker-only `drop_inserted` facts survive pass-manager
  snapshot/restore as first-class fact changes, and that frontend JSON preserves
  `bound_local` for list/tuple/dict/set/frozenset absorbing constructors. The
  same-day
  targeted hot-only `bench_exception_heavy` attempt was non-authoritative and
  refused (`size phase: nonzero`) while another native selected-operand test was
  active, so no speed claim moved.
  Backend compile cache publication uses session-independent locks under the
  resolved cache root's `locks/` directory, while Cargo/backend rebuild locks
  are keyed by the mutable build-state root: default `MOLT_SESSION_ID` runs get
  isolated lock directories and explicit shared `CARGO_TARGET_DIR`/
  `MOLT_BUILD_STATE_DIR` runs share lock files. Canonical dev/CI/DX and CLI
  Cargo build environments default `CARGO_INCREMENTAL=0` unless an operator
  explicitly opts into incremental-debug work, and the memory guard quarantines
  only Cargo `*/incremental` directories under the effective `CARGO_TARGET_DIR`
  after guarded Cargo/rustc/rustdoc interruption, with summary JSON/stderr
  receipts and bounded quarantine retention. Multi-agent task scaffolds now
  route through `tools/throughput_env.sh` and capture a sourced
  `logs/agents/<task>/env.sh`, so resumed agents reuse the same
  `RunContext`-derived artifact root, shared target/cache roots, daemon socket
  policy, and `MOLT_SESSION_ID` instead of rediscovering build custody from
  ambient shell state. Runtime stringprep is now
  leaf-owned: the `molt-runtime` in-facade fallback module is deleted,
  `molt_stringprep_*` resolver ownership is generated into the
  `molt-runtime-stringprep` leaf sub-registry, and feature-on/feature-off
  runtime checks prove the facade no longer carries a second stringprep
  implementation. Runtime static-archive fingerprints now
  include all extracted `molt-runtime-*` leaf crates plus the runtime workspace
  manifests, so edits to leaves such as `molt-runtime-stringprep` invalidate
  profile-qualified `libmolt_runtime.*.a` artifacts instead of linking stale
  archives. Persisted JSON/text/byte cache,
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
- Runtime import entrypoints now have one Rust-owned transaction path.
  Source-language imports route through `molt_importlib_import_transaction`
  with explicit import payloads, while the public `importlib.import_module`
  shim uses the narrower `molt_importlib_import_module(name, package)` wrapper
  for CPython public-API argument validation and relative-name resolution before
  delegating into the same transaction implementation. The empty
  `_MODULE_ALIASES` side table was deleted, and frontend literal/direct-call
  folding of `importlib.import_module("literal")` now emits the public
  `molt_importlib_import_module(name, None)` wrapper whenever callable identity
  and an absolute literal name are statically stable; runtime import owns target
  availability,
  version-gated absence, module cache custody, provenance, fromlist behavior,
  and error shape. User rebinding of `importlib.import_module` still stays
  observable in compiled native code because the frontend records module-attribute
  mutation through `importlib` or any alias and disables both the transaction
  fold and cross-module static direct-call lowering when that attribute is not
  stable. Build-time module graph discovery uses the same callable-identity
  model for `importlib`, `importlib as alias`, and
  `from importlib import import_module as alias`, and refuses static target
  collection after `import_module` rebinding.
- Ordinary source-language imports now carry explicit
  `name`/`fromlist`/`level` payloads into the same Rust transaction for the
  focused active paths. Graph-proven `fromlist` child auto-import/binding is
  transaction-owned for the covered native path: existing package exports win,
  successful child module imports bind onto the parent package, absent requested
  children fall through to the final `IMPORT_FROM` `ImportError`, and dependency
  import errors propagate instead of being broadly suppressed. Static package
  `__all__` child modules named by source `from package import *` now enter the
  same import-scan/dependency graph and the same Rust transaction-side
  `fromlist=["*"]` preparation path; successful children are materialized before
  star binding, while missing children remain absent and produce the CPython
  star-binding `AttributeError`. Relative `builtins.__import__` package-context
  calculation now follows the CPython 3.12 order for the covered transaction
  cases: `globals` must be a dict, non-`None` `__package__` must be a string,
  `__package__ is None` consults `__spec__.parent` and preserves missing-parent
  `AttributeError`, and fallback uses `__name__` plus package `__path__` or
  dotted-name parent calculation. The broader dynamic `__all__` and
  namespace-package edge matrix remains open. Public resolver validation for
  `importlib.import_module` and `importlib.util.resolve_name` now shares private
  Rust relative-name math while preserving CPython 3.12 API-specific error
  surfaces for non-string names/packages, missing packages, empty names, and
  beyond-top-level relative imports. `FileLoader`/`SourceFileLoader.load_module`
  now routes through the shared Rust spec-execution transaction used by the
  runtime spec-first import path: new modules are visible in `sys.modules`
  before `exec_module`, failed new loads are removed, existing module reload
  failures preserve CPython's no-rollback behavior, and successful
  `sys.modules` substitution determines the returned module object. Current
  focused coverage for this active import slice is split by authority: source
  imports/fromlist still exercise `molt_importlib_import_transaction`, while
  public `importlib.import_module` literal folds and shim dispatch exercise the
  public `molt_importlib_import_module(name, package)` wrapper before it enters
  the shared Rust transaction core. The cache-identity regression coverage now
  proves that shared stdlib keys diverge for resolved capability config,
  capability-manifest runtime env, and ambient per-file `MOLT_CAPABILITIES`;
  rerun the order-dependent full-profile importlib differential shard before
  promoting the import matrix to green. Existing focused coverage also proves
  that pending child-body import failures are preserved through active handler
  frames and fromlist preparation: the runtime
  `from_import_child_missing_clear_preserves_unrelated_pending_failure_in_handler`
  unit, the seven-case native import transaction/fromlist slice in
  `tests/test_native_import_bootstrap_regressions.py`, and the four-file
  full-profile differential slice
  (`importlib_import_module_basic.py`,
  `importlib_import_module_helper_constant.py`,
  `importlib_import_module_helper_submodule.py`,
  `importlib_dunder_import_fromlist.py`). The same four-file slice fails closed
  under the micro profile with an explicit stdlib feature-profile diagnostic for
  `zipfile`/`csv`/compression dependencies; treat that as profile selection
  evidence, not semantic import failure. The static package `__all__`
  star-child proof is `tests/test_native_import_star_all_regressions.py`,
  `tests/cli/test_cli_import_collection.py::test_from_import_star_graph_admits_static_all_child_module`,
  the paired basic differential rerun
  (`logs/import_star_package_all_child_pair_diff.log`,
  `logs/import_star_package_all_child_pair_diff_results.jsonl`), and the
  transaction regression rerun
  (`logs/importlib_transaction_fromlist_star_regression_diff.log`,
  `logs/importlib_transaction_fromlist_star_regression_diff_results.jsonl`).
  The package-context proof is
  `tests/test_native_import_package_context_regressions.py`,
  `tests/differential/basic/import_dunder_package_context.py`,
  `logs/import_dunder_package_context_diff.log`, and
  `logs/import_dunder_package_context_diff_results.jsonl`.
  The public importlib resolver-validation proof is
  `tests/test_native_importlib_public_api_regressions.py`,
  `tests/differential/stdlib/importlib_public_api_validation.py`,
  `logs/importlib_public_api_validation_diff.log`, and
  `logs/importlib_public_api_validation_diff_results.jsonl`.
  The load-module spec-execution transaction proof is
  `tests/test_native_importlib_load_module_transaction.py`,
  `tests/differential/stdlib/importlib_load_module_transaction.py`,
  `logs/importlib_load_module_transaction_diff.log`,
  `logs/importlib_load_module_transaction_diff_results.jsonl`,
  `logs/importlib_spec_execution_transaction_regression_diff.log`, and
  `logs/importlib_spec_execution_transaction_regression_diff_results.jsonl`.
  A current native
  `importlib.util.module_from_spec` external-module materialization proof did
  not reach runtime semantics: its guarded pytest finished with return code 1
  because the inner native full-profile build hit the harness 600s build
  timeout (`tmp/pytest-memory-guard/pytest-50869_outer-guard.json`,
  `logs/harness_memory_guard/commands.jsonl`; `violation=null`, no orphaned
  process groups). Treat that as compile-throughput/DX evidence, not an
  importlib semantic failure. The separate full-profile native `threading`
  import split-transport blocker is closed: the SimpleIR megafunction splitter
  now clones suffix cleanup handlers only when the suffix's external reads are
  available from the extracted chunk or its split frame, fails closed instead of
  stripping external `check_exception` targets, and passes
  `test_native_full_profile_import_threading_survives_split_frame_transport`
  for default, `1000`, and `500` split limits under
  `logs/memory_guard_native_threading_all_summary.json` (2.41 GB peak
  process-tree RSS, `violation=null`, no orphaned process groups).
- Native module attribute lookup preserves CPython-shaped module `__getattr__`
  exception custody for the covered bootstrap path: direct `module.attr` and
  two-argument `getattr(module, name)` propagate a raised `AttributeError`,
  while `getattr(module, name, default)` suppresses only `AttributeError` and
  returns the default. The runtime keeps the pending exception as runtime state
  and returns the normal error sentinel to native code instead of transferring a
  borrowed pending exception object through the result value.

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
- Native RC ownership now uses the same TIR DropInsertion activation path as
  LLVM/WASM/Luau for the proven ExceptionRegion slice. Remaining native RC work
  is deletion of broader automatic temp-RC/value-tracking lanes once shared
  drop/codegen facts cover their full ownership surface. The old native-local
  CreationRef/MatchRef maps and `exception_pop` release side path are gone; no
  further native exception-release map was found safe to delete. The pre-bail
  exception-only drop slice now has its own
  `exception_region_drops_inserted` fact, while `drop_inserted` remains reserved
  for full-function RC ownership and native legacy-RC suppression. The remaining
  deletion blocker is wider shared drop/codegen coverage for handler/state-machine
  lifetimes, not an exception-release side path.
- Exception handler lifetime is not yet represented as a full backend-neutral
  `ExceptionRegion` / `HandlerState` ownership boundary. Native Cranelift now
  consumes shared TIR DropInsertion `DecRef`s for CreationRefs and reachable
  handler MatchRefs; the old native-only CreationRef lifetime carve-out and
  exception-pop side path are deleted. Checked backend consumption evidence now
  covers LLVM lowering order, WASM host-EH/native-EH
  import behavior plus the LIR `dec_ref` runtime-call lane, shared
  `exception_pop` block-arg split/drop dominance, and Luau lowering of
  shared `drop_inserted` / `exception_region_drops_inserted` / `inc_ref` /
  `dec_ref` / `release` artifacts as GC no-ops plus executed Luau and LLVM
  runtime proof for the raise/catch leak loop. WASM now has runtime differential
  proof for that focused leak loop; completion still requires broader WASM
  `HandlerState` parity and the wider
  `HandlerState` boundary. The 2026-06-20 direct release-fast WASM backend
  replay also proves the active-frame ExceptionRegions rule and removes the
  `_collections_abc__Sequence_index` false-owner RSS cliff under the same 12 GiB
  guard that previously killed the backend. The prior WASM runtime-surface blocker that pulled
  `molt-db`/sqlite into linked runtime builds is closed at the feature-plane
  level: wasm micro/full availability, Cargo command features, fingerprints,
  and bench-wrapper runtime feature construction exclude sqlite, while explicit
  sqlite-on-wasm still fails closed. The corrected end-to-end WASM proof now
  builds a structurally valid linked artifact and advances past the former
  `func 1233` stack-validation failure. The JS harness host map now includes the
  process host ABI imports required by the linked runtime, including
  `env::molt_process_terminate_host`, and the
  `tools/wasm_diff.py` leak-loop differential now passes for
  `tests/differential/memory/exception_raise_catch_loop_leak.py`. This closes
  the raise/catch leak-loop runtime proof for WASM; broader WASM
  `HandlerState` parity remains open. A
  2026-06-12 targeted `bench_exception_heavy` hot-only after-Luau-parity rerun
  produced valid in-binary cycle attribution (`inner_loops=40`, launch/page-in
  0.0%, top in-binary frames `molt_runtime::object::dec_ref_ptr` 10.2%,
  `molt_runtime::concurrency::gil::GilGuard::new` 10.1%, and
  `bench_exception_heavy__molt_user_main` 8.0%) but was non-authoritative
  because host load was not quiescent (`loadavg_1m=23.81`, threshold `9.00`);
  no speedup claim moved. The
  2026-06-15 hot-only rerun likewise moved no claim: it was non-authoritative
  and refused before sampling because the looped profiling binary failed during
  the size phase while another native selected-operand test was active.
  The targeted native leak gate
  `MOLT_ASSERT_NO_LEAK=1 python3 tools/safe_run.py --rss-mb 1024 --timeout 180 -- uv run python -m molt.cli run tests/differential/memory/exception_raise_catch_loop_leak.py --target native --release --rebuild`
  passed with `live_objects=649` after 500,000 raises/catches.
- Non-escaping objects with `__del__` now have a shared TIR drop-insertion
  primitive for dominated return-boundary release instead of SSA-last-read
  release. `DeleteVar` now carries the old slot occupant as a first-class TIR
  operand and the shared drop pass releases that occupant at the delete boundary
  while excluding `None`/missing sentinels from RC placement. The frontend now
  preserves the named-local `bound_local` carrier for dict/set/frozenset
  constructors as well as list/tuple. Current proof is deliberately narrower:
  native scope-exit ordering plus unit-level direct-field/runtime finalizer
  guards pass, and the 2026-06-15 focused native differential shard now passes
  object-attribute release, container clear/pop/delete finalizers, the finalizer
  matrix, and standalone raising-finalizer isolation. Complete finalizer
  ordering parity still requires widening this proof across backend/profile
  parity and deleting stale native value-tracking lanes once shared facts cover
  them.
- Runtime descriptor-cache lookup now returns retained snapshots rather than
  shallow copied heap bits, and `descriptor_bind` owns the descriptor for the
  full binding operation. This closes the reentrant descriptor mutation class
  where cached/property/class-dict descriptors could be deleted or replaced
  while `__get__`/property code still used borrowed storage. Guarded evidence:
  `uv run python tools/guarded_exec.py --prefix MOLT_TEST --timeout 600 -- cargo test -p molt-runtime --lib descriptor_bind_retains_descriptor_across_get_mutation -- --nocapture`,
  `uv run python tools/guarded_exec.py --prefix MOLT_TEST --timeout 240 -- cargo test -p molt-runtime --lib descriptor_cache_store_owns_released_heap_bits -- --nocapture`,
  and `uv run python tools/guarded_exec.py --prefix MOLT_TEST --timeout 240 -- cargo test -p molt-runtime --lib class_apply_set_name_retains_entries_across_hook_mutation -- --nocapture`.
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
  into that function-owned map. The type-refine solver treats produced values as
  `Never` until solved, recomputes op results from opcode, operands, and
  structural attrs each round, widens known-dynamic results to `DynBox`, and
  fails closed on nonconvergence instead of freezing oscillating values through a
  stderr fallback. Range/list devirtualization records the I64 and Bool facts it
  synthesizes for generated loop carriers and comparisons instead of leaving
  those facts solely in `_fast_int` attrs. TIR-to-SimpleIR value
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
	  Upstream tinygrad is now registered as an enabled pinned friend-suite
	  benchmark lane (`tinygrad_off_the_shelf`, commit
	  `a83710396c991272241e40da94489747c2393851`). Its upstream-owned
	  `tinygrad` runner executes `CHECK_OOB=0 DEV=CPU TYPED=1 python
	  test/test_tiny.py` through an isolated no-project
	  `uv run --isolated --no-project --with typeguard` dependency lane plus
	  runner-local `PYTHONPATH={suite_root}` so the pinned checkout stays clean;
	  the CPython runner executes public API
	  workloads through `tools/tinygrad_off_shelf_adapter.py`. The Molt runner is
		  executable by default and uses the full-stdlib `{project_python} -m molt.cli run`
		  static-package command; earlier guarded evidence reached `molt-backend --daemon`
	  and then tripped the guarded process RSS limit at 12.005 GB after 435.5s
	  (`tmp/memory_guard/friends_tinygrad_molt_sqlite_profile.json`), proving
		  that blocker was backend-daemon compile memory before adapter
	  workload execution. Native TIR optimization now partitions uncached
	  user-function work by function count and op budget, runs only one bounded
	  parallel batch at a time, and applies/cache-writes optimized ops before
	  constructing the next batch. Follow-up guarded evidence
	  (`tmp/memory_guard/friends_tinygrad_molt_tir_batched.json`,
	  `bench/results/friends/20260612T184515Z/`) reached that bounded path,
	  reduced the peak single backend process, and exposed the next bug as
	  aggregate process-tree RSS from overlapping daemon plus hidden one-shot
	  fallback. The CLI now fails closed after full daemon request admission
	  instead of restarting the daemon or silently launching that second backend
	  compile; short readiness misses from verified live daemons no longer
	  authorize socket unlink/rebind while another compile may be running.
	  Follow-up guarded evidence
	  (`tmp/memory_guard/friends_tinygrad_molt_daemon_custody.json`,
	  `bench/results/friends/20260612T203111Z/`) no longer trips the outer
	  memory guard (`violation=null`, no orphaned groups, 4.92 GB peak
	  process-tree RSS), but the daemon dies mid full request; the runner stderr
	  is the explicit fail-closed diagnostic `Backend daemon compile failed:
	  backend daemon died while request was in flight`. A 2026-06-15 guarded
	  list-workloads smoke
	  (`tmp/memory_guard/tinygrad_importlib_module_from_spec_smoke.json`) timed
	  out after 900s with `violation=null`, no orphaned process groups, 3.75 GB
	  peak process-tree RSS, and Cargo incremental quarantine while compiling the
	  full-stdlib tinygrad adapter. The active backend IR for that lane was
	  49 MB with 5,845 functions and 866,671 ops, so classify the result as cold
	  build/compiler-throughput evidence before adapter workload enumeration,
	  not a tinygrad semantic failure. Direct guarded backend replays of that
	  49 MB IR (`tmp/memory_guard/tinygrad_backend_replay_indexed_20260615.json`
	  and
	  `tmp/memory_guard/tinygrad_backend_replay_indexed_scratch_20260615.json`)
	  both parsed the IR, detected 1,469 leaf functions, and then failed closed
	  before object emission because `MOLT_RUNTIME_INTRINSIC_SYMBOLS` was absent;
	  they are backend compile-memory receipts only, with peak RSS 0.891 GB and
	  0.887 GB respectively, not semantic/codegen success. A later lazy-index
	  guarded list-workloads retry
	  (`tmp/memory_guard/tinygrad_adapter_list_workloads_lazy_index_20260615.json`)
	  still timed out in the full-stdlib adapter build after 1200s with
	  `violation=null`, no orphaned process groups, 1.34 GB peak process RSS, and
	  2.28 GB peak process-tree RSS; the post-run sentinel receipt
	  (`tmp/memory_guard/process_sentinel_after_lazy_index_20260615.json`) returned
	  0 with no incident or orphaned process groups. A later guarded
	  Molt-only rerun (`bench/results/friends/20260612T205850Z/`) did not trip the
	  memory guard and instead failed after 208.19s with `Backend daemon compile
	  failed: backend daemon returned empty response`. The 21:12 guard sidecar
	  (`tmp/memory_guard/friends_tinygrad_molt_daemon_harness_custody.json`)
	  records a separate daemon compile-memory event: the bench sentinel
	  terminated only the Molt-owned daemon process group when that process hit
	  the 12 GB process RSS cap. Native application-object batching now consumes
	  the same `MOLT_BACKEND_BATCH_OP_BUDGET` authority as stdlib batching, and
	  the production self-spawn worker path is covered by
	  `cargo test -p molt-backend --test native_batch_worker_spawn`
	  (`tmp/memory_guard/cargo_test_native_batch_worker_spawn_cleanup_diag_20260615.json`):
	  the real `molt-backend` binary compiles two live functions as two
	  materialized batches through `--native-batch-job-file`. Daemon-off proof
	  now builds the full-stdlib adapter and reaches upstream tinygrad runtime
		  execution under guard. The older 1.985 GB invalid-header receipt is
		  historical after the importlib bootstrap export, list-clear detach,
		  namedtuple return-boundary ownership, defaultdict factory-handle
		  ownership, and deque retained-handle ownership fixes. Fresh 2026-06-20
		  guarded evidence now builds the full-stdlib adapter, gets past the
		  `tinygrad/uop/ops.py:1586` teardown invalid-header abort, and fixes the
		  post-JSON `argparse.Namespace` return-cleanup double drop.
		  Direct rebuilt-adapter evidence covered the then-four default
		  public-API workloads. The current CPython adapter source now enumerates
		  five default public-API workloads, including `attention_core`, and the
		  pinned upstream CPython probe exits cleanly for all five. The official
		  `tinygrad_off_the_shelf` Molt friend runner with clean pinned source
		  custody reached upstream tinygrad's lazy pattern compiler at
		  `tinygrad/uop/upat.py:167`, where `upat_compile` calls
		  `exec(code_str, globs, namespace)`. Unrestricted `exec()` is outside
		  Molt's verified AOT subset; historical artifact:
		  `bench/results/friends/2026-06-20-tinygrad-origin-fix-rerun/`. The
		  friend manifest now prepares a generated
		  `_molt_tinygrad_upat_static_exec_registry` module from pinned upstream
		  matcher sources, admits it beside `tinygrad` in the Molt
		  static-package lane, and configures the adapter to install
		  `exec_static` as the package-scoped `tinygrad.uop.upat.exec` global.
		  Fresh 2026-06-23 guarded evidence
		  (`bench/results/friends/20260623T131504Z-tinygrad-molt-fixed-env/`)
		  now gets through registry preparation, backend object emission, and
		  native Windows linking under the sanitized friend harness environment
		  with clean pinned source custody; the current blocker is runtime
		  execution failing with `TypeError: 'str' object is not callable` from
		  `<molt-builtin>` line 12. The next required proof must isolate that
		  TypeError in the wired-registry runtime path. It remains the
	  compatibility/perf case study for compiling and profiling unmodified
	  tinygrad code. The friend-suite harness now records
	  git source custody, fails dirty or wrong-ref checkouts, accepts per-suite
	  `--suite-root` and `--repo-ref` overrides for pinned local clones, supports
	  manifest-declared runner names without a hidden allowlist, ingests
	  `json_stdout` workload timings into structured runner metrics, preserves
	  per-phase memory-guard diagnostics, has emergency-writer coverage for
	  bounded partial `results.json` / `summary.md` snapshots, and cleans only
	  identity-verified current-session backend daemons through the canonical
	  custody module. The public tinygrad wrappers now
	  carry canonical dtype codes, byte tensors report `uint8`, explicit uint/int
	  constructors upload exact little-endian storage through
	  `molt_gpu_prim_create_tensor_raw`, typed zeros use
	  `molt_gpu_prim_zeros_dtype`, handle-only readback decodes
	  `molt_gpu_prim_read_data_raw` without f32 transit, and elementwise
	  unary/binary operations, ternary `where`, typed casts, explicit-axis
	  reductions, and Rust-owned all-axis reductions via
	  `molt_gpu_prim_reduce_all` carry runtime handles through the corresponding
	  GPU primitive intrinsics. Public `import tinygrad` and
	  `from tinygrad import Tensor` now expose the same `molt.gpu.Tensor` class,
	  carry canonical dtype objects, and cover `where` promotion plus
	  pad/shrink/flip/contiguous view movement through the off-the-shelf adapter
	  workloads. The
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
	  scan policy and stdlib allowlist digest, and Darwin memory-guard sizing now
	  uses `vm_stat` free/inactive/speculative/purgeable pages as the live
	  available-memory source instead of falling back to physical-RAM-only
	  budgeting. Import graph
	  materialization now has one immutable `ImportPlan`: entry planning owns the
	  runtime-import support closure, while final materialization owns namespace
		  stubs, generated importer modules, known-module sets, allowlist snapshots,
		  and module graph metadata before frontend analysis or backend lowering can
		  observe the graph. The final binary-image closure payload is emitted in
		  build diagnostics and wrapper-cache manifests; static imports are final
		  image roots, and dead-module-elimination mode participates in wrapper
		  cache identity. Build diagnostics also emit `binary_image_analysis`,
		  a cross-layer source/AST, schedule, lowering, backend IR/TIR-input, and
		  artifact evidence envelope with a frontend SourceSite digest ledger:
		  source hashes, span-derived AST site digests, binary-image roles, and a
		  semantic identity digest. The active IR/TIR source-site carrier now moves
		  `source_line`, `col_offset`, and `end_col_offset` from frontend line
		  markers through SimpleIR, TIR `SourceSite` attrs, selected optimization
		  rewrites, TIR-to-SimpleIR lowering, and backend IR diagnostics; the
		  backend `source_sites` projection reports attributed-op coverage,
		  source-line hot spots, and a stable digest over the lowered op stream.
		  Backend diagnostics also project allocation/ownership pressure from the
		  same carrier: heap/stack allocation roots, retain/release events,
		  heap-exposure ops, arena eligibility, and finalizer-sensitive results
		  are counted by source line with a stable event digest.
		  Core stdlib closure honors the same nested-scan exception
		  set as regular stdlib discovery, so `collections` keeps its required
		  function-body `copy` import in the graph and native hello-world no longer
		  links against a missing `copy__copy` symbol. Shared stdlib cache identity now
		  seeds every explicit stdlib module init like backend DFE, requires a
		  backend-written partition manifest sidecar before reuse, and backend reuse/
		  publish rejects any partition whose SimpleIR function references are not
		  closed inside the partition (including the historical
		  `collections__UserDict_copy -> copy__copy` failure shape).
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
- Stdlib lowering audit: `916` modules audited; `41` intrinsic-backed; `874` intrinsic-partial; `1` policy-gate; `0` python-only.
- Platform availability metadata: `66` modules with explicit availability notes; `41` WASI-blocked; `37` Emscripten-blocked in CPython docs.
- Deep evidence: see the stdlib intrinsics audit and platform availability matrices under `docs/spec/areas/compat/surfaces/stdlib/`.
<!-- GENERATED:compat-summary:end -->

- Ecosystem compatibility is now generator-owned for `26` audited packages.
  NumPy is an explicit top-priority row in
  `docs/spec/areas/compat/surfaces/ecosystem/ecosystem_compat_matrix.generated.md`;
  it derives `partial` through `D28 Source-recompiled libmolt extension
  package` with `D16` lazy module attributes supported and `D23` CPython
  binary-wheel bridge recorded only as optional/non-canonical.
  `numpy_off_the_shelf` is now an enabled pinned friend lane with a
  custody-only source-tree audit runner, an isolated CPython `numpy==2.4.2`
  public-API baseline, and a canonical
  `molt extension scan --source {suite_root}/numpy --fail-on-missing` C-API
  closure gauge with per-symbol `runtime_backed`, `source_compile_only`,
  `fail_fast`, and `missing` status. The Molt runner uses
  `MOLT_EXTERNAL_STATIC_PACKAGES=numpy`, explicit `module.extension.exec`
  capability, and all-loaded-`numpy.*` module-origin custody. Build admission
  now validates and fingerprints package-local native artifact sidecars for
  explicitly admitted external packages, and native builds publish those
  validated artifacts plus sidecars and runtime shim candidates under a
  deterministic `external_static_packages/<plan-digest>/` root and inject that
  staged root into generated native binaries before runtime startup. That is not
  yet a green no-host NumPy import proof. Friend-suite metrics now exclude
  custody/scan runners from speedup math, and git-suite custody rejects ignored
  checkout artifacts in addition to dirty or wrong-ref trees. The Molt lane is
  expected to fail until no-host source-recompiled extension package build,
  NumPy C-API symbol closure, and NumPy import/runtime-load proof are complete;
  host-Python fallback is not an allowed completion path.

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
