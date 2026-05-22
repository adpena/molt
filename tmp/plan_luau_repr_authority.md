Luau Representation Authority Slice

Design
- Complete the Luau backend slice where string attribute closure lowering and
  list method specialization still read legacy `var_type_hints` as authority.
- Keep `var_type_hints` only as existing Luau-local annotation/propagation state
  until the larger typed-IR removal arc replaces it completely.
- Route specialization decisions through `ScalarRepresentationPlan`, matching
  the already-landed len/get_item/truthiness authority contract.

Files
- runtime/molt-backend/src/luau.rs

Tests
- Add negative regressions proving manually-seeded legacy hints cannot select
  string-method closures or list table-specialized method calls.
- Add/keep positive typed regressions proving structured TIR facts still select
  the optimized string/list Luau paths.
- Run focused guarded Rust tests for the new Luau regressions and neighboring
  existing representation-authority checks.

Risks
- If `ScalarRepresentationPlan` does not carry a needed typed fact, fix the plan
  rather than falling back to transport metadata or per-op guards.
- Do not widen Luau support or add semantic fallbacks in this slice.

Exit Criteria
- Luau string/list specialization decisions no longer consult
  `var_type_hints`.
- New negative and positive tests pass under `release-fast`.
- Working tree contains only owned staged changes before commit.
