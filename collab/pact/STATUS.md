# STATUS - Pact dogfooding of Molt (2026-07-01)

Positive signal first: the browser call shape is no longer the core unknown.
`wasm/browser_embed.js` owns the narrow split-runtime embed path, and
`examples/browser_embed_forward/` now contains a source-level numeric
`forward(Float32Array) -> Float32Array` example plus a plain JS runner that
consumes a generated build directory. The full `wasm/browser_host.js` process
host remains separate, while `wasm/loader_bridge.js` now owns the shared WASM
import/tag parser, isolate-import i64 bridge, and reserved runtime-call bridge
used by the browser and Node loaders.

No checked-in `examples/browser_embed_forward/artifacts/` bundle remains. A
committed prebuilt runtime/app package would be a second artifact lane and would
rot unless it is managed by a release/integrity pipeline. Generated WASM remains
an output.

## Current Blocker

### 2026-07-01 Runtime ABI and DX Update

The live aperture has moved again. Queue row
`20260701T205002-pact-witness-acceptance-cdd2f00c403240e7` reached
split-runtime `app.wasm` host initialization with the staged NumPy/SciPy native
artifacts linked, then failed before `candidate_outputs.npz` with
`TypeError: call arity mismatch (expected 3, got 1) for _LazyIntrinsic.__call__`.
That exposed a shared runtime-call primitive, not a Pact-specific Python shim:
fixed-arity `call_function_objN` helpers must route varargs/default/varkw
metadata through the same `CallArgs` binder used by dynamic calls, while the
raw-trampoline fast path must not recursively bypass binding.

Green evidence for that primitive:

- `20260701T210919-runtime-call-binder-varargs-r2-2ae9ce2369164665` passed
  `cargo test -p molt-runtime --lib fixed_arity_entry_routes_varargs_functions_through_binder`.
- `uv run --active --project . --python 3.12 python tools\gen_wasm_abi.py --check --timings`
  now hits the persistent render cache in 1.362s after a source-rendering
  no-cache check passed in 26.8s; the previous observed no-cache/check path
  took 52.3s before the cache/batched-rustfmt/no-op-write move.
- `uv run --active --project . --python 3.12 python -m pytest
  tests\test_gen_wasm_abi.py::test_wasm_abi_manifest_owns_runtime_callable_registry
  tests\test_gen_wasm_abi.py::test_wasm_abi_reserved_runtime_callable_import_names_are_fail_closed
  tests\test_generate_worker.py::test_loader_bridge_enforces_manifest_reserved_callable_dispatch
  -q` passed.
- `20260701T211646-wasm-reserved-runtime-callable-table-slots-r3-627e26437ca049e2`
  passed the existing import-transaction WASM ABI proof after reserved runtime
  callables gained generated optional import-token authority. Tokenless reserved
  callables remain sentinel-owned instead of leaking fake imports; import-backed
  reserved callables can route through real ABI metadata.

Queue row `20260701T211814-pact-witness-acceptance-09339473a62c443f` was the
full acceptance rerun derived from that table proof. It built and linked split
`app.wasm` plus `molt_runtime.wasm`, then failed under Node before
`candidate_outputs.npz` with `WebAssembly.instantiate(): Compiling function
#4258 failed: undeclared reference to function #732`. That moves the next
aperture to split-runtime function-index/table relocation authority; Pact Kernel
A acceptance is still open.

The live evidence says the current tree can now build and link the Kernel A
`field_solve.py` WASM package with canonical sealed NumPy/SciPy roots, but has
not yet passed the full `candidate_outputs.npz` parity runner:

- The old first failure, `scipy.ndimage.distance_transform_edt` becoming an
  unsupported fake Python direct call, is retired for manifest-declared native
  callable exports. `known_modules` remains import visibility only, while
  direct-symbol and object-call `module_attr` callable exports now lower the
  `scipy.ndimage` witness closure to executable `invoke_ffi` ABI metadata.
- The remaining blocker is now downstream of build/link: the full acceptance
  runner must execute the emitted WASM, write `candidate_outputs.npz`, and pass
  `check_parity.py`. Queue row
  `20260701T203840-pact-witness-acceptance-43e969d640e44709` reaches that
  runner and fails in Node before `candidate_outputs.npz` with
  `RuntimeError: null function or function signature mismatch`. The immediate
  aperture is the runtime call-table/signature closure for the linked
  native/object-call path; ndarray/storage/dtype/buffer, C/API, capsule, and
  module-state remain the next primitives to promote as the trap is traced.
- Adding NumPy/SciPy source roots without package admission correctly fails
  closed.
- Adding package admission against the local Python 3.14 site-packages root now
  fails closed before graph expansion: NumPy/SciPy contain native
  source/artifact markers but do not publish wasm32 `static_link`
  `libmolt_source` artifact manifests with package symbol custody. Source roots
  alone are not linkable WASM evidence.
- The stale NumPy package-root export blocker is retired in the current witness
  root: `tmp/pact_numpy_multiarray_sealed_for_witness` publishes `python_exports
  = ["numpy"]`, target-compatible `static_link` WASM custody, and the modern and
  legacy NumPy `_ARRAY_API` / `_UFUNC_API` capsule names required by SciPy.
- The stale SciPy high-level wrapper export blocker is retired in the current
  witness root: `tmp/pact_scipy_ndimage_sealed_for_witness_next` replaces the
  bad `_nd_image`-owned wrapper exports with `module_attr` callable exports
  backed by explicit provider support modules and checksummed source custody.
- The corrected SciPy shape is provider-source plus reachable native artifacts:
  `distance_transform_edt` is provided by `scipy.ndimage._morphology`,
  gaussian/min/max filters by `scipy.ndimage._filters`, and `label` by
  `scipy.ndimage._measurements`. Those wrappers import `_nd_image`,
  `_ni_label`, `_rank_filter_1d`, `_ni_support`, `_ni_docstrings`, and narrow
  SciPy/NumPy helpers. The current sealed witness plan selects the existing
  `_nd_image` and `_ni_label` static-link artifacts plus NumPy
  `_multiarray_umath`; `_rank_filter_1d` remains the next wrapper-reachable
  native artifact to expose if graph/runtime execution reaches rank-filter
  support.
- The current recovery first moved the next failure from late WASM execution
  into import/link authority. Reachable provider support source is now sliced
  once for graph discovery and frontend lowering, decorator/doc-only support
  imports are stripped from executable closure, stdlib helper imports join the
  runtime import-dispatch roots, and missing native-package child imports fail
  closed during import-plan materialization. An earlier Pact build stopped in
  4.7s with `scipy.ndimage._ni_label` reported as lacking source or artifact
  custody, instead of building a candidate that traps later with `ImportError`.
  That failure consulted sealed-artifact sidecar provenance and pointed at the
  upstream source candidate:
  `bench\friends\repos\scipy_off_the_shelf\scipy\ndimage\src\_ni_label.pyx`.
  With a target-specific WASI sysroot configured, the source-extension lane now
  builds `_ni_label` into a wasm32 static-link artifact without cloning or
  rewriting SciPy semantics: `object_count=1`, `linked_object_count=1`,
  `warnings=[]`, and `errors=[]`. This retires the first missing native child
  artifact.
- An earlier package-admission probe timed out after 300s in the live WASM build
  path.
- A graph-only probe took 100.4s before backend work, found 186 modules, zero
  staged native artifacts, and pulled broad NumPy plus `scipy` and
  `scipy.ndimage` package initializer closure.
- The latest NumPy `_multiarray_umath` source-plan C/API graph probe is now
  green before backend work: 109 reachable compile units, 44 narrow
  package-generated token-paste prefixes, 17 generated helper symbols, zero
  missing C/API symbols, and zero fail-fast C/API symbols. This retires the
  previous scanner noise around `npy_to_*`, `npyv_*`, local static
  `PyArray_*`, and `PyUFunc_handlefperr` symbols; it does not yet prove
  compile/link/import/runtime execution.
- The latest source-extension build probe reaches toolchain custody before any
  broad NumPy object compilation. LLVM `clang`, `wasm-ld`, and a target-specific
  WASI sysroot are enough to compile the focused `_ni_label` artifact; broad
  NumPy/SciPy package execution still needs the remaining reachable native
  artifact, ndarray/storage, and C/API primitive closure.

The latest custody/build probe closes the first manifest-led artifact plan
without entering broad graph discovery: roots
`tmp/pact_numpy_multiarray_sealed_for_witness` and
`tmp/pact_scipy_ndimage_sealed_for_witness_next` select
`numpy._core._multiarray_umath`, `scipy.ndimage._nd_image`, and
`scipy.ndimage._ni_label`; publish the five Kernel A ndimage callable exports;
stage `_morphology`, `_filters`, `_measurements`, `_ni_support`,
`scipy._lib._util`, and `numpy.exceptions`; and pass the queued WASM build/link
row `20260701T203639-pact-witness-acceptance-28ad06e50b3344bb`
(`app.wasm` plus `molt_runtime.wasm`). The generated support files in
`tmp\pact_witness_acceptance_queue\.molt_build\field_solve\` are
`native_support_numpy_exceptions.py`,
`native_support_scipy_ndimage__filters.py`,
`native_support_scipy_ndimage__measurements.py`,
`native_support_scipy_ndimage__morphology.py`,
`native_support_scipy_ndimage__ni_support.py`, and
`native_support_scipy__lib__util.py`.

That means the next real structural unit is the live Kernel A runtime/parity
closure: promote the first missing runtime call-table/signature, C/API,
ndarray/storage, buffer, capsule, module-state, or object-call primitive exposed
by the Node trap into shared Molt ABI surface, then rerun the named acceptance
lane. Molt-owned Python shims for NumPy/SciPy would be the wrong architecture.

## What Worked

- The repository has first-class WASM and browser surfaces: split runtime,
  browser embed JS, process host JS, browser GPU worker, and generated ABI
  tables.
- The browser embed path can be expressed as source plus generated artifacts:
  compile the example through `molt build --target wasm --profile browser
  --wasm-profile pure --split-runtime`, serve the output directory, then call it
  from plain JS.
- The WASM ABI manifest now carries lifecycle runtime aliases such as
  `molt_runtime_init` without turning them into Python-callable runtime
  trampolines.
- Package-native browser forward calls now have a real typed WASM ABI:
  `molt.forward_f32_v1` imports native symbols as
  `(input_ptr: i32, byte_len: i64, output_ptr: i32) -> i32`, routes through
  generated runtime byte/scratch imports, and `browser_embed.js` satisfies that
  contract with direct `Float32Array` memory views. There is no boxed
  `forward_f32_v1` native-call lane left.
- `molt extension build --target wasm` now has the missing producer-side
  custody contract for package-native admission: it emits a wasm32
  `static_link` `.molt.wasm` artifact, validates declared `direct_symbol`
  callable exports against binary function exports, and publishes sidecar
  `object_closure` symbol custody. The installed NumPy/SciPy wheels still need
  reachable source-recompiled artifacts in that shape before the pact witness
  can link.

## Proof Status

Queue-native Pact witness lanes:

- `uv run --active --project . --python 3.12 python tools/proof_queue.py status`
- `uv run --active --project . --python 3.12 python tools/proof_queue.py pact-witness-acceptance` owns the heavy browser/WASM Kernel A aperture. The current spec renders to `tools/pact_witness_acceptance.py`, which builds `field_solve.py`, runs the emitted WASM, writes `candidate_outputs.npz`, and executes `check_parity.py`.
  Latest full-acceptance evidence:
  `20260701T203840-pact-witness-acceptance-43e969d640e44709` builds and links,
  then fails in Node before `candidate_outputs.npz` with
  `RuntimeError: null function or function signature mismatch`.
  The named lane now auto-admits conventional staged NumPy/SciPy static-link
  artifact roots under `tmp/` when present, so the default acceptance command
  exercises manifest-led package-native closure instead of rediscovering the
  unauthenticated `distance_transform_edt` direct-call failure.
- `uv run --active --project . --python 3.12 python tools/proof_queue.py pact-witness-oracle` is the smallest queued witness parity proof: it regenerates the Kernel A fixture/reference pair and runs `check_parity.py reference_outputs.npz` under queue custody.

Green in this recovery:

- `uv run ruff check src\molt\cli\extension_scan_surface.py src\molt\cli\extension_scan.py src\molt\cli\source_extensions.py src\molt\cli\commands.py tests\cli\test_cli_extension_commands.py`
- `uv run pytest tests\cli\test_cli_extension_commands.py -q -k "source_plan_object_closure or generated_c_api_symbols or macro_bodies"`
- Graph-only NumPy `_multiarray_umath` source-plan C/API probe against
  `tmp\worktrees\pact-collab\tmp\pact_numpy_multiarray_meson_wasm_build_generated_metadata`:
  109 compile units, 17 package-generated helper symbols, `missing_count 0`,
  `fail_fast_count 0`.
- `uv run ruff check src\molt\cli\source_extension_toolchain.py tests\cli\test_cli_extension_commands.py`
- `uv run pytest tests\cli\test_cli_extension_commands.py -q -k "source_extension_toolchain or source_plan_object_closure or generated_c_api_symbols"`
- `MOLT_WASM_CC=clang uv run python -m molt extension build --project <numpy_off_the_shelf> --module numpy._core._multiarray_umath --target wasm --abi-tier cpython-abi --source-plan <intro-targets.json> --source-plan-target _multiarray_umath --source-plan-source-root <numpy_off_the_shelf> --source-plan-build-root <meson_wasm_build> --source-plan-compile-commands <compile_commands.json> --capabilities fs.read --python-export numpy --provided-capsules numpy.core._multiarray_umath._ARRAY_API --provided-capsules numpy.core._multiarray_umath._UFUNC_API --no-deterministic --json` fails fast before object compilation because `clang` cannot compile the WASI probe including `<errno.h>` without `WASI_SYSROOT`, `WASI_SDK_PATH`, or `zig`.
- `uv run ruff check src/molt/native_callable_abi.py src/molt/frontend/visitors/call_module_dispatch.py src/molt/frontend/visitors/statement_scope.py tests/cli/test_cli_import_collection.py tests/test_wasm_browser_embed.py`
- `uv run pytest tests/cli/test_cli_import_collection.py::test_frontend_pact_ndimage_operation_closure_lowers_to_native_abi -q`
- `uv run pytest tests/cli/test_cli_import_collection.py::test_external_native_artifact_plan_rejects_unknown_callable_export_abi tests/cli/test_cli_import_collection.py::test_frontend_native_callable_callargs_export_lowers_keyword_child_module_attr tests/cli/test_cli_import_collection.py::test_browser_native_callable_manifest_is_import_driven tests/test_wasm_browser_embed.py::test_browser_embed_object_callargs_native_callable_import_adapter -q`
- `cargo test -p molt-ir native_callable_abi_contracts_are_canonical --lib`
- `cargo test -p molt-backend-wasm --features wasm-backend native_callable_direct_symbol_object_callargs_imports_and_directly_calls_symbol --lib`
- Queued `pact-witness-acceptance` fails closed at the same Kernel A build aperture when no package/native callable metadata is admitted.
- Queued `pact-witness-acceptance` with `MOLT_MODULE_ROOTS=<Python 3.14 site-packages>` and `MOLT_EXTERNAL_STATIC_PACKAGES="numpy scipy"` fails closed before graph expansion because no wasm32 `static_link` `libmolt_source` artifact manifests exist for the admitted NumPy/SciPy roots.
- `uv run pytest tests/cli/test_cli_extension_commands.py::test_extension_build_wasm_target_emits_static_link_artifact_and_manifest tests/cli/test_cli_extension_commands.py::test_extension_build_wasm_target_rejects_missing_direct_symbol tests/cli/test_cli_extension_commands.py::test_extension_build_cross_target_uses_target_compiler_and_manifest tests/cli/test_cli_extension_commands.py::test_extension_build_emits_public_exports_in_manifest tests/cli/test_cli_extension_commands.py::test_extension_build_emits_wheel_and_manifest -q`
- `uv run pytest tests/cli/test_cli_import_collection.py::test_external_static_package_wasm_artifact_plan_is_manifest_led tests/cli/test_cli_import_collection.py::test_source_recompiled_static_package_requires_native_artifact_candidate_pregraph tests/cli/test_cli_import_collection.py::test_admitted_external_native_package_does_not_close_source_only_ndimage_initializers tests/cli/test_cli_import_collection.py::test_external_native_artifact_plan_rejects_missing_wasm_callable_symbol tests/cli/test_cli_import_collection.py::test_external_native_artifact_plan_selects_callable_exported_imports -q`
- `uv run pytest tests/cli/test_cli_import_collection.py::test_frontend_pact_ndimage_operation_closure_lowers_to_native_abi tests/cli/test_cli_import_collection.py::test_browser_native_callable_manifest_is_import_driven tests/test_wasm_browser_embed.py::test_browser_embed_object_callargs_native_callable_import_adapter tests/test_wasm_browser_embed.py::test_browser_embed_native_callable_import_must_be_manifest_declared -q`
- Queued `pact-witness-acceptance` with local Python package roots still fails closed before graph expansion because the installed NumPy/SciPy roots have no wasm32 `static_link` `libmolt_source` artifact manifests.
- `cargo test -p molt-lang-cpython-abi pycomplex_binary_exports_fail_closed_until_bridge_storage_exists --lib`
- `uv run pytest tests/cli/test_cli_extension_commands.py::test_cpython_abi_variadic_shim_does_not_export_header_inline_stubs -q`
- `uv run pytest tests\test_gen_wasm_abi.py::test_wasm_abi_manifest_owns_lir_runtime_calls tests\test_gen_wasm_abi.py::test_wasm_abi_manifest_owns_runtime_export_policy -q`
- `uv run python tools\gen_wasm_abi.py --check`
- `uv run pytest tests\test_wasm_browser_embed.py::test_browser_embed_forward_f32_native_callable_import_adapter -q`
- `cargo check -p molt-backend-wasm --features wasm-backend`
- `cargo test -p molt-backend-wasm --features wasm-backend native_callable_forward_f32_imports_and_directly_calls_typed_payload_symbol --lib`
- `uv run ruff check src\molt\cli\extension_audit.py src\molt\cli\entrypoint_parser.py src\molt\cli\entrypoint_dispatch.py tests\cli\test_cli_extension_commands.py tests\test_frontend_ir_alias_ops.py tools\agent_coordination.py tests\test_agent_coordination.py`
- `uv run pytest tests\test_agent_coordination.py -q`
- `uv run pytest tests\test_frontend_ir_alias_ops.py -q -k "native_callable"`
- `uv run pytest tests\cli\test_cli_extension_commands.py -q -k "extension_audit_requires_manifest_python_export or extension_audit_reports_required_callable_exports_json or extension_audit_requires_checksums_when_requested"`
- `uv run pytest tests\cli\test_cli_extension_commands.py -q -k "extension_audit_requires_static_link_artifact_custody or extension_audit_rejects_static_link_artifact_hash_mismatch"`
- `uv run pytest tests\cli\test_cli_import_collection.py -q -k "native_callable_module_attr or native_callable_callargs_export or native_callable_manifest_is_import_driven or external_native_artifact_plan_selects_module_attr"`
- `uv run pytest tests\cli\test_cli_import_collection.py::test_external_native_artifact_plan_selects_module_attr_callable_exports tests\cli\test_cli_import_collection.py::test_external_native_artifact_plan_rejects_fake_module_attr_export tests\cli\test_cli_import_collection.py::test_external_native_artifact_plan_publishes_support_source_module_attr tests\cli\test_cli_import_collection.py::test_materialize_import_plan_adds_reachable_native_support_source_closure tests\cli\test_cli_extension_commands.py::test_extension_seal_rejects_fake_module_attr_callable_export -q`
- `uv run pytest tests\cli\test_cli_extension_commands.py::test_extension_seal_publishes_package_root_export_for_existing_static_artifact -q`
- `uv run pytest tests\cli\test_cli_import_collection.py::test_materialize_import_plan_adds_reachable_native_support_source_closure tests\cli\test_cli_import_collection.py::test_materialize_import_plan_rejects_missing_native_support_artifact tests\cli\test_cli_import_collection.py::test_native_support_source_stdlib_imports_join_compile_closure tests\cli\test_cli_import_collection.py::test_native_support_provider_prunes_unreachable_functions tests\cli\test_cli_import_collection.py::test_native_support_function_roots_cross_imported_helpers -q`
- `uv run ruff check src\molt\compiler_analysis\native_support_slice.py src\molt\cli\module_graph.py src\molt\cli\frontend_pipeline.py src\molt\cli\wrapper_build.py src\molt\frontend\visitors\statement_scope.py tests\cli\test_cli_import_collection.py`
- Queued `pact-witness-acceptance` with the sealed NumPy/SciPy staging roots fails closed with `scipy.ndimage._ni_label` missing source/artifact custody and the upstream source candidate `bench\friends\repos\scipy_off_the_shelf\scipy\ndimage\src\_ni_label.pyx`. Evidence remains in the queue log plus the prior `tmp\pact_build_probe_missing_native_custody.log` and `tmp\memory_guard\active\pact_missing_native_custody_build.json`.
- `WASI_SYSROOT=E:\molt-target\toolchains\wasi-sysroot-33.0+m uv run python -m molt extension build --project bench\friends\repos\scipy_off_the_shelf --out-dir tmp\pact_scipy_ni_label_molt_ext_wasm_cpython_abi --module scipy.ndimage._ni_label --target wasm --abi-tier cpython-abi --source-plan tmp\pact_scipy_ni_label_source_plan.json --source-plan-target _ni_label --source-plan-source-root bench\friends\repos\scipy_off_the_shelf --source-plan-build-root bench\friends\repos\scipy_off_the_shelf\build --source-plan-compile-commands tmp\pact_scipy_ni_label_compile_commands.json --capabilities core --python-export scipy.ndimage._ni_label --no-deterministic --json` passes and emits `scipy\ndimage\_ni_label.molt.wasm`, `object_count=1`, `linked_object_count=1`, `warnings=[]`, `errors=[]`. Evidence: `tmp\pact_scipy_ni_label_extension_build.log` and `tmp\memory_guard\active\pact_scipy_ni_label_extension_build.json`.
- `uv run python -c "from pathlib import Path; from molt.cli.external_native import _resolve_external_package_native_artifact_plan; root=Path('tmp/pact_scipy_ndimage_sealed_for_witness').resolve(); plan, errors=_resolve_external_package_native_artifact_plan(external_module_roots=(root,), admitted_packages={'scipy'}, required_modules={'scipy.ndimage.distance_transform_edt'}); print(plan is not None); print('\\n'.join(errors[:8]))"` fails fast with the new SciPy `_nd_image` module-attribute custody diagnostic for the stale high-level wrapper exports.
- `uv run python -m molt extension audit --path tmp\worktrees\pact-collab\tmp\pact_numpy_multiarray_molt_ext_wasm_cpython_abi --require-python-export numpy --json` fails fast with `missing_python_exports=["numpy"]` and the rebuild hint `molt extension build --python-export numpy`.
- `uv run python -m molt extension audit --path tmp\worktrees\pact-collab\tmp\pact_numpy_multiarray_molt_ext_wasm_cpython_abi --require-loader-kind libmolt_source --require-runtime-linkage static_link --require-artifact-kind wasm_relocatable_object --require-artifact-file --require-object-closure --require-checksum --json` passes standalone static-link artifact/hash/object-closure custody for the staged NumPy bytes, but still does not grant `numpy` import ownership.
- `uv run python -m molt extension audit --path bench\friends\repos\scipy_off_the_shelf\scipy\ndimage\_nd_image.molt.wasm.extension_manifest.json --require-loader-kind libmolt_source --require-runtime-linkage static_link --require-artifact-kind wasm_relocatable_object --require-artifact-file --require-object-closure --require-checksum --json` can still prove standalone static-link artifact/hash/object-closure custody for `_nd_image`, but it no longer proves high-level witness callable custody.
  The audited NumPy sidecar reports 130 closure objects, 583 runtime symbols,
  438 undefined symbols, and 56 defined symbols. The audited SciPy `_nd_image`
  sidecar reports 41 runtime symbols and 58 undefined symbols, but no closure
  digest/object list yet.

Unknown in this recovery:

- `uv run pytest tests\test_wasm_browser_embed.py::test_browser_embed_forward_roundtrips_float32_typed_arrays -q`

The latest export-custody proof:

- Queued `pact-witness-acceptance` with
  `MOLT_MODULE_ROOTS=tmp\worktrees\pact-collab\tmp\pact_numpy_multiarray_molt_ext_wasm_cpython_abi;bench\friends\repos\numpy_off_the_shelf;bench\friends\repos\scipy_off_the_shelf`
  and `MOLT_EXTERNAL_STATIC_PACKAGES="numpy scipy"` fails before graph
  expansion: the staged NumPy static-link artifact is present, but its manifest
  publishes no `python_exports` or `callable_exports`, so it cannot own the
  `numpy` import or any callable symbol. A focused resolver regression also
  rejects the subtler wrong sidecar where the artifact exports only
  `numpy._core._multiarray_umath` while the required import root is `numpy`.

Older unknown:

The browser proof entered a long WASM compile, showed live `cargo/rustc`
progress in read-only process snapshots, then the tool session disappeared
without a captured pytest footer. Treat it as not proven until rerun on a quiet
machine.

## Next Step

Start with Kernel A, not Kernel B. The aperture is the live `field_solve.py`
operation closure: `scipy.ndimage.distance_transform_edt`, `gaussian_filter`,
`maximum_filter`, `minimum_filter`, `label`, plus NumPy ndarray operations such
as `sort`, `argmax`, `percentile`, `where`, `lexsort`, `gradient`, `clip`,
`stack`, and `linalg.eigh`. The full rip is the package-native
object/symbol/storage closure needed to make those calls real in WASM without
host-CPython fallback and without Molt-owned NumPy/SciPy Python semantics. The
next executable primitive is reachable upstream extension custody: make the
NumPy/SciPy artifact publisher emit precise `python_exports`,
`provider_module`-backed `module_attr` callable exports, checksummed support
Python sources, and static-link artifacts for the wrapper-reachable native
extensions (`_nd_image`, `_ni_label`, `_rank_filter_1d`, then NumPy providers).
Stage only the native objects/symbols reachable from that operation closure,
scan their C/API gaps, bucket the gaps into shared primitives, and link/fail
closed before the browser parity lane runs.
