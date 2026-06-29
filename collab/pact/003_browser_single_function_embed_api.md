# 003 - DX: a minimal "call one compiled function from the browser" embed recipe

- **Priority:** medium
- **Kind:** developer experience / docs + narrow runtime path

## Ask
A documented, minimal path to: (a) ship `molt_runtime.wasm` plus a user module
compiled to wasm, and (b) from plain JS, call
`mod.forward(typedArray) -> typedArray` without standing up the full WASI
process host.

## Answer on `main` (2026-06-29)
- `wasm/browser_embed.js` is the single browser embed authority for the
  `typedArray -> typedArray` single-function path.
- Package-native `molt.forward_f32_v1` imports now lower as a typed WASM
  callable, `(input_ptr: i32, byte_len: i64, output_ptr: i32) -> i32`, and the
  browser adapter satisfies that import with `Float32Array` views over WASM
  memory. The old boxed `forward_f32_v1` native-call lane is gone.
- `examples/browser_embed_forward/forward.py` is the source dogfood function. It
  performs a real `Float32Array` numeric transform over byte-backed input and
  returns byte-backed output.
- `examples/browser_embed_forward/run_browser_embed_forward.mjs` is plain JS
  that imports a generated `browser_embed.js` URL and calls
  `forward(Float32Array) -> Float32Array`.
- `tests/test_wasm_browser_embed.py::test_browser_embed_forward_roundtrips_float32_typed_arrays`
  is the narrow proof lane. It builds the example through the existing
  split-runtime browser WASM path, serves the generated output directory, and
  calls it from Node without `wasm/browser_host.js`.
- No checked-in `examples/browser_embed_forward/artifacts/` package or copied
  embed loader remains. Prebuilt binary distribution is a release/artifact
  publishing problem, not a second in-repo embed lane.

## Recovery evidence (2026-06-29)
- The browser proof exposed a real WASM ABI manifest gap first:
  `molt_runtime_init` was a generated runtime export name but not a manifest
  runtime import alias. The manifest/generator path now admits lifecycle
  `runtime_name` aliases without classifying them as Python-callable runtime
  trampolines.
- The package-native browser ABI exposed a second live gap: `forward_f32_v1`
  still used the boxed object-call import type. The backend now imports native
  forward symbols through the typed pointer/length/output-pointer signature,
  emits runtime byte/scratch calls from the generated WASM ABI manifest, and
  `browser_embed.js` calls JS/native implementations with direct
  `Float32Array` memory views.
- The long source-build browser proof was interrupted/disappeared before a
  captured pytest result. Do not treat the browser proof as green until the
  pinned test is rerun on a quiet machine.
- The Pact witness kernel is blocked beyond this embed seam: plain WASM compile
  of `field_solve.py` fails on unsupported/linkable
  `scipy.ndimage.distance_transform_edt`; adding NumPy/SciPy source roots
  without package admission fails closed; package admission timed out after
  300s; and a graph-only probe took 100.4s, found 186 modules, zero staged
  native artifacts, and pulled broad NumPy plus `scipy`/`scipy.ndimage`
  initializer closure.

## What we found
- `wasm/browser_host.js` plus `wasm/run_wasm.js` are the full WASI-style process
  host: sockets, fds, errno, vfs, and process-shaped execution. That is the
  right host for running a browser program, but it is larger than the one-pure
  numeric-function visualization path.
- `wasm/*.wasm.sha256` files are integrity pins, not shipped runtime payloads.
  Downstream no-source-build consumption needs release-managed runtime artifacts
  with matching integrity metadata.

## Proposed
1. Keep the source-build browser embed example as the single in-repo lane:
   generated artifacts are outputs, not committed source.
2. Ship release-managed `molt_runtime.wasm` artifacts with integrity metadata so
   consumers can dogfood without local Rust/wasm-tools builds.
3. Move Kernel A through upstream package/native-artifact custody: SciPy/NumPy
   source admission, C/API symbol closure, ndarray/storage/buffer primitives,
   native extension staging, and tree-shaken reachable object closure.
4. Optional: a `browser_gpu_worker.js` example that runs a compiled compute
   kernel over a big array, so embarrassingly-parallel numeric kernels get the
   WebGPU path.

## Impact
The embed path removes the browser-host/process boundary for one compiled
numeric export. The remaining Pact blocker is not the JS call shape; it is the
NumPy/SciPy package-native closure needed for Kernel A.
