# STATUS - Pact dogfooding of Molt (2026-06-29)

Positive signal first: the browser call shape is no longer the core unknown.
`wasm/browser_embed.js` owns the narrow split-runtime embed path, and
`examples/browser_embed_forward/` now contains a source-level numeric
`forward(Float32Array) -> Float32Array` example plus a plain JS runner that
consumes a generated build directory. The full `wasm/browser_host.js` process
host remains separate.

No checked-in `examples/browser_embed_forward/artifacts/` bundle remains. A
committed prebuilt runtime/app package would be a second artifact lane and would
rot unless it is managed by a release/integrity pipeline. Generated WASM remains
an output.

## Current Blocker

The live evidence says the current tree cannot yet produce
`candidate_outputs.npz` for the Pact witness:

- The old first failure, `scipy.ndimage.distance_transform_edt` becoming an
  unsupported fake Python direct call, is retired for manifest-declared native
  callable exports. `known_modules` remains import visibility only, while
  direct-symbol callable exports now lower the `scipy.ndimage` witness closure
  to executable `invoke_ffi` ABI metadata.
- The current remaining blocker is downstream of that lowering: the live Pact
  build still lacks admitted reachable NumPy/SciPy native artifacts,
  ndarray/storage/dtype/buffer truth, and the C/API primitive closure needed to
  link and execute those upstream extension symbols in WASM.
- Adding NumPy/SciPy source roots without package admission correctly fails
  closed.
- Adding package admission against the local Python 3.14 site-packages root now
  fails closed before graph expansion: NumPy/SciPy contain native
  source/artifact markers but do not publish wasm32 `static_link`
  `libmolt_source` artifact manifests with package symbol custody. Source roots
  alone are not linkable WASM evidence.
- An earlier package-admission probe timed out after 300s in the live WASM build
  path.
- A graph-only probe took 100.4s before backend work, found 186 modules, zero
  staged native artifacts, and pulled broad NumPy plus `scipy` and
  `scipy.ndimage` package initializer closure.

That means the next real structural unit is upstream package-native closure:
NumPy/SciPy source admission, native artifact staging, C/API symbol closure,
ndarray/storage/buffer primitives, capsule/module-state lifecycle, and
tree-shaken reachable object closure. Molt-owned Python shims for NumPy/SciPy
would be the wrong architecture.

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

Green in this recovery:

- `uv run ruff check src/molt/native_callable_abi.py src/molt/frontend/visitors/call_module_dispatch.py src/molt/frontend/visitors/statement_scope.py tests/cli/test_cli_import_collection.py tests/test_wasm_browser_embed.py`
- `uv run pytest tests/cli/test_cli_import_collection.py::test_frontend_pact_ndimage_operation_closure_lowers_to_native_abi -q`
- `uv run pytest tests/cli/test_cli_import_collection.py::test_external_native_artifact_plan_rejects_unknown_callable_export_abi tests/cli/test_cli_import_collection.py::test_frontend_native_callable_callargs_export_lowers_keyword_child_module_attr tests/cli/test_cli_import_collection.py::test_browser_native_callable_manifest_is_import_driven tests/test_wasm_browser_embed.py::test_browser_embed_object_callargs_native_callable_import_adapter -q`
- `cargo test -p molt-ir native_callable_abi_contracts_are_canonical --lib`
- `cargo test -p molt-backend-wasm --features wasm-backend native_callable_direct_symbol_object_callargs_imports_and_directly_calls_symbol --lib`
- `uv run python -m molt build collab\pact\pact_witness_kernel\field_solve.py --target wasm --profile browser --wasm-profile auto --split-runtime --out-dir tmp\pact_plain_build_probe_after_native_callable` fails closed at `distance_transform_edt` when no package/native callable metadata is admitted.
- `MOLT_MODULE_ROOTS=<Python 3.14 site-packages> MOLT_EXTERNAL_STATIC_PACKAGES="numpy scipy" uv run python -m molt build collab\pact\pact_witness_kernel\field_solve.py --target wasm --profile browser --wasm-profile auto --split-runtime --out-dir tmp\pact_package_admission_probe_after_native_callable` fails closed before graph expansion because no wasm32 `static_link` `libmolt_source` artifact manifests exist for the admitted NumPy/SciPy roots.
- `uv run pytest tests/cli/test_cli_extension_commands.py::test_extension_build_wasm_target_emits_static_link_artifact_and_manifest tests/cli/test_cli_extension_commands.py::test_extension_build_wasm_target_rejects_missing_direct_symbol tests/cli/test_cli_extension_commands.py::test_extension_build_cross_target_uses_target_compiler_and_manifest tests/cli/test_cli_extension_commands.py::test_extension_build_emits_public_exports_in_manifest tests/cli/test_cli_extension_commands.py::test_extension_build_emits_wheel_and_manifest -q`
- `uv run pytest tests/cli/test_cli_import_collection.py::test_external_static_package_wasm_artifact_plan_is_manifest_led tests/cli/test_cli_import_collection.py::test_source_recompiled_static_package_requires_native_artifact_candidate_pregraph tests/cli/test_cli_import_collection.py::test_admitted_external_native_package_does_not_close_source_only_ndimage_initializers tests/cli/test_cli_import_collection.py::test_external_native_artifact_plan_rejects_missing_wasm_callable_symbol tests/cli/test_cli_import_collection.py::test_external_native_artifact_plan_selects_callable_exported_imports -q`
- `uv run pytest tests/cli/test_cli_import_collection.py::test_frontend_pact_ndimage_operation_closure_lowers_to_native_abi tests/cli/test_cli_import_collection.py::test_browser_native_callable_manifest_is_import_driven tests/test_wasm_browser_embed.py::test_browser_embed_object_callargs_native_callable_import_adapter tests/test_wasm_browser_embed.py::test_browser_embed_native_callable_import_must_be_manifest_declared -q`
- `MOLT_MODULE_ROOTS=<Python 3.14 site-packages> MOLT_EXTERNAL_STATIC_PACKAGES="numpy scipy" uv run python -m molt build collab\pact\pact_witness_kernel\field_solve.py --target wasm --profile browser --wasm-profile auto --split-runtime --out-dir tmp\pact_package_admission_probe_after_wasm_extension_build` fails closed before graph expansion because the installed NumPy/SciPy roots still have no wasm32 `static_link` `libmolt_source` artifact manifests.
- `cargo test -p molt-lang-cpython-abi pycomplex_binary_exports_fail_closed_until_bridge_storage_exists --lib`
- `uv run pytest tests/cli/test_cli_extension_commands.py::test_cpython_abi_variadic_shim_does_not_export_header_inline_stubs -q`
- `uv run pytest tests\test_gen_wasm_abi.py::test_wasm_abi_manifest_owns_lir_runtime_calls tests\test_gen_wasm_abi.py::test_wasm_abi_manifest_owns_runtime_export_policy -q`
- `uv run python tools\gen_wasm_abi.py --check`
- `uv run pytest tests\test_wasm_browser_embed.py::test_browser_embed_forward_f32_native_callable_import_adapter -q`
- `cargo check -p molt-backend-wasm --features wasm-backend`
- `cargo test -p molt-backend-wasm --features wasm-backend native_callable_forward_f32_imports_and_directly_calls_typed_payload_symbol --lib`

Unknown in this recovery:

- `uv run pytest tests\test_wasm_browser_embed.py::test_browser_embed_forward_roundtrips_float32_typed_arrays -q`

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
next executable primitive is reachable upstream extension custody: stage only
the native objects/symbols reachable from that operation closure, scan their
C/API gaps, bucket the gaps into shared primitives, and link/fail closed before
the browser parity lane runs.
