# 0970 Browser Numeric Kernel Embed

Status: active contract, 2026-06-29

## Contract

Browser consumers that want one pure numeric kernel should use the split-runtime
embed artifact, not the full process host:

- `wasm/browser_embed.js` owns the narrow `typedArray -> typedArray` callable
  kernel path for split-runtime artifacts.
- `loadMoltBrowserKernel(options)` compiles the manifest/runtime/app authority
  into a plain JavaScript `forward(Float32Array) -> Float32Array` call.
- `wasm/browser_host.js` remains the full browser process host for running a
  program, host IO, DB/socket/process shims, and `loadMoltWasm(...).invokeExport`
  escape-hatch calls. It does not own a duplicate typed-array kernel entrypoint.

For ordinary Python exports, typed arrays cross the host ABI as Python `bytes`.
The Python export should accept `bytes` plus explicit shape/length metadata,
then return `bytes`. JavaScript decodes those bytes into the requested
typed-array view.

For package-native browser callables, the manifest carries the executable ABI
token for each remaining `molt_native.<symbol>` import. `molt.object_call_v1`
is the positional boxed Molt-value ABI: the app imports a direct symbol as
`(i64...) -> i64`, `browser_embed.js` validates every argument/result as a Molt
value handle, and native implementations receive the same object handles the
WASM backend emits for `invoke_ffi`. `molt.object_callargs_v1` is the call
protocol ABI for keyword, star-arg, and C-extension style calls: the frontend
builds the canonical CallArgs object and the app imports the direct symbol as
`(callargs_bits: i64) -> i64`. `molt.forward_f32_v1` is a separate typed WASM
ABI for unary Float32Array kernels, not a boxed object ABI. It imports the
native symbol as `(input_ptr: i32, byte_len: i64, output_ptr: i32) -> i32`; the
backend uses the runtime byte/scratch authority (`bytes_as_ptr`,
`scratch_alloc`, `bytes_from_bytes`, `scratch_free`) to bridge a bytes-backed
Molt value to the native pointer/length call and then wraps the output buffer
once. `browser_embed.js` satisfies that typed import from plain JS by passing
`Float32Array` input/output views over WASM memory.

## Minimal Kernel

```python
# kernel.py
import struct


def forward(raw: bytes, n: int) -> bytes:
    xs = struct.unpack("<" + "f" * n, raw)
    ys = [x * 2.0 for x in xs]
    return struct.pack("<" + "f" * n, *ys)
```

Build the browser artifacts:

```bash
molt build kernel.py --target wasm --profile browser --wasm-profile pure --split-runtime --out-dir dist
```

Serve `dist/app.wasm`, `dist/molt_runtime.wasm`, `dist/manifest.json`,
`dist/browser_embed.js`, and `dist/loader_bridge.js` from the same origin, then
call the export:

```js
import { loadMoltBrowserKernel } from "./browser_embed.js";

const kernel = await loadMoltBrowserKernel({
  baseUrl: "./",
  exportName: "forward",
  resultType: "float32",
});

const input = new Float32Array([1.25, -2.5, 0, 4.75]);
const output = await kernel.forward(input, input.length);
```

`resultType` accepts `bytes`, `uint8`, `int8`, `uint16`, `int16`, `uint32`,
`int32`, `float32`, `float64`, `json`, `repr`, and `raw`.

## Not A Package Strategy

This embed fixture proves the browser ABI seam only. It is not a strategy for
reimplementing NumPy, SciPy, pandas, tinygrad, or hot numeric libraries in Molt
Python. Real package support must compile upstream package Python plus native
extension sources through Molt package/import custody, link source-recompiled
C/C++/Cython/Rust extension artifacts against `libmolt` and the Molt C/API
surface, then tree-shake to the user-reachable symbol and object closure.
Performance-critical operations must lower to typed storage, compiler IR, SIMD,
native codegen, or WebGPU/GPU kernels.

## WebGPU Route

Compiled Molt GPU kernels still use the process host, because WebGPU dispatch is
a host capability rather than the pure numeric embed surface:

```js
import { loadMoltWasm } from "./browser_host.js";

const host = await loadMoltWasm({
  wasmUrl: "./output.wasm",
  runtimeUrl: "./molt_runtime.wasm",
  preferLinked: false,
  env: { MOLT_GPU_BACKEND: "webgpu" },
});
```

The default browser path dispatches through `wasm/browser_gpu_worker.js` when a
blocking worker-capable host is available. Tests may inject
`gpuKernelDispatcher` to prove dispatch deterministically without requiring a
real browser GPU in CI.

## Artifact Distribution

Current source builds produce `molt_runtime.wasm` beside the app artifact. The
checked-in `wasm/*.wasm.sha256` files are integrity pins, not shipped runtime
payloads. A downstream no-source-build deployment needs a release artifact,
wheel payload, or CDN object that publishes `molt_runtime.wasm` with matching
integrity metadata. Until that release lane exists, the honest path is:

1. Build once on a machine allowed to run the Rust/WASM toolchain.
2. Publish `output.wasm` and `molt_runtime.wasm` together.
3. Keep `loader_bridge.js`, `browser_host.js`, and `browser_gpu_worker.js`
   version-matched with the runtime.

`--split-runtime` remains the cache-friendly deployment mode for larger apps:

```bash
molt build kernel.py --target wasm --profile browser --split-runtime --out-dir dist
```

That mode emits `app.wasm`, `molt_runtime.wasm`, `worker.js`,
`browser_embed.js`, `loader_bridge.js`, and `manifest.json`. The runtime module
is designed to stay CDN-cacheable across apps when its export surface is
unchanged.

## Package Compatibility Boundary

This browser entrypoint is intentionally narrower than NumPy/SciPy/tinygrad
support. It proves that a Molt-compiled export can accept and return typed-array
bytes in the browser. It does not claim package execution, ndarray semantics, or
third-party numeric coverage.

NumPy, SciPy, pandas, tinygrad, and similar packages must use the ecosystem
compatibility route: compile upstream package Python plus source-recompiled
native extension artifacts through Molt package/import custody; link against
`libmolt`, the Molt C/API surface, ndarray/storage primitives, buffer protocol,
capsule/module-state lifecycle, and per-target artifact staging; then
tree-shake to the user-reachable object and symbol closure. Missing behavior is
a shared primitive or a fail-closed diagnostic, not a Molt-owned Python package
surface.

For the Pact witness, this means the browser ABI proof here is only the loading
boundary. Kernel A/Kernel B package semantics remain out of scope until they are
compiled through upstream source plus ABI/C-API/object closure and lowered onto
typed storage, SIMD, native codegen, or WebGPU/GPU kernels.

Current Pact recovery evidence reinforces that boundary. The old first failure,
`scipy.ndimage.distance_transform_edt` becoming an unsupported fake Python
direct call, is retired for sidecar-declared `direct_symbol` native callable
exports: the witness `scipy.ndimage` operation closure now lowers to executable
`invoke_ffi` ABI metadata, including keyword filters through
`molt.object_callargs_v1`. NumPy/SciPy source roots without package admission
still fail closed. Package admission against the local Python 3.14
site-packages root now fails closed before graph expansion because the
installed NumPy/SciPy roots contain native markers but no wasm32 `static_link`
`libmolt_source` artifact manifests with package symbol custody; an earlier
package-admission probe timed out after 300s; and a graph-only probe found 186
modules, zero staged native artifacts, and broad NumPy plus
`scipy`/`scipy.ndimage` initializer closure. The remaining blocker belongs to
package-native object/symbol/storage, ndarray/buffer, and C/API primitive
closure, not to the browser typed-array embed API.

The producer-side custody path for those missing static-link artifacts now
exists: `molt extension build --target wasm` emits a wasm32 `.molt.wasm`
artifact with `runtime_linkage = "static_link"`,
`artifact_kind = "wasm_relocatable_object"`, binary-verified `direct_symbol`
callable exports, and `object_closure` symbol custody. The Pact blocker is that
the admitted NumPy/SciPy roots have not yet published reachable artifacts in
that shape.

Once those artifacts exist, replay Kernel A, `field_solve(lstar)`, first; it is
the SciPy ndimage stress test and interactive payload. The Molt-produced
candidate must pass `python check_parity.py candidate_outputs.npz`, then Kernel B
`witness_forward.levelset_argmax` must match
`witness_forward_reference.npz["lstar"]`. WASM-CPU remains the determinism
authority; WebGPU/WGSL/SIMD are speed lanes after the oracle path is green.

## Proof

Pinned tests:

- `tests/test_wasm_browser_embed.py::test_browser_embed_forward_roundtrips_float32_typed_arrays`
- `tests/test_wasm_browser_db_host.py::test_browser_host_direct_mode_can_invoke_export_with_host_args`
- `tests/test_wasm_browser_db_host.py::test_browser_host_direct_mode_can_invoke_export_with_host_args_split_runtime`
- `tests/test_wasm_browser_gpu_host.py::test_browser_host_direct_mode_compiled_gpu_kernel_uses_webgpu_dispatch`

The first test proves the pact-facing ABI: a JavaScript `Float32Array` reaches a
Molt split-runtime export as `bytes`, the export returns `bytes`, and
`browser_embed.js` decodes the result back into `Float32Array`.

Native callable exports use the same browser delivery path. A WASM app may
import a sidecar-declared `direct_symbol` through `molt.object_call_v1`,
`molt.object_callargs_v1`, or `molt.forward_f32_v1` as `molt_native.<symbol>`.
Linked WASM packaging stages
the reachable external `wasm_relocatable_object`/`static_archive` bytes into
`external_static_packages/<plan-digest>/`, passes those staged objects or
archives into `wasm-ld`, and fingerprints the staged artifact, sidecar manifest,
and support-file bytes for link reuse. Static-link artifacts resolved by
`wasm-ld` become app code and do not appear in
`abi.browser_embed.native_callables.symbols`; that manifest table is reserved
for remaining `app.wasm` `molt_native` imports that the browser host must
satisfy. Packaging filters the table to actual imports and fails closed when an
imported symbol is absent from the staged native artifact plan. `browser_embed.js`
requires packaged `molt_native` imports to be present in that manifest table and
rejects signature/token drift. For `molt.object_call_v1`, JS implementations
receive Molt i64 value handles and must return a Molt i64 value handle. For
`molt.object_callargs_v1`, JS implementations receive exactly one CallArgs i64
handle and must return a Molt i64 value handle. For `molt.forward_f32_v1`, JS
implementations receive `Float32Array` input and output views over WASM linear
memory and either fill the output view or return a same-length `Float32Array`
for the adapter to copy. There is no duplicate boxed `forward_f32_v1` lane.
Full NumPy/SciPy ndarray storage, dtype, stride, and multi-buffer custody remain
separate work.

Additional narrow proofs:

- `tests/test_wasm_browser_embed.py::test_browser_embed_forward_f32_native_callable_import_adapter`
- `tests/test_wasm_browser_embed.py::test_browser_embed_object_call_native_callable_import_adapter`
- `tests/test_wasm_browser_embed.py::test_browser_embed_object_call_native_callable_result_must_be_handle`
- `tests/test_wasm_browser_embed.py::test_browser_embed_object_callargs_native_callable_import_adapter`
- `cargo test -p molt-backend-wasm --features wasm-backend native_callable_direct_symbol_object_call_imports_and_directly_calls_symbol --lib`
- `cargo test -p molt-backend-wasm --features wasm-backend native_callable_direct_symbol_object_callargs_imports_and_directly_calls_symbol --lib`
- `cargo test -p molt-backend-wasm --features wasm-backend native_callable_forward_f32_imports_and_directly_calls_typed_payload_symbol --lib`

Recovery note, 2026-06-29: the native callable ABI and plain JS adapter proofs
are green. The source-build browser proof entered a long WASM compile and the
tool session disappeared before a captured pytest footer. The test remains the
pinned source-build proof, but this recovery does not claim it green.
