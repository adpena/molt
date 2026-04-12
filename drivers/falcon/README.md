# Falcon Drivers

Falcon-OCR target drivers live here.

The Falcon app in `enjoice` should consume these drivers rather than defining
its own deployment/runtime adapters.

Current targets:
- `browser_webgpu/`
- `browser_wasm_cpu/` (planned target-local config)
- `wasi_server/` (planned target-local config)
- `cloudflare_thin_adapter/` (planned target-local config)
- `native_packaging/` (planned target-local config)
