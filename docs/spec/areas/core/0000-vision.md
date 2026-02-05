# Vision: Molt — Verified Subset Python → Native / WASM

Molt compiles a **verified per-application subset of Python** into **small, fast native binaries** (and optionally WASM) with strict determinism and reproducibility. The compiler applies semantic reduction and specialization to make Python feel like systems code without runtime probabilistic dependencies.

## Goals
- **Deterministic outputs**: Bit-identical binaries given the same source and lockfiles.
- **Whole-program optimization**: Tiered compilation with aggressive specialization for stable code paths.
- **Production-grade safety**: Soundness rules and explicit guard/deopt for dynamic behavior.
- **Practical deployment**: Single-file executables with clear capability boundaries.
- **Version focus**: Target Python 3.12+ semantics; document any version-specific differences.

## Non-goals (near-term)
- Full CPython compatibility for every dynamic feature.
- CPython C-extension ABI compatibility in Tier 0 (recompile against `libmolt` instead).
- Browser-side JIT or hidden nondeterminism.

## Compatibility model
- **Minimal Python Subset (MPS)** is derived per application and encoded in an Optimization Manifest.
- **Tier 0 (Frozen Python)**: static guarantees, no monkeypatching, no `eval/exec`, closed-world imports.
- **Tier 1 (Guarded Python)**: guarded speculation with deoptimization and slow paths.

## Soundness and determinism
- Tier 0 optimizations require proofs; Tier 1 requires guards and deopt exits.
- Nondeterminism is opt-in via explicit capabilities (time, randomness, I/O).
- Differential testing vs CPython is the correctness baseline for supported semantics.

## Pipeline summary
1. Parse Python → HIR (desugared AST)
2. Infer types and shapes → TIR (typed SSA)
3. Lower to LIR (explicit memory and ownership)
4. Emit Cranelift IR → native/WASM
5. Link with the Molt runtime and verified packages

## Runtime contracts
- NaN-boxed object model with RC + incremental cycle detection.
- **Current RT1 contract:** a single GIL serializes runtime mutation and Python-visible execution
  (see `docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md`).
- FFI and WASM packages are capability-gated with explicit effects.
- Lockfile enforcement and SBOM generation for reproducible builds.

## Concurrency & parallelism
- **CPython-correct asyncio** by default: a single-threaded event loop with deterministic ordering,
  structured cancellation, and explicit async boundaries under the GIL contract.
- **True parallelism is explicit**: CPU work goes through executors or isolated runtimes/actors with
  message passing; shared mutable parallelism is opt-in, capability-gated, and limited to
  explicitly safe types.
- **Runtime-first implementation**: the event loop, I/O poller, and cancellation propagation live in
  Rust so compiled binaries are self-contained; stdlib wrappers stay thin.
- **Native + WASM parity**: identical semantics across targets, with host I/O gated by capabilities.
