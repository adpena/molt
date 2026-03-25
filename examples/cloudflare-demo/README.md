# Molt Python on Cloudflare Workers

Python compiled to WASM, running at the edge. No interpreter. No cold start penalty.

## Quick Start

```bash
# Build the Python app to WASM
cd /path/to/molt
molt build examples/cloudflare-demo/src/app.py \
    --target wasm --profile cloudflare \
    --output examples/cloudflare-demo/dist/output.wasm \
    --linked-output examples/cloudflare-demo/dist/worker_linked.wasm

# Test locally
cd examples/cloudflare-demo
wrangler dev

# Deploy to Cloudflare
wrangler deploy
```

## What This Demo Shows

- Python functions (fibonacci, data transformation, report generation)
- JSON serialization
- Zero-dependency deployment
- Compiled to WASM with all Molt optimizations (br_table, dead local elimination, constant folding, box/unbox elimination)

## Split-Runtime Mode (Recommended)

For production deployments, use `--split-runtime` to tree-shake the runtime
and produce two modules: a tiny app module and a shared runtime that can be
cached independently by the CDN.

```bash
molt build examples/cloudflare-demo/src/app.py \
    --target wasm --profile cloudflare \
    --output examples/cloudflare-demo/dist/output.wasm \
    --linked-output examples/cloudflare-demo/dist/worker_linked.wasm \
    --split-runtime
```

This produces:
- `app.wasm` (~50-100KB) - just your compiled Python code
- `molt_runtime.wasm` (~1-2MB) - tree-shaken runtime with only the builtins your app uses
- `worker.js` - multi-module loader that stitches them together
- `manifest.json` - deployment manifest
- `wrangler.toml` - ready-to-deploy Cloudflare config

vs. the monolithic `worker_linked.wasm` (~3MB gzipped) that includes everything.

## Architecture

```
app.py → molt build → worker_linked.wasm → Cloudflare Workers
                                ↓
                          worker.js (entry point)
                          WASI shim (stdout capture)
                          Response = stdout output

Split-runtime:
app.py → molt build --split-runtime →  app.wasm (tiny, changes per deploy)
                                     +  molt_runtime.wasm (shared, cached by CDN)
                                     +  worker.js (multi-module loader)
```
