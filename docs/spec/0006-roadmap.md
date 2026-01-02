# Molt Roadmap: From Scaffold to Production

## Milestone 1: The "Hello World" Scaffold (Week 1-2)
**Goal**: A minimal pipeline that compiles a Python function adding two integers.
- [ ] Rust `molt-runtime` with basic integer representation.
- [ ] Python `molt-compiler` parsing AST and generating simple Cranelift IR.
- [ ] `molt build` CLI command that produces an executable.
- [ ] Basic CI with `uv` and `cargo` setup.
**Acceptance Criteria**: `molt build hello.py` produces a binary that prints `42` and exits.

## Milestone 2: The Tier 0 Foundation (Month 1-2)
**Goal**: Support for classes, functions, and basic collections in "Frozen" mode.
- [ ] Structification of Tier 0 classes.
- [ ] Fast list and dict implementations in Rust.
- [ ] Basic type inference and monomorphization.
- [ ] Differential test harness against CPython.
**Acceptance Criteria**: A simple "Mandelbrot" or "Binary Trees" benchmark runs significantly faster than CPython.

## Milestone 3: The Service/Pipeline Tier (Month 3-4)
**Goal**: Support for async, JSON, and basic networking.
- [ ] `molt_json` package (Rust-backed).
- [ ] Async/Await support mapped to a Rust async runtime (Tokio/Embassy).
- [ ] WASM interop: calling a Rust-in-WASM module from Python.
- [ ] Tier 1 Guards + Deoptimization mechanism.
**Acceptance Criteria**: A minimal HTTP "Hello World" server built with Molt beats CPython/Uvicorn in throughput.

## Milestone 4: Production Ready (Month 6+)
**Goal**: Full toolchain, optimization, and packaging.
- [ ] Profile-Guided Optimization (PGO).
- [ ] Cross-compilation for Linux (x86/arm64) and macOS (arm64).
- [ ] Security audit and sandboxing (WASM-based).
- [ ] Benchmarking suite with regression gates.
**Acceptance Criteria**: Successful deployment of a non-trivial data pipeline or microservice in a production-like environment.
