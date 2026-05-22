# Plan: Retire MOLT_TIR_SKIP and make TIR passes unconditional

## Design
- Remove the `MOLT_TIR_SKIP` environment-variable hook from `tir::passes::run_pipeline`.
- Keep the pass list itself as the single source of truth for order and identity.
- Preserve debug observability through existing TIR dumps (`MOLT_DUMP_IR` / `TIR_DUMP`) and verifier failures, not pass-skipping behavior.
- Do not add a replacement opt-out or feature flag.

## Files
- `runtime/molt-backend/src/tir/passes/mod.rs`
- Plan artifact: `tmp/plan_tir_unconditional.md`

## Tests
- Baseline and post-change: `cargo test --profile release-fast -p molt-backend --features native-backend --lib tir::passes`
- Post-change source audit: no `MOLT_TIR_SKIP` references in `runtime/`, `src/`, `tests/`, `tools/`, or canonical docs.
- Post-change default gates if the patch is accepted: backend lib tests, workspace build, compliance.

## Risks
- Some hidden local workflow could have depended on skipping individual passes. That conflicts with `ROADMAP.md` and `docs/spec/STATUS.md`, so the structural fix is to remove the bypass and rely on dumps/verifier evidence.
- Removing the skip machinery changes pipeline debug behavior only; supported compilation behavior should remain unchanged when the variable is unset.

## Exit Criteria
- No code path reads `MOLT_TIR_SKIP`.
- `run_pipeline` always records every pass invocation in source order.
- Focused TIR pass tests and the smallest convincing touched-surface gates pass.
