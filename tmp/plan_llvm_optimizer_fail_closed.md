# LLVM Optimizer Fail-Closed Plan

## Design

LLVM optimization is part of the release backend contract. If LLVM rejects the
configured pass pipeline, Molt must not emit an unoptimized object after a
warning. Convert the optimizer boundary to `Result<(), String>` and propagate
failures to the production compile path before object emission.

Preserve the current linkage-protection invariant: externally visible lowered
functions are temporarily marked `dllexport` while the pass pipeline runs, and
their original linkage is restored whether optimization succeeds or fails.

## Files

- `runtime/molt-backend/src/llvm_backend/mod.rs`
- `runtime/molt-backend/src/lib.rs`

## Tests

- Focused LLVM backend unit tests for successful optimization and pass-pipeline
  error propagation.
- `cargo test --profile release-fast -p molt-backend --no-default-features --features llvm llvm_backend::tests:: --lib`
- Canonical build/backend/compliance gates if the touched surface compiles cleanly.

## Risks

- LLVM pass-manager errors may leave temporary linkage in the module if cleanup
  is not centralized.
- Existing tests compiled with `--no-default-features --features llvm` have
  pre-existing dead-code warnings from native-backend-gated code; do not create
  new warnings.

## Exit Criteria

- No `continuing unoptimized` fallback remains in the LLVM backend.
- Optimizer failures are observable as `Err` and the compile path fails closed.
- Linkage restoration is deterministic on success and failure.
