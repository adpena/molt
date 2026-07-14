# Molt runtime support matrix — {WASM-CPU, WebGPU} × {headless, browser}

Status: evidence-based, reviewed through reports 011/012 on 2026-07-14. Answers
the question Pact asked in `008 §4`, `009 P1`, and `010 W1(a)` so the collaboration can pick the
**contest-legal decode target** with confidence. Accuracy over optimism: this doc
gates a real decision, so every cell is graded against captured evidence (files,
tests, docs on disk), not intent. Where the honest answer is "infra exists but is
unproven," it says so.

## The matrix

Axes: **execution model** {WASM-CPU deterministic lane, WebGPU/WGSL compute lane}
× **environment** {headless (node / CI-class runner, no display, no browser),
browser (real `navigator.gpu` / DOM)}.

Verdict vocabulary (per pact's `008 §4` request): **supported** = a real
molt-compiled path runs with captured passing evidence; **needs-port** =
capability/infra is present and partially proven but a required proof or harness
is not yet captured; **blocked** = not reachable in that environment through the
molt WASM lane today.

|                | **headless (node / CI, no display)**                          | **browser (real `navigator.gpu`/DOM)**                         |
|----------------|---------------------------------------------------------------|----------------------------------------------------------------|
| **WASM-CPU**   | **supported** (substrate); witness (Kernel A) parity **in-flight** | **needs-port** (loaders shipped + node-proven; real-browser E2E automation not captured) |
| **WebGPU**     | **blocked** (no node WebGPU runtime; JS-mock dispatcher only) | **needs-port** (real WGSL shaders shipped; no captured on-GPU numeric/parity run) |

## Bottom line for the contest-legal target

**The contest-legal numeric decode target is WASM-CPU (or native), not WebGPU.**
The contest runs `inflate.sh` in a headless CI-class runner (no display, no
browser). WebGPU is unreachable there through molt's WASM lane: there is **no
node-side WebGPU binding anywhere in the repo** (no root `package.json`, no
`@webgpu/dawn` / `wgpu-native` / `node-webgpu` dependency), and the browser
WebGPU host code fails closed without `navigator.gpu`. WebGPU is therefore a
**showcase / speed-lane** target (browser or edge headless-Chrome), off the
contest-legal critical path - exactly the disposition `008 §4` anticipated.

This also honors the `008 §3` / `009 §3` axis rule: CPU and CUDA/GPU are separate
axes, neither inferred from the other. A WASM-CPU pass does not imply a WebGPU
pass; each cell is graded on its own captured evidence.

## Cell detail

### WASM-CPU × headless (node) — supported (substrate); witness parity in-flight

- **Substrate is real and proven.** The node WASM runner
  `wasm/run_wasm.js` + `wasm/loader_bridge.js` is the split-runtime witness
  execution path (`tools/pact_witness_acceptance.py` drives `field_solve.py`
  through it). A broad suite of node-run WASM tests passes
  (`tests/wasm_linked_runner.py` resolves `node`; `test_wasm_control_flow`,
  `test_wasm_string_ops`, `test_wasm_list_dict_ops`, `test_wasm_pipeline_e2e.py`,
  etc.). The slow numeric-forward roundtrip test exists at
  `tests/test_wasm_browser_embed.py::test_browser_embed_forward_roundtrips_float32_typed_arrays`,
  but the contract ledger does not contain a captured passing run; its presence
  proves the intended path, not completed `EMBED-API` acceptance.
- **Witness (Kernel A) parity is NOT green.** The `pact-witness-acceptance` lane
  has not produced a passing `candidate_outputs.npz`. The dated call-indirect
  failure in `STATUS.md` is historical; `PACT_CONTRACT_LEDGER.md` row `KA` and
  the proof queue own the current prerequisite. This is the P0 grind, not a
  matrix-cell substrate claim.
- **Honest caveat:** the PaddleOCR node harnesses (`tests/e2e/run_paddleocr_wasm.js`,
  `tests/e2e/bench_paddleocr_compiled.js`) prove **instantiation + primitive
  matmul throughput only**, not an OCR round-trip - the harness self-documents
  that a weight-loading path "is [not yet] wired." So node WASM-CPU is a proven
  numeric substrate; a full application inference round-trip in node is not yet
  captured.

### WASM-CPU × browser — needs-port (loaders shipped, node-proven; browser E2E uncaptured)

- **Loaders shipped:** `wasm/browser_host.js` (full WASI process host),
  `wasm/browser_embed.js` (narrow single-function embed), `wasm/browser_host.html`,
  `wasm/molt_vfs_browser.js`, and the `examples/browser_embed_forward/` sample
  (`forward.py` + `run_browser_embed_forward.mjs`, no process host).
- **Scoped under node, not a real browser.** The non-slow structural tests in
  `tests/test_wasm_browser_embed.py` execute under **node** (import-manifest
  parsers and native-callable adapters); the slow forward roundtrip remains
  uncaptured in the contract ledger. The only "browser" check
  (`tests/e2e/test_webgpu_correctness.py::TestBrowserTestPageDeployment`) `curl`s a
  deployed Cloudflare Worker for HTTP 200 - a page-serving check, not a compute
  test. No Playwright / Puppeteer / headless-Chrome automation drives molt WASM in
  a real browser.
- **The gate that is unmet** (`docs/design/foundation/71_wasm_webgpu_numeric_acceleration.md`,
  exit #13): "Browser E2E: the browser path runs in a real browser or
  browser-equivalent automation, **not only Node**." Verdict is **needs-port**: no
  missing capability, but the real-browser acceptance proof is not captured. This
  is the same done-but-unproven status the `EMBED-API` obligation carries.

### WebGPU × headless (node / CI) — blocked

- **No node WebGPU runtime exists.** Grep for `@webgpu/dawn` / `wgpu-native` /
  `node-webgpu` returns zero; there is no root `package.json` GPU binding. Headless
  node cannot obtain a WebGPU device.
- **The browser GPU host fails closed without a browser.** `wasm/browser_gpu_worker.js`
  and `wasm/browser_gpu_dispatch.js` throw `"navigator.gpu is unavailable..."` when
  there is no browser GPU (and additionally require `SharedArrayBuffer` +
  `Atomics.wait` + `Worker`). `tests/test_wasm_browser_gpu_host.py` builds **real**
  molt GPU programs (vector_add, `x.linear`, tinygrad `nn.Linear`, attention) and
  runs them in node - but every test **injects a pure-JS `dispatchKernel` mock**;
  it proves the WASM->host dispatch plumbing (WGSL emission, binding layout,
  grid/workgroup), not GPU execution. The unmocked path
  (`..._without_webgpu_fails_fast`) asserts it fails closed.
- **A native Rust `wgpu` adapter exists but is a different lane.**
  `runtime/molt-gpu/src/device/webgpu.rs` (`wgpu` crate v29, feature
  `webgpu-backend`) can request a native adapter (Vulkan/Metal/Dx12/GL, possibly
  software) - but it is the **native runtime** path, has **zero `#[cfg(test)]`
  coverage**, and is adapter-dependent; it is not the node/CI-WASM contest lane.
- **Doc authority** (`71_...md`): "A JavaScript-only fake dispatcher is CI
  scaffolding, never acceptance for a WebGPU claim." `docs/architecture/webgpu-inference-roadmap.md`
  §1: "**Current State: CPU-Only WASM** ... All computation runs on the CPU via
  the WASM linear memory interpreter." Verdict: **blocked** for the headless WASM
  lane. (Headless-Chrome-with-WebGPU exists as a Cloudflare edge deploy -
  `deploy/cloudflare/worker.js` - but that is a real browser engine on the edge,
  not a display-less node runner, and has no captured automated parity test.)

### WebGPU × browser — needs-port (shaders shipped; execution/parity uncaptured)

- **Real WGSL compute shaders shipped.** `deploy/browser/webgpu-engine.js` carries
  `MATMUL_WGSL` (tiled 16×16, `@workgroup_size(16,16,1)`, workgroup memory,
  `fma()`), `CONV2D_WGSL` (16×16 workgroup, `fma()`), plus
  `SOFTMAX`/`RMSNORM`/`ROPE`/`ADD`/`MUL` WGSL; standalone PoC
  `deploy/browser/webgpu-matmul.js` (`@compute`, `requestAdapter`/`requestDevice`);
  WGSL renderer `runtime/molt-gpu/src/render/wgsl.rs`.
- **Tests are structural, not on-GPU.** `tests/e2e/test_webgpu_correctness.py`
  asserts the shader **source strings** (`TILE_SIZE = 16`, `@workgroup_size`,
  binding declarations, entry-point names); `tests/e2e/bench_webgpu_conv2d.js`
  regex-extracts the WGSL as text and then benchmarks a **CPU** Conv2d. No numeric
  output, no CPU-reference parity, no cross-vendor run is captured.
- **Acceptance target (not a measured result),** per `010` W1(b): "cross-vendor GPU
  fp32 bit-exactness is hard - do NOT promise it. Aim for
  **deterministic-per-device** on WebGPU and reserve bit-exact for the WASM-CPU
  authority lane." Verdict: **needs-port** - shaders and browser host code exist,
  real-GPU execution would work, but a passing on-GPU numeric/parity row is not
  captured. Do not claim WebGPU parity until `71_...md` exit #7 (a real WGSL
  dispatch/parity row from a compiled molt GPU kernel) is green.
- **Clarification (do not conflate):** `deploy/browser/simd-ops-rs/src/lib.rs`
  `matmul_f32_tiled` is a **WASM-SIMD CPU** matmul (`f32x4`, 4×4 register tiling),
  not WebGPU. The GPU tiled matmul is `MATMUL_WGSL`.

## Related capabilities referenced by the work package

- **WebCodecs** (the `010` W2 in-browser FLOW transport): **planned only.** The
  sole repo reference is the `010` planning doc; no `VideoDecoder`/`VideoFrame`/
  `EncodedVideoChunk` exists in any runtime or JS code. W2 is blocked on both the
  WebGPU-browser lane and a WebCodecs integration that does not exist yet.
- **Target feature manifest.** `wasm/target_feature_manifest.json` declares the
  `wasm-browser-webgpu` row and the `navigator.gpu` adapter probe; split-runtime
  browser packages select it from the WebGPU dispatch host. This is capability
  *declaration*, consistent with the cells above - declaration is not execution
  proof.

## What would move each cell to "supported"

- **WASM-CPU × headless -> fully supported for the witness:** complete the
  current package/runtime prerequisites, produce all 11 candidate outputs, and
  pass the canonical manifest-driven parity engine (the live P0).
- **WASM-CPU × browser:** add a real-browser (Playwright/headless-Chrome)
  automation that loads a molt WASM build and asserts a numeric result + the
  pinned `EMBED-API` roundtrip test captured green on a quiet machine.
- **WebGPU × browser:** capture one real on-GPU WGSL dispatch with a
  deterministic-per-device numeric parity row vs the numpy-fp32 reference for a
  compiled molt kernel (`71_...md` exit #7), with documented vendor variance.
- **WebGPU × headless:** would require either a node WebGPU binding
  (`@webgpu/dawn`/`wgpu-native`) or promoting the native Rust `wgpu` path with a
  software adapter under test - neither is the contest lane, so this stays a
  showcase/native concern, not a contest-legal requirement.

## Evidence sources

- Loaders / node lane: `wasm/run_wasm.js`, `wasm/loader_bridge.js`,
  `wasm/browser_host.js`, `wasm/browser_embed.js`, `tests/wasm_linked_runner.py`.
- WASM-CPU tests: `tests/test_wasm_browser_embed.py`, `tests/test_wasm_pipeline_e2e.py`.
- GPU host plumbing (mock-dispatched): `tests/test_wasm_browser_gpu_host.py`,
  `wasm/browser_gpu_worker.js`, `wasm/browser_gpu_dispatch.js`.
- Shaders: `deploy/browser/webgpu-engine.js`, `deploy/browser/webgpu-matmul.js`,
  `runtime/molt-gpu/src/render/wgsl.rs`; native adapter
  `runtime/molt-gpu/src/device/webgpu.rs`.
- Structural-only WebGPU tests: `tests/e2e/test_webgpu_correctness.py`,
  `tests/e2e/bench_webgpu_conv2d.js`.
- Authorities: `docs/design/foundation/71_wasm_webgpu_numeric_acceleration.md`
  (exit criteria #7, #13; "fake dispatcher is CI scaffolding"),
  `docs/architecture/webgpu-inference-roadmap.md` ("Current State: CPU-Only WASM"),
  `collab/pact/010_*.md` (deterministic-per-device acceptance),
  `collab/pact/008_*.md` §4 (the original open question).
- Witness frontier: `docs/agent/PACT_CONTRACT_LEDGER.md` row `KA`, the proof
  queue, and current logs. `collab/pact/STATUS.md` is dated history.
