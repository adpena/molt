---
description: "Compile Python to WASM with molt and deploy to Cloudflare Workers edge"
---

# Deploy Python to Cloudflare Workers Edge

You are deploying a Python application to Cloudflare Workers via molt (Python-to-WASM compiler).

## Architecture

- Python source at `examples/cloudflare-demo/src/app.py` is compiled to WASM
- `worker.js` is the WASI shim that runs the WASM binary on Workers
- The Python code receives the URL path via `sys.argv[1]` and query string via `QUERY_STRING` env var
- `print()` output becomes the HTTP response body
- The VFS provides `/bundle` (read-only assets) and `/tmp` (scratch space)

## Steps

1. If the user provided Python code or a description of what to deploy, update `examples/cloudflare-demo/src/app.py` accordingly. Keep imports minimal: `sys`, `os`, `math`, `json`, `random` are safe.

2. Kill any stale backend daemon and compile to WASM:
```bash
pkill -f "molt-backend.*daemon" 2>/dev/null; sleep 1
MOLT_WASM_PROFILE=pure .venv/bin/python -m molt build examples/cloudflare-demo/src/app.py \
    --target wasm --stdlib-profile micro \
    --output /tmp/molt_deploy/output.wasm \
    --linked-output /tmp/molt_deploy/linked.wasm
```

3. Optimize the binary:
```bash
wasm-opt -Oz --enable-bulk-memory --enable-reference-types --enable-simd \
    --enable-mutable-globals --enable-sign-ext --enable-nontrapping-float-to-int \
    --enable-multivalue --no-validation --remove-unused-module-elements \
    --remove-unused-names --strip-debug --dce --vacuum \
    /tmp/molt_deploy/linked.wasm -o /tmp/molt_deploy/linked_opt.wasm
```

4. Report binary size:
```bash
ls -lh /tmp/molt_deploy/linked_opt.wasm
gzip -k -f /tmp/molt_deploy/linked_opt.wasm && ls -lh /tmp/molt_deploy/linked_opt.wasm.gz
```

5. Copy and deploy:
```bash
cp /tmp/molt_deploy/linked_opt.wasm examples/cloudflare-demo/dist/worker_linked.wasm
wrangler deploy --config examples/cloudflare-demo/wrangler.toml
```

6. Test the live deployment:
```bash
curl -s https://molt-python-demo.adpena.workers.dev/
```

7. Report results to the user:
   - Live URL
   - Binary size (raw and gzip)
   - Test output from curl

## Constraints

- Do NOT modify `worker.js` unless the WASI shim needs updating
- Python code must not use: `eval`, `exec`, `__import__`, threading, raw sockets
- Keep total gzip under 3 MB (Cloudflare free tier limit)
- Available stdlib: sys, os, math, json, random, re, collections, functools, itertools, dataclasses, typing, enum
