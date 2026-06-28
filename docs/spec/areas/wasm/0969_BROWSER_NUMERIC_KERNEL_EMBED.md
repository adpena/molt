# Browser Numeric Kernel Embed
**Spec ID:** 0969
**Status:** Implemented narrow path
**Audience:** browser visualization authors, WASM runtime engineers

## Contract

`wasm/browser_embed.js` is the browser embed authority for one compiled numeric
kernel. It loads the split-runtime artifacts produced by:

```bash
molt build kernel.py --target wasm --profile browser --wasm-profile pure --split-runtime --out-dir dist/kernel
```

The build stages:

- `app.wasm`
- `molt_runtime.wasm`
- `manifest.json`
- `browser_embed.js`

`manifest.json` owns the embed ABI metadata. `abi.browser_embed` records the
generated call-indirect import family, runtime-import fallback specs, and table
layout constants consumed by the loader. `browser_embed.js` must reject a
manifest that lacks those generated facts rather than carrying a parallel copy
of the WASM ABI.

The plain-JS call surface is:

```js
import { loadMoltBrowserKernel } from "./browser_embed.js";

const kernel = await loadMoltBrowserKernel({ baseUrl: "./" });
const output = await kernel.forward(new Float32Array([1.25, -2.5, 0, 4.75]));
```

`forward(Float32Array) -> Float32Array` is a JS-facing typed-array contract. The
compiled Molt export remains the existing Python-object export ABI. The embed
loader owns the narrow bridge: it passes the typed array as a Molt `bytes`
argument and decodes a returned Molt `bytes` object back into a typed array.

## Kernel Shape

Use a pure numeric export that accepts and returns packed little-endian bytes:

```python
from array import array

def forward(raw: bytes):
    values = array("f")
    values.frombytes(raw)
    out = array("f")
    for value in values:
        out.append(value * 1.5 + 0.25)
    return out.tobytes()
```

This keeps the browser ABI stable while the compiler continues to own the
Python function export ABI. Higher-level pact kernels can pack coordinates,
weights, or code vectors into the input buffer without introducing a second
browser export lane.

## Boundary

`browser_embed.js` is not a process host. It fails closed for VFS, process,
socket, WebSocket, GPU dispatch, and broad WASI calls. Use
`wasm/browser_host.js` for "run this Python program in a browser" workflows.
Use `browser_embed.js` for "call this pure numeric function from a browser
visualization" workflows.

The focused proof is
`tests/test_wasm_browser_embed.py::test_browser_embed_forward_roundtrips_float32_typed_arrays`.
