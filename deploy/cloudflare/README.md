# OCR Worker and compiled model builds

The production entry point is `worker.js`. Its `/ocr/paddle-molt` route currently
executes the JavaScript ONNX interpreter in `ocr_api.js`. Its local Falcon
initializer selects JavaScript CPU models; the `wasmInstance` variable is never
assigned. Route names, status responses and historical size comments therefore
do not prove that either model runs through Molt WebAssembly.

## Local model builds

Start in a managed checkout based on `origin/main`. Record `git rev-parse HEAD`
and `git status --short` with the build receipt. Set `MOLT_PYTHON` to the selected
slot development image's `bin/python`, and `MOLT_OCR_BUILD_DIR` to an owned
directory under the scratch root reported by `molt dx env`. Use that interpreter
with `-B` for every Python command; do not replace or synchronize a shared image.

Record `"$MOLT_PYTHON" -B -m molt.cli --version` together with the source commit,
selected image and toolchain identities to identify a particular build.

Paddle's entry module is `demos/tinygrad/paddleocr_driver.py`:

```sh
"$MOLT_PYTHON" -B -m molt.cli dx run -- \
  "$MOLT_PYTHON" -B -m molt.cli build \
  --module demos.tinygrad.paddleocr_driver --target wasm \
  --release --wasm-opt-level O3 --linked --require-linked \
  --out-dir "$MOLT_OCR_BUILD_DIR/paddle" --json
```

Falcon's entry module is `demos/tinygrad/wasm_driver.py`, which imports
`demos/tinygrad/examples/falcon_ocr.py`:

```sh
"$MOLT_PYTHON" -B -m molt.cli dx run -- \
  "$MOLT_PYTHON" -B -m molt.cli build \
  --module demos.tinygrad.wasm_driver --target wasm \
  --release --wasm-opt-level O3 --linked --require-linked \
  --capabilities env.read --out-dir "$MOLT_OCR_BUILD_DIR/falcon" --json
```

Use the generated `manifest.json` to select the linked artifact, its dependencies
and its app export bindings. `paddleocr.wasm` and `falcon-ocr.wasm` are serving
names, not sufficient artifact identities. Preserve the manifest and its
declared files together. A successful build alone does not prove that module
initialization, model loading or OCR works.

`demos/tinygrad/wasm_manifest.json` records the historical Falcon command but no
compiler commit or artifact checksum. `deploy/scripts/build_wasm.sh` builds GPU
primitives and prints the Python build command; it does not build the full Python
model. `deploy/scripts/deploy.sh` combines a build with uploads and deployment,
so it is not a local build or measurement command.

Paddle's driver still imports `tinygrad.lazy.LazyOp` and `LazyBuffer`; the public
compatibility surface under `src/tinygrad` does not provide that module. Do not
make production builds resolve through `demos/tinygrad/reference_stdlib`: that
tree is quarantined for research and reference tests. The demo compilation and
parity obligation is tracked by HF-351 in the
[V1 ledger](../../docs/agent/V1_HANDOFF_FINDINGS.md). Falcon's remaining module-local
lifetime obligation is HF-548; the earlier WASM nesting repair is recorded under
HF-546 in that same authority.

## Qualification

Build for correctness and performance first: release plus a speed optimization
level such as `O3`. The compiler's default release optimization is currently
`Oz`, so specify the speed level explicitly. Any change that trades performance
for memory or size, including quantization, a smaller model or deferred loading,
requires the owner's decision with measurements of both choices.

For each exact artifact, retain uncompressed artifact and complete bundle sizes,
instantiation time, first-call time, peak linear memory and CPU on one fixed
image with independently expected text. Identify the image, real weight files,
configuration and tokenizer by digest. Stub weights and JavaScript inference do
not establish compiled real-model OCR. Measure module startup separately from
model initialization and request handling.

[Cloudflare's limits](https://developers.cloudflare.com/workers/platform/limits/)
are 64 MiB for the complete uncompressed Worker bundle, 1 second to parse and
execute global scope, and 128 MB per isolate including JavaScript and WebAssembly
allocations. The paid CPU ceiling is 300,000 ms, selected by this Worker's
`limits.cpu_ms`. A `memory_mb = 256` setting does not establish a larger documented
isolate allowance. Local Node timings do not establish Cloudflare startup or CPU
acceptance. The owner's additional size ceiling must also be recorded before
claiming acceptance.

Stop a model that fails to build, initialize or fit; preserve the exact failure
and its owning ledger row. Do not substitute Workers AI, a GPU service, the
JavaScript ONNX interpreter or a reduced model for a failing Molt artifact.

The real-model demo is a periodic witness, never a landing gate. Its scheduled
or on-demand run must reuse the last-built artifacts, make one fixed sample
request per model against `ocr.adpena.com`, compare pinned expected text and
record timings. A failure is reported to the ledger owner. Keep pre-submit
coverage limited to small routing, authorization and registry unit tests; do
not add a full-model pre-submit test or proof-plan cell.

Deployment, secrets and resource creation belong to the integrator after source
integration. This local build procedure performs none of those operations.
