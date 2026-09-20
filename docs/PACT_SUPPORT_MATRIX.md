# Pact runtime support matrix

This matrix distinguishes implemented Molt runtime infrastructure from the
evidence required to claim that the Pact workload works in a cell. It is a
stable capability map, not a dated status report. Current execution state and
artifacts belong to the proof queue.

## Claim vocabulary

- **Implemented**: Molt contains a real execution path for the cell. A Pact
  claim still requires Molt-produced output from the real Pact workload in that
  exact cell, accepted against independent reference outputs.
- **Per-device**: the path depends on a concrete GPU adapter and may be claimed
  only for the tested device, driver, browser, and shader set.
- **Unavailable**: the required runtime binding is absent. Mock dispatch or a
  different runtime family does not promote the cell.

## Capability and evidence matrix

| execution model | headless Node / CI | real browser |
|---|---|---|
| WASM-CPU | **Implemented.** The Node and Molt WASM hosts execute the CPU runtime. A Pact claim requires all 11 Kernel A outputs produced by Molt in this lane to pass the canonical gate manifest against the independent reference. | **Implemented.** `tools/wasm_run_matrix.py` drives the browser host in headless Chromium through Puppeteer or Playwright. A Pact claim requires Molt-produced witness output from that browser cell and parity against the independent reference; generic browser automation is not that proof. |
| WebGPU | **Unavailable in Node.** The repository has no `@webgpu/dawn`, `wgpu-native`, or `node-webgpu` binding. JavaScript mock dispatch proves plumbing only. Native Rust `wgpu` and Chromium are different environments. | **Per-device infrastructure.** Browser GPU hosts and WGSL kernels exist, but a Pact claim requires an unmocked adapter run plus witness-specific numeric/parity evidence for the named device, driver, and browser. |

## Cell contracts

### WASM-CPU in headless Node or CI

`wasm/run_wasm.js`, `wasm/loader_bridge.js`, and the Molt WASM host provide the
headless CPU substrate. This is the contest-compatible portable lane because it
does not require a display or browser GPU. Infrastructure tests, module
instantiation, and primitive numeric examples establish the substrate only.
They do not establish Pact acceptance. The claim becomes valid only when the
real package-native Kernel A run produces `candidate_outputs.npz` and the
canonical parity command passes all 11 outputs.

### WASM-CPU in a real browser

`wasm/browser_host.html`, `wasm/browser_host.js`, `wasm/browser_embed.js`, and
the browser asset closure provide the browser CPU path. The `browser` lane in
`tools/wasm_run_matrix.py` detects Puppeteer or Playwright, serves the real
browser harness over HTTP, captures page output, and compares it with the
expected output. This closes the old infrastructure gap where browser-shaped
tests ran only under Node. It does not, by itself, prove the Pact witness in a
browser; that claim needs a witness-specific browser run and accepted outputs.

### WebGPU in headless Node or CI

There is no Node-side WebGPU binding in the repository. Browser GPU host code
fails closed when `navigator.gpu` is unavailable. Tests that inject a
JavaScript dispatcher validate WASM-to-host dispatch structure, WGSL emission,
bindings, and workgroup geometry, but do not execute a GPU and cannot support a
WebGPU claim.

The Rust `wgpu` adapter is a native-runtime lane, not a Node binding. Likewise,
headless Chromium is the real-browser column even when it runs without a
visible display.

### WebGPU in a real browser

Molt ships browser GPU host plumbing and real WGSL compute kernels. Promotion
from infrastructure to a Pact claim requires execution on an actual adapter,
readback of real kernel results, and comparison against the canonical Pact
oracle. Evidence must identify the device, driver, browser, shader set, inputs,
and gate manifest. Source inspection, shader compilation, mock dispatch, page
deployment, or a WASM-CPU result is insufficient.

WebGPU is deterministic per named device configuration, not presumed
bit-identical across vendors. It remains a speed/showcase lane; WASM-CPU or
native CPU remains the deterministic contest authority unless a separately
available contest GPU runtime is proved on the actual headless target.

## Cross-cell rules

- Native, WASM, headless, browser, CPU, and GPU cells do not inherit evidence
  from one another.
- Package/source custody and successful compilation are prerequisites, not
  parity results.
- Mock, host fallback, Pyodide, host-CPython, and Molt-owned NumPy/SciPy
  semantic substitutes cannot satisfy a cell.
- The accepted output schema and tolerance policy come only from the Pact gate
  manifest and the shared parity engine documented in
  [`collab/pact/README.md`](../collab/pact/README.md).
