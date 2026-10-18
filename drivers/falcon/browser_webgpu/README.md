# Falcon Browser WebGPU Driver

Target-local deployment and benchmark scaffold for the Falcon browser WebGPU
lane.

This directory intentionally lives in `molt`:
- Falcon deployment/driver ownership stays with the compiler/runtime repo.
- `enjoice` should only provide the Falcon application artifacts and a thin
  import layer.

Files:
- `browser.js`: target-local browser-facing runtime loader and `ocrTokens`
  driver API, with driver metadata pointing to the canonical build manifest
- `wrangler.jsonc`: target-local Cloudflare Worker config
- `worker.ts`: thin Cloudflare driver-metadata/bootstrap worker
- `deploy.py`: deployment-surface discovery plus bundle materialization and immutable artifact manifest/hashes
- `verify.py`: materialize + `wrangler check` + `deploy --dry-run` verifier
- `bench_hostfed.py`: host-fed benchmark wrapper using Molt's generic helper

Target root contract:
- the complete WASM build output under `dist/browser_split/` (the existing
  artifact directory name), including `manifest.json`, all declared modules
  and every declared asset; both canonical linked and split-runtime modes work
- weight blobs under `weights/`
- Falcon config at either `config.json` or `weights/config.json`

Bootstrap contract:
- `initFalconBrowserWebGpu()` always resolves this driver's manifest and
  model metadata first, then passes its `artifacts.execution_manifest.url` to the
  canonical browser host. Deployment preserves the build manifest and its complete
  file closure under `assets/runtime/`; module/ABI metadata is never synthesized.
  Split builds supply their own declared browser host assets. Linked builds omit
  that kit, so deployment selects the canonical host closure and records its
  bytes separately in `hosting_assets`; verification uses those retained records,
  without consulting a newer compiler checkout. Deployment and verification reuse
  the browser asset generator's Node/Acorn scanner to reject retained JavaScript
  references to undeclared files, including dependencies omitted from both the
  inventory and disk. Bootstrap the parser with
  `python tools/bootstrap_browser_asset_graph.py` before these development checks.
  This is intentionally separate from the direct-WASM
  `deploy/enjoice/` integration path.
- WebGPU capability is enforced lazily by the browser host only if the loaded
  application actually exercises GPU kernel dispatch, or when the caller
  injects a `gpuKernelDispatcher`.

Initialization fetches host, weights and config in parallel and settles ownership
before reporting a failure. A fulfilled host is disposed if any acquisition,
program entry or model initialization fails; cleanup failures retain the original
error. Successful initialization transfers `session.host` to the caller: keep it
for repeated OCR calls, and call `session.host.dispose()` in the owner's `finally`
when the session ends.
