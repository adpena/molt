# WASM Container Representation Contract Plan

## Design

The roadmap and `ScalarRepresentationPlan` contract say backend semantic
container dispatch must come from structured representation facts, not
transport-only `container_type` or result-side `type_hint` metadata.

A focused `wasm-backend` regression currently expects `container_type=str`
alone to select the `len_str` import. Running it proves the test is stale:
the backend emits the generic `len` import because the shared plan correctly
ignores transport metadata.

Update the regression so it proves both sides of the contract:

- `type_hint=str` alone does not select `len_str`.
- `container_type=str` alone does not select `len_str`.
- structured parameter type facts (`param_types = ["str"]`) do select
  `len_str`.

No runtime semantics change should be needed.

## Files

- `runtime/molt-backend/tests/wasm_compilation.rs`

## Tests

- Baseline failure already observed:
  `cargo test --profile release-fast -p molt-backend --features wasm-backend scalar_type_hint_alone_does_not_select_wasm_len_specialization -- --nocapture`
- After patch:
  same focused command should pass.
- Run `cargo fmt --check` or `cargo fmt --check -p molt-backend` if accepted.
- Run `git diff --check`.

## Risks

- If structured `param_types = ["str"]` does not select `len_str`, the backend
  contract is incomplete and needs code changes rather than test-only repair.
- Keep the patch scoped to WASM representation-contract coverage; do not touch
  unrelated backend hint cleanup in this slice.

## Exit Criteria

- Focused wasm-backend test passes.
- Diff check is clean.
- Working tree contains only owned, staged changes before commit.
