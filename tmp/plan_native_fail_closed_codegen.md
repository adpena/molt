Native Backend Fail-Closed Codegen Slice

Design
- Remove the native backend resilience path that catches Cranelift panics,
  retries at opt_level=none, and emits trap stubs for failed functions.
- Treat Cranelift compile errors, panics, signature mismatches, and leftover
  undefined exported declarations as hard backend failures with actionable
  diagnostics.
- Preserve parallel deferred compilation, deterministic result ordering, and
  existing external-function import handling for batched/shared stdlib builds.

Files
- runtime/molt-backend/src/lib.rs
- runtime/molt-backend/src/native_backend/function_compiler.rs
- src/molt/cli.py
- tests/cli/test_backend_manifest_contract.py

Tests
- Add source-contract regressions proving native backend code does not contain
  `catch_unwind` and does not route backend failures through trap stubs.
- Run the new focused pytest.
- Run guarded native backend Rust unit tests and workspace build because this
  changes backend codegen failure handling.

Risks
- If any current test relies on trap stubs for an undefined exported function,
  that is a real backend bug to fix structurally rather than reintroducing a
  runtime-aborting artifact.
- If Cranelift exposes a genuine upstream panic on a supported program, the
  correct response is a minimized regression and codegen fix, not swallowing it.

Exit Criteria
- No native backend `catch_unwind` call remains.
- Native compile/signature/undefined-export failures fail closed.
- Focused and backend proof gates pass under the memory guard.
