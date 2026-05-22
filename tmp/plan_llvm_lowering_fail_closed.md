# LLVM Lowering Fail-Closed Plan

## Design

LLVM lowering must not convert malformed TIR into verified-but-semantically-wrong IR. Missing `ValueId` definitions, unsupported phi coercions, and incomplete phi predecessor coverage are fatal lowering errors. The lowering boundary should report those errors before the module is verified, optimized, emitted, or executed.

Keep valid TIR behavior unchanged. Preserve intentionally undefined entry phi values for loop/header variables that have no initial function parameter, because that is currently the documented entry-trampoline representation.

## Files

- `runtime/molt-backend/src/llvm_backend/lowering.rs`
- `runtime/molt-backend/src/lib.rs`
- focused LLVM lowering unit regressions in the existing lowering test module

## Tests

- `cargo test --profile release-fast -p molt-backend --no-default-features --features llvm llvm_backend::lowering::tests:: --lib`
- `cargo build --profile release-fast --workspace`
- if LLVM feature availability blocks local focused tests, run the smallest native/backend proof matrix and record the exact blocker.

## Exit Criteria

- Malformed LLVM lowering no longer inserts `undef` as a silent fallback.
- Compile path receives a structured lowering error and aborts before module verification/emission.
- Valid existing LLVM lowering regressions keep passing.
