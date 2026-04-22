# Falcon Drivers

Falcon-OCR target drivers live here.

The downstream enjoice app should consume the Molt-owned handoff adapters in
`deploy/enjoice/` and should not define duplicate low-level deployment/runtime
drivers. This `drivers/falcon/` tree owns target-local driver experiments and
benchmarks; `deploy/enjoice/` owns the product integration adapter surface.

Current targets:
- `browser_webgpu/`
- `browser_wasm_cpu/` (planned target-local config)
- `wasi_server/` (planned target-local config)
- `cloudflare_thin_adapter/` (planned target-local config)
- `native_packaging/` (planned target-local config)
