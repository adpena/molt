# Vision: Molt — Verified Subset Python → Native / WASM

Molt compiles a **verified per-application subset** of Python into **small, fast native binaries** (and optionally WASM),
using semantic reduction + specialization + a micro-runtime.

This document is a placeholder. The authoritative spec will define:

- Compatibility tiers and the “Minimal Python Subset” (MPS) per application
- Soundness model: proofs vs guards vs deopt/fallback
- Micro-runtime contracts (strings/containers/exceptions/memory model)
- IR design and optimization pipeline
- Packaging, reproducibility, cross-compilation, and security model
