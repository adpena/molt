# STATUS (Canonical)

This document is the source of truth for Molt's current supported surface.
It is current-state only. For forward-looking priorities, use
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
- WASM remains a supported target area, but same-contract parity with native is
  still incomplete.
- Luau is a checked source-emission target for the current/future Luau surface;
  current OpIR support is generated in
  `docs/spec/areas/compiler/luau_support_matrix.generated.md`.
- Backend-facing native and WASM lowering always runs through the TIR pipeline;
  the old environment-variable opt-out has been removed so SimpleIR transport
  metadata cannot bypass typed-IR validation.

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
  `int` claim: unbounded arithmetic and shifts stay boxed/runtime-backed until
  a range/shift-count proof can show that the operation cannot overflow i64,
  promote to BigInt, or raise for Python shift semantics.
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
Latest run: 2026-05-19 (macOS arm64, CPython 3.12.13).
Top speedups: `bench_sum.py` 22.72x, `bench_bytes_find.py` 20.20x, `bench_struct.py` 9.37x, `bench_bytes_find_only.py` 7.01x, `bench_matrix_math.py` 6.03x.
Regressions: `bench_exception_heavy.py` 0.06x, `bench_json_roundtrip.py` 0.15x, `bench_counter_words.py` 0.31x, `bench_etl_orders.py` 0.46x, `bench_csv_parse_wide.py` 0.61x, `bench_csv_parse.py` 0.74x, `bench_generator_iter.py` 0.83x, `bench_tuple_pack.py` 0.94x.
Slowest: `bench_exception_heavy.py` 0.06x, `bench_json_roundtrip.py` 0.15x, `bench_counter_words.py` 0.31x.
Molt build/run failures: `bench_async_await.py`, `bench_channel_throughput.py`, `bench_dict_comprehension.py`, `bench_import_time.py`, `bench_parse_msgpack.py`, `bench_procedural_gen.py`, `bench_ptr_registry.py`.
Comparator baseline coverage: PyPy baseline unavailable; Codon baseline unavailable; Nuitka baseline unavailable; Pyodide baseline unavailable.
WASM run: 2026-03-28 (macOS arm64, CPython 3.12.13). Slowest: none; largest sizes: `bench_sum.py` 7182.5 KB; WASM vs CPython slowest ratios: none.
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
