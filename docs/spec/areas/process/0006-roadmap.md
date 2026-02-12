# Molt Roadmap: From Scaffold to Production
**Status:** Historical milestone framing (superseded)

This document preserves the original milestone definitions for context. Current status and sequencing live in `ROADMAP.md`, with near-term steps in `docs/ROADMAP_90_DAYS.md`.

## Milestone 1: The "Hello World" Scaffold (Week 1-2)
**Goal:** Minimal pipeline that compiles a Python function to native code.
- Rust runtime with basic integer representation
- Python compiler frontend parsing AST and emitting simple backend IR
- `molt build` CLI producing an executable
- Basic CI with `uv` and `cargo`

## Milestone 2: The Tier 0 Foundation (Month 1-2)
**Goal:** Frozen Python support for classes, functions, and basic collections.
- Structification of Tier 0 classes
- Fast list/dict implementations in Rust
- Basic type inference and monomorphization
- Differential test harness vs CPython

## Milestone 3: The Service/Pipeline Tier (Month 3-4)
**Goal:** Async, structured codecs, and basic networking for service workloads.
- `molt_msgpack` / `molt_cbor` packages (Rust-backed); JSON retained for compatibility/debug
- Async/await mapped to a Rust async runtime
- WASM interop for Rust-in-WASM modules
- Tier 1 guards + deoptimization

## Milestone 4: Production Ready (Month 6+)
**Goal:** Full toolchain, optimization, and packaging.
- Profile-guided optimization (PGO)
- Cross-compilation for Linux and macOS
- Security audit and sandboxing (WASM-based)
- Benchmarking suite with regression gates
