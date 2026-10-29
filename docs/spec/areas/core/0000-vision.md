# Vision: Molt — Verified Subset Python → Native / WASM

Molt compiles a **verified per-application subset of Python** into **small, fast native binaries** and WASM modules with strict determinism and reproducibility. The compiler applies semantic reduction and specialization to make Python feel like systems code without runtime probabilistic dependencies.

This document states the lasting goals. The forward plan and its order live in
[ROADMAP.md](../../../../ROADMAP.md). The long-horizon engineering plan lives in
[the century systems plan](../../../design/CENTURY_SYSTEMS_PLAN.md). Current
support lives in [STATUS.md](../../STATUS.md).

## The contract
- **Version focus**: CPython `>=3.12` parity target for the verified subset; document any version-specific differences. The [verified-subset contract](../compat/contracts/verified_subset_contract.md) owns the intended semantics.
- **Outside the subset**: unrestricted `exec`/`eval`/`compile`, runtime monkeypatching and unrestricted reflection. Molt refuses them early with a useful diagnostic.
- **No host-CPython fallback**: binaries are fully standalone.
- **CPython is the floor**: compiled code must beat CPython on every claimed benchmark cell. PyPy is the reference for dynamic code and Codon for statically compilable code. Where either wins, name the missing Molt mechanism; mark semantic models that differ instead of scoring them.
- **Footprint is part of the product**: warm speed, cold start, peak memory, binary size and compile time are separate measured dimensions. The long-term targets are under 50 ms cold start and under 2 MB of shipped runtime payload.
- **GPU and ML run everywhere tinygrad runs**, with exact fidelity to the tinygrad and DFlash sources of truth.

## Goals
- **Deterministic outputs**: Bit-identical binaries given the same source and lockfiles.
- **Whole-program optimization**: Tiered compilation with aggressive specialization for stable code paths.
- **Production-grade safety**: Soundness rules and explicit guard/deopt for dynamic behavior.
- **Practical deployment**: Single-file executables with clear capability boundaries.

## Non-goals (near-term)
- Full CPython compatibility for every dynamic feature.
- CPython C-extension ABI compatibility in Tier 0 (recompile against `libmolt` instead).
- A browser-side JIT or hidden nondeterminism.
- Substitute implementations of upstream packages such as NumPy or SciPy.

## Compatibility model
- **Minimal Python Subset (MPS)** is derived per application and encoded in an Optimization Manifest.
- **Tier 0 (Frozen Python)**: static guarantees, no monkeypatching, no `eval/exec`, closed-world imports.
- **Tier 1 (Guarded Python)**: guarded speculation with deoptimization and slow paths.

## The method: facts, not reconstruction
Most wrong or slow programs share one root cause: the compiler tries to
reconstruct Python meaning (lifetimes, types, shapes, call targets, ownership,
effects) from low-level events after the high-level structure is gone. Molt
keeps that meaning as typed, generated, checkable facts on the IR, the
[semantic fact plane](../../../design/foundation/59_semantic_fact_plane.md).
Each fact retires a whole class of wrong or slow programs, and each has a
validator. The [compression ladder](../../../design/foundation/65_perf_compression_ladder.md)
orders that work, one class at a time.

## Soundness and determinism
- Tier 0 optimizations require proofs; Tier 1 requires guards and deopt exits.
- Nondeterminism is opt-in via explicit capabilities (time, randomness, I/O).
- Differential testing vs CPython is the correctness baseline for supported semantics.

## Pipeline summary
1. Parse Python → HIR (desugared AST)
2. Infer types and shapes → TIR (typed SSA)
3. Lower to LIR (explicit representation and ownership)
4. Lower to a backend: Cranelift native by default, LLVM, WASM or the JIT. Luau,
   the Rust source emitter and MLIR are experimental. Every backend implements
   an op or refuses it with a typed refusal against the same closed contract.
5. Link with the Molt runtime and verified packages

## Runtime contracts
- NaN-boxed object model with precise reference counting and a generational
  cycle collector (the [generational GC authority](../../../design/foundation/generational_gc_authority.md)).
- **Current RT1 contract:** a single GIL serializes runtime mutation and Python-visible execution
  (see `docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md`). The design keeps
  free-threaded and GIL-less execution possible without changing the default.
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
