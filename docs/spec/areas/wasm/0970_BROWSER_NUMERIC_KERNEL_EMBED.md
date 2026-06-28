# 0970 Browser Numeric Kernel Embed

Status: active contract, 2026-06-27

## Contract

Browser consumers that want one pure numeric kernel should use the existing
browser host authority, not a second hand-written WebAssembly loader:

- `wasm/browser_host.js` owns browser instantiation, split-runtime wiring,
  runtime exceptions, host imports, and WebGPU dispatch.
- `loadMoltKernel(options)` is the small callable facade for
  `typedArray -> typedArray` use cases.
- `loadMoltWasm(options).invokeExport(name, args)` remains the lower-level
  escape hatch for structured Molt object calls.

Typed arrays cross the host ABI as Python `bytes`. The Python export should
accept `bytes` plus explicit shape/length metadata, then return `bytes`.
JavaScript decodes those bytes into the requested typed-array view.

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
molt build kernel.py --target wasm --profile browser --out-dir dist
```

Serve `dist/output.wasm`, `dist/molt_runtime.wasm`, and
`wasm/browser_host.js` from the same origin, then call the export:

```js
import { loadMoltKernel } from "./browser_host.js";

const kernel = await loadMoltKernel({
  wasmUrl: "./output.wasm",
  runtimeUrl: "./molt_runtime.wasm",
  preferLinked: false,
  exportName: "kernel__forward",
  resultType: "float32",
});

const input = new Float32Array([1.25, -2.5, 0, 4.75]);
const output = await kernel.forward(input, input.length);
```

`resultType` accepts `bytes`, `uint8`, `int8`, `uint16`, `int16`, `uint32`,
`int32`, `float32`, `float64`, `json`, `repr`, and `raw`.

## WebGPU Route

Compiled Molt GPU kernels use the same host:

```js
const kernel = await loadMoltKernel({
  wasmUrl: "./output.wasm",
  runtimeUrl: "./molt_runtime.wasm",
  preferLinked: false,
  env: { MOLT_GPU_BACKEND: "webgpu" },
  exportName: "kernel__forward",
  resultType: "float32",
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
3. Keep `browser_host.js` and `browser_gpu_worker.js` version-matched with the
   runtime.

`--split-runtime` remains the cache-friendly deployment mode for larger apps:

```bash
molt build kernel.py --target wasm --profile browser --split-runtime --out-dir dist
```

That mode emits `app.wasm`, `molt_runtime.wasm`, `worker.js`, and
`manifest.json`. The runtime module is designed to stay CDN-cacheable across
apps when its export surface is unchanged.

## Numeric Coverage Truth Table

This table answers the pact witness question directly. "Green" means there is
repo-backed implementation and proof for the named source API on that target.

Separate layer: the pinned upstream C-API source scans are green on this branch
for NumPy and SciPy. NumPy scanned 447 source files / 1,258 required symbols
with zero missing or fail-fast; SciPy scanned 592 source files / 321 required
symbols with zero missing or fail-fast. These suites are `c_api_probe` evidence:
they prove missing-symbol closure and source-boundary scanning, not package
build, link, import, or runtime execution. The table below is about unchanged
high-level NumPy/SciPy runtime execution on Molt/WASM for the Pact witness
operations, not missing-symbol closure.

| Operation | NumPy/SciPy native | NumPy/SciPy WASM | NumPy/SciPy WASM+SIMD | Molt Tensor/tinygrad path |
|---|---:|---:|---:|---|
| `A @ B` 2D float32 matmul | Not green | Not green | Not green | Present through Tensor/tinygrad composition and GPU runtime primitives; exact pact WASM smoke should be added per workload. |
| `sin`, `cos`, `tanh`, `exp` | Not green | Not green | Not green | Present on Tensor/tinygrad; `cos` is composed from `sin`. |
| `clip` | Not green | Not green | Not green | Express as Tensor clamp/min/max composition; no NumPy `np.clip` surface. |
| `argmax(axis=-1)` | Not green | Not green | Not green | Tensor `argmax(axis)` exists; host-realized today in the Python shim. |
| `max(axis=-1, keepdims=True)` | Not green | Not green | Not green | Tensor reductions keep the reduced dimension as size 1. |
| Broadcasting | Not green | Not green | Not green | Tensor broadcasting is implemented with ShapeTracker/runtime broadcast paths. |
| `concatenate`, `reshape`, `transpose` | Not green | Not green | Not green | Tensor `cat`, `reshape`, and `permute` exist. |
| `scipy.ndimage.distance_transform_edt` | Partial | Not green | Not green | Source-owned exact 2D boolean/unit-sampling lower-envelope implementation is present and SciPy-reference-tested for Pact masks; Kernel A still needs reflect-mode filters, 4-connectivity label, percentile/top-k ordering, gradient, and deterministic 2x2 eigensolve before WASM parity is green. |

The current Pact handoff now supplies an exact runnable bundle under
`collab/pact/pact_witness_kernel/`. The correct near-term browser route is
therefore not "ship an unchanged NumPy/SciPy WASM runtime today." Source C-API
closure is green; package build/import/runtime custody and SciPy ndimage
semantics are separate remaining layers. The Pact route is:

1. Compile Kernel A, `field_solve(lstar)`, first. It is the SciPy ndimage
   stress-test and interactive payload.
2. Run the Molt-produced candidate on `lstar_sample.npz` and pass
   `python check_parity.py candidate_outputs.npz`.
3. Compile Kernel B, `witness_forward.levelset_argmax`, and match
   `witness_forward_reference.npz["lstar"]` exactly.
4. Keep WASM-CPU as the determinism authority and use WebGPU/WGSL/SIMD as the
   speed lane once the oracle path is green.

## Proof

Pinned tests:

- `tests/test_wasm_browser_db_host.py::test_browser_kernel_facade_roundtrips_float32_typed_arrays`
- `tests/test_wasm_browser_db_host.py::test_browser_host_direct_mode_can_invoke_export_with_host_args`
- `tests/test_wasm_browser_db_host.py::test_browser_host_direct_mode_can_invoke_export_with_host_args_split_runtime`
- `tests/test_wasm_browser_gpu_host.py::test_browser_host_direct_mode_compiled_gpu_kernel_uses_webgpu_dispatch`

The first test proves the pact-facing ABI: a JavaScript `Float32Array` reaches a
Molt export as `bytes`, the export returns `bytes`, and the browser facade
decodes the result back into `Float32Array`.
