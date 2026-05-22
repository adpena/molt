Topic: retire residual scalar transport attrs inside TIR

Design:
- Treat `TirFunction.value_types` / typed LIR as the only scalar representation authority inside TIR.
- Stop preserving frontend `fast_int` / `fast_float` as `_fast_int` / `_fast_float` attrs during SSA lift.
- Stop adding `_fast_int` attrs to range/list devirtualization synthesized compares, increments, and len calls.
- Keep `_type_hint` only where it carries structural class identity for object allocation round-trips.

Files:
- `runtime/molt-backend/src/tir/ssa.rs`
- `runtime/molt-backend/src/tir/passes/range_devirt.rs`
- `runtime/molt-backend/src/tir/passes/iter_devirt.rs`
- `runtime/molt-backend/src/tir/tests_roundtrip.rs`
- `tmp/plan_tir_scalar_attr_retire.md`

Tests:
- Baseline and post-change targeted TIR tests for SSA roundtrip, range devirt, iter devirt, and lower-to-simple.
- Full `cargo test --profile release-fast -p molt-backend --features native-backend --lib` before claiming done.
- Workspace build and compliance if the focused backend proof is green.

Risks:
- Some latent code path may still be reading `_fast_int` attrs even though current search only shows writers.
- Devirt performance must keep typed facts; regressions assert I64/Bool facts remain on synthesized values.

Exit criteria:
- `rg "_fast_int|_fast_float" runtime/molt-backend/src/tir` shows only tests/comments that intentionally cover legacy SimpleIR input, not TIR attr writers.
- Devirt synthesized ops carry value facts without scalar transport attrs.
- Required gates pass and the worktree is clean after commit/push.
