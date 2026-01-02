# Vision: Molt — Verified Subset Python → Native / WASM

Molt compiles a **verified per-application subset of Python** into **small, fast native binaries** (and optionally WASM) with strict determinism and reproducibility. The compiler applies semantic reduction and specialization to make Python feel like systems code without runtime AI dependencies.

## Goals
- **Deterministic outputs**: Bit-identical binaries given the same source and lockfiles.
- **Whole-program optimization**: Tiered compilation with aggressive specialization for stable code paths.
- **Production-grade safety**: Soundness rules and explicit guard/deopt for dynamic behavior.
- **Practical deployment**: Single-file executables with clear capability boundaries.

## Non-goals (near-term)
- Full CPython compatibility for every dynamic feature.
- C-extension ABI compatibility in Tier 0.
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
- No GIL; concurrency via tasks/channels.
- FFI and WASM packages are capability-gated with explicit effects.
- Lockfile enforcement and SBOM generation for reproducible builds.
