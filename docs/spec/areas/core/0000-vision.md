# Vision: Molt - CPython 3.12+ Parity Target to Native / WASM

Molt compiles **Python with a CPython `>=3.12` parity target** into **small, fast native binaries** (and optionally WASM) with strict determinism and reproducibility. The compiler applies semantic reduction and specialization to make Python feel like systems code without runtime probabilistic dependencies.

## Goals
- **Deterministic outputs**: Bit-identical binaries given the same source and lockfiles.
- **Whole-program optimization**: Tiered compilation with aggressive specialization for stable code paths.
- **Production-grade safety**: Soundness rules and explicit guard/deopt for dynamic behavior.
- **Practical deployment**: Single-file executables with clear capability boundaries.
- **Version focus**: Target Python 3.12+ semantics; document any version-specific differences.
- **Parity direction**: push toward full CPython `>=3.12` language/runtime/stdlib behavior parity wherever that does not violate Molt's explicit break policy.
- **Standalone runtime**: compiled artifacts run without any host Python installation and never rely on hidden host-CPython fallback.

## Non-goals (near-term)
- Unrestricted `eval`/`exec` execution in compiled binaries.
- Runtime monkeypatching as a general compatibility promise.
- Unrestricted reflection/introspection that defeats static reasoning.
- CPython C-extension ABI compatibility in Tier 0 (recompile against `libmolt` instead).
- Browser-side JIT or hidden nondeterminism.

## Compatibility model
- **Parity-first direction**: Molt aims at full CPython `>=3.12` parity as the default architectural direction, then ratchets proof and evidence feature by feature.
- **Verified supported surface**: the currently verified surface is derived per application and encoded in an Optimization Manifest.
- **Tier 0 (Frozen Python)**: static guarantees, no monkeypatching, no `eval/exec`, closed-world imports.
- **Tier 1 (Guarded Python)**: guarded speculation with deoptimization and slow paths.

## Soundness and determinism
- Tier 0 optimizations require proofs; Tier 1 requires guards and deopt exits.
- Nondeterminism is opt-in via explicit capabilities (time, randomness, I/O).
- Differential testing vs CPython is the correctness baseline for supported semantics.

## Pipeline summary
1. Parse Python to HIR (desugared AST)
2. Infer types and shapes to TIR (typed SSA)
3. Lower to LIR (explicit memory and ownership)
4. Emit Cranelift IR to native/WASM
5. Link with the Molt runtime and verified packages

## Runtime contracts
- NaN-boxed object model with RC; incremental cycle detection remains on the
  active runtime roadmap and is not treated as complete until the runtime
  milestones mark it implemented.
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

## Month 1 Sign-off Readiness
- Status: Draft ready for alignment review (2026-02-11) per `docs/ROADMAP_90_DAYS.md`.
- Criteria:
  1. Goals/pipeline language matches canonical capability state in `docs/spec/STATUS.md`.
  2. Determinism/parity and capability-boundary language matches active planning in `ROADMAP.md`.
  3. Runtime contract language remains aligned with `docs/spec/areas/compiler/0100_MOLT_IR.md`.
  4. Runtime and compiler owners review and acknowledge this spec revision.
- Sign-off date: pending explicit owner approval (candidate baseline: 2026-02-11).
