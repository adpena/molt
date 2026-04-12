# Cloudflare Thin Adapter Surface

Canonical home for generalized thin-Worker deployment helpers that serve
immutable manifests/assets while client-side wasm or WebGPU handles execution.

Current shared surfaces:
- `worker.ts`: reusable manifest-serving Worker helper for assets-backed deployments
- `verify.py`: reusable `wrangler check` / `deploy --dry-run` verification helpers
