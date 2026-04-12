# Falcon Browser WebGPU Driver

Target-local deployment and benchmark scaffold for the Falcon browser WebGPU
lane.

This directory intentionally lives in `molt`:
- Falcon deployment/driver ownership stays with the compiler/runtime repo.
- `enjoice` should only provide the Falcon application artifacts and a thin
  import layer.

Files:
- `browser.js`: browser-facing runtime loader and `ocrTokens` driver API
- `wrangler.jsonc`: target-local Cloudflare Worker config
- `worker.ts`: Worker entrypoint scaffold
- `deploy.py`: deployment-surface discovery plus immutable artifact manifest/hashes
- `bench_hostfed.py`: host-fed benchmark wrapper using Molt's generic helper
