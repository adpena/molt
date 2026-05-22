# loop_narrow retirement plan

## Design
- Remove the TIR `loop_narrow` pass from the optimization pipeline because it
  is analysis-only, mutates no IR, and still scans `_fast_int` transport attrs.
- Preserve the real typed-IR authority: loop and backend scalar decisions must
  come from `TirFunction.value_types`, LIR lowering, and the shared
  `ScalarRepresentationPlan`, not a no-op transport-hint pass.
- Update neighboring comments so no code claims `loop_narrow` owns bounded-loop
  narrowing or `no_signed_wrap`; `range_devirt` already stamps the actual
  `no_signed_wrap` proof where it is known sound.

## Files
- `runtime/molt-backend/src/tir/passes/mod.rs`
- `runtime/molt-backend/src/tir/passes/loop_narrow.rs`
- `runtime/molt-backend/src/tir/passes/bce.rs`
- `runtime/molt-backend/src/llvm_backend/lowering.rs`

## Tests
- Baseline: `cargo test --profile release-fast -p molt-backend --features native-backend --lib loop_narrow`
- Baseline: `cargo test --profile release-fast -p molt-backend --features native-backend --lib tir::passes`
- Focused post-change: `cargo test --profile release-fast -p molt-backend --features native-backend --lib tir::passes`
- Source audit: `rg -n "loop_narrow|_fast_int" runtime/molt-backend/src/tir/passes runtime/molt-backend/src/llvm_backend/lowering.rs`

## Risks
- Pipeline pass-count expectations could exist outside direct tests; source
  search must confirm no caller depends on a `loop_narrow` stat.
- Removing a pass can expose stale docs/comments; all references must be
  updated in the same change.

## Exit criteria
- No `loop_narrow` module or pipeline entry remains.
- No TIR pass consumes `_fast_int` as an input-only analysis signal.
- Existing TIR pass tests remain green.
