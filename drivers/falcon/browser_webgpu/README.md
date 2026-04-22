# Falcon Browser WebGPU Driver

Target-local deployment and benchmark scaffold for the Falcon browser WebGPU
lane.

This directory intentionally lives in `molt`:
- Falcon deployment/driver ownership stays with the compiler/runtime repo.
- `enjoice` should only provide the Falcon application artifacts and a thin
  import layer.

Files:
- `browser.js`: target-local browser-facing runtime loader and `ocrTokens`
  driver API, with legacy manifest bootstrap support for this driver lane
- `wrangler.jsonc`: target-local Cloudflare Worker config
- `worker.ts`: thin Cloudflare manifest/bootstrap worker for this legacy
  driver-discovery lane
- `deploy.py`: deployment-surface discovery plus bundle materialization and immutable artifact manifest/hashes
- `verify.py`: materialize + `wrangler check` + `deploy --dry-run` verifier
- `bench_hostfed.py`: host-fed benchmark wrapper using Molt's generic helper

Target root contract:
- split-runtime artifacts under `dist/browser_split/`
- weight blobs under `weights/`
- Falcon config at either `config.json` or `weights/config.json`

Bootstrap contract:
- `initFalconBrowserWebGpu()` always resolves this driver's manifest and
  artifacts first. This is intentionally separate from the direct-WASM
  `deploy/enjoice/` integration path.
- WebGPU capability is enforced lazily by the browser host only if the loaded
  application actually exercises GPU kernel dispatch, or when the caller
  injects a `gpuKernelDispatcher`.
