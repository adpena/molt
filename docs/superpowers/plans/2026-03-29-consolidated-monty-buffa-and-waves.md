# Canonical Engineering Burndown Plan: Monty, Buffa, and Runtime/WASM Closure

> Audited and re-consolidated on 2026-03-30 for maximum developer velocity, signal density, and low-overhead execution.

## Why this plan exists

This is now the single canonical engineering execution plan for the remaining repo-local compiler/runtime/tooling work.

The separate `docs/superpowers/plans/2026-03-26-linear-grouped-backlog.md` plan remains isolated on purpose because it drives live Linear/workspace operations with a different cadence, credentials boundary, and convergence loop.

## Consolidation outcome

The following engineering child plans are now fully folded into this document and should not be used as separate execution plans:

- `2026-03-26-stdlib-object-partition.md`
- `2026-03-27-wave-a-correctness-fortress.md`
- `2026-03-27-wave-b-ecosystem-unlock.md`
- `2026-03-27-wave-c-wasm-first-class.md`

Already retired before this pass:

- `2026-03-27-wrapper-artifact-contract.md`
- `2026-03-27-branch-integration-into-main.md`
- `2026-03-27-molt-stabilization-and-roadmap-continuation.md`
- `2026-03-27-repo-gap-closure-program.md`
- `2026-03-28-cloudflare-demo-hardening.md`
- `2026-03-28-phase1-wire-and-ship.md`
- `2026-03-28-harness-engineering.md`

## Execution model

- Use this file for all engineering burndown and closure tracking.
- Use `2026-03-26-linear-grouped-backlog.md` as the only separate ops/workspace plan.
- Do not create new child plans unless the work truly needs a distinct operational lane.
- Prefer closing whole tracks with explicit gates over growing more checklist sprawl.

## Burndown board

| Track | Area | Dependency tier | Validation gate | Current blocker |
|---|---|---|---|---|
| E0 | Stdlib partition contract | Tier 0 | Focused backend + CLI partition tests | Need final ownership proof, explicit link-input proof, and `emit=obj` contract |
| E1 | Wave A correctness exit | Tier 0 | Focused differential + backend + daemon/TIR sweep | Need one explicit exit pass and Cranelift-baseline decision record |
| E2 | Buffa/protobuf end-to-end proof | Tier 0 | Molt-facing e2e proof, not crate-local only | Existing crate code exists; repo still lacks a canonical Molt-surface proof path |
| E3 | Ecosystem unlock (`click`, `attrs`) | Tier 1 (after E0/E1) | Small ecosystem differential matrix | `six` is covered; `click` and `attrs` focused lanes still missing |
| E4 | WASM parity and deploy proof | Tier 1 (after E0/E1) | WASM parity sweep + live deploy proof + benchmark artifact | Need one current parity pass, deploy proof artifact, and size/startup evidence |
| E5 | Final docs/status convergence | Tier 2 (after E2/E3/E4) | Canonical docs reflect only proven claims | Must wait for engineering exit gates to settle |

## Tier 0 - run in parallel now

### E0 - Stdlib partition contract

Scope folded from the old stdlib-object-partition residual plan.

Remaining work:
- prove backend symbol ownership excludes non-entry stdlib `molt_init_*` symbols while preserving entry/runtime ABI roots;
- keep native linking driven by explicit stdlib partition artifacts and artifact membership;
- lock one canonical `emit=obj` contract under partition mode.

Validation:
- `cargo test -p molt-backend --features native-backend user_owned_symbol_whitelist_keeps_only_entry_roots -- --nocapture`
- `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k 'stdlib_link_fingerprint or stdlib_partition_mode_changes_cache_identity or stdlib_partition_emit_obj'`

### E1 - Wave A correctness exit

Scope folded from the old Wave A residual plan.

Remaining work:
- record whether `cranelift 0.130.0` remains the intended baseline or upgrade if not;
- run the focused Wave A regression sweep for nested loops, stdlib attribute access, tuple-subclass MRO, and genexpr enumerate unpacking;
- verify daemon/TIR behavior on the same focused slice.

Validation:
- `cargo check -p molt-backend --features native-backend`
- `cargo test -p molt-backend --features native-backend -- --nocapture`
- `MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py --jobs 1 tests/differential/basic/nested_indexed_loops.py tests/differential/basic/triple_nested_loops.py tests/differential/basic/stdlib_attr_access.py tests/differential/basic/tuple_subclass_mro.py tests/differential/basic/genexpr_enumerate_unpack.py`

### E2 - Buffa/protobuf end-to-end proof

This stays in the umbrella plan because it cuts across runtime crates, Molt-facing APIs, and documentation.

Remaining work:
- prove the existing `runtime/molt-runtime-protobuf` encode/decode/audit-event implementation is exercised from a Molt-facing surface;
- if that proof does not exist, add the smallest end-to-end test/documented path that makes it real;
- document the supported boundary once proven.

Validation:
- a repo-local Molt-facing proof path plus focused tests under canonical roots;
- `cargo test -p molt-runtime-protobuf`

## Tier 1 - starts once E0 and E1 are green

### E3 - Ecosystem unlock

Scope folded from the old Wave B residual plan.

Remaining work:
- add and close a focused `click` import/decorator differential lane;
- add and close a focused `attrs` end-to-end differential lane;
- keep `six`, `click`, and `attrs` on one small matrix and fix reusable semantics at the runtime/frontend/backend layers, not with package shims.

Validation:
- `MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/import_six.py tests/differential/basic/import_click.py tests/differential/basic/import_attrs.py --jobs 1`

### E4 - WASM parity and deploy proof

Scope folded from the old Wave C residual plan.

Remaining work:
- re-run and stabilize the focused WASM parity sweep;
- produce one real Cloudflare live deploy verification artifact, or leave an explicit credentials blocker;
- emit one current WASM benchmark artifact for size/startup closure.

Validation:
- `PYTHONPATH=src uv run --python 3.12 python3 -m pytest -q tests/test_wasm_importlib_machinery.py tests/test_wasm_link_validation.py tests/cli/test_cli_wasm_artifact_validation.py`
- `python3 tools/cloudflare_demo_deploy_verify.py --live-base-url <worker-url> --artifact-root logs/cloudflare_demo_verify/live`
- `PYTHONPATH=src uv run --python 3.12 python3 tools/bench_wasm.py --linked --output bench/results/wave_c_exit_gate.json`

## Tier 2 - starts once E2, E3, and E4 are green

### E5 - Docs and status convergence

Remaining work:
- refresh docs/spec/status surfaces so they claim only what the finished gates prove;
- keep `bench/results/`, `logs/`, and `tmp/` as the canonical evidence roots;
- remove this plan only when the engineering burndown has no remaining open tracks.

## Global exit gate

- E0-E5 are either closed or have an explicit blocker recorded in this file.
- The only remaining separate execution plan is `docs/superpowers/plans/2026-03-26-linear-grouped-backlog.md`.
- No new engineering child plans are needed to understand or execute the remaining work.
