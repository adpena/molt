# Drivers

Target-specific deployment and execution adapters live here.

Rules:
- Drivers own host/runtime/environment wiring.
- Core `molt` owns semantics, tensor primitives, lowering contracts, and
  backend-independent behavior.
- Model-specific drivers are allowed here when the behavior is irreducible and
  should not leak into core runtime/compiler surfaces.

Current layout:
- `_shared/`
- `browser/`
- `wasm/`
- `cloudflare/`
- `native/`
- `falcon/browser_webgpu/`
