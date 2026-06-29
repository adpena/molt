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

- Plain WASM compile of `pact_witness_kernel/field_solve.py` fails at
  `scipy.ndimage.distance_transform_edt`; it is not a supported/linkable direct
  call.
- Adding NumPy/SciPy source roots without package admission correctly fails
  closed.
- Adding package admission timed out after 300s in the live WASM build path.
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

## Proof Status

Green in this recovery:

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
host-CPython fallback and without Molt-owned NumPy/SciPy Python semantics.
