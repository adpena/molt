# Consolidated Remaining Work: Monty, Buffa, and Active Waves

> Audited on 2026-03-30. This is now the umbrella plan for the work that remains after pruning completed and superseded superpowers plans.

## Completed or retired during the audit

The following plan files were removed because their scope is either complete or fully obsolete as execution plans:

- `2026-03-27-wrapper-artifact-contract.md`
- `2026-03-27-branch-integration-into-main.md`
- `2026-03-27-molt-stabilization-and-roadmap-continuation.md`
- `2026-03-27-repo-gap-closure-program.md`
- `2026-03-28-cloudflare-demo-hardening.md`
- `2026-03-28-phase1-wire-and-ship.md`
- `2026-03-28-harness-engineering.md`

## Active child plans

- `docs/superpowers/plans/2026-03-26-stdlib-object-partition.md`
- `docs/superpowers/plans/2026-03-26-linear-grouped-backlog.md`
- `docs/superpowers/plans/2026-03-27-wave-a-correctness-fortress.md`
- `docs/superpowers/plans/2026-03-27-wave-b-ecosystem-unlock.md`
- `docs/superpowers/plans/2026-03-27-wave-c-wasm-first-class.md`

## Audit outcome

- Complete enough to remove from active execution tracking:
  - wrapper artifact contract;
  - Phase 1 wire-and-ship;
  - Cloudflare hardening;
  - harness engineering;
  - the three superseded meta-plans.
- Still open at umbrella level:
  - close the remaining Wave A/B/C and stdlib-partition residuals;
  - finish the grouped Linear backlog migration;
  - prove Buffa/protobuf support end-to-end instead of only at crate level;
  - refresh the final documentation and evidence once the active plans are closed.

## Parallel execution tiers

### Tier 0 - foundation closure (run in parallel now)

1. Finish `stdlib-object-partition` residuals.
2. Run the `wave-a-correctness-fortress` exit gate.
3. Audit `runtime/molt-runtime-protobuf` end-to-end integration: prove the existing encode/decode/audit-event crate is exercised from Molt-facing surfaces, or write the missing end-to-end tests/tasks that make that true.

### Tier 1 - ecosystem and wasm closure (starts once Tier 0 is green)

1. Run `wave-b-ecosystem-unlock` to close `click` and `attrs`.
2. Run `wave-c-wasm-first-class` to close parity, live deploy proof, and benchmark evidence.

### Tier 2 - tooling and roadmap convergence (starts once Tier 1 is green)

1. Finish `linear-grouped-backlog` and drive the live workspace to grouped convergence.
2. Refresh docs and canonical status surfaces so they only claim what the completed exit gates proved.
3. Remove any now-empty residual plan files on the next audit pass.

## Umbrella exit gate

- Every active child plan either reaches zero open tracks and is deleted, or remains with a clearly stated blocker.
- Buffa/protobuf has an end-to-end proof path, not just crate-local code.
- Final validation artifacts live under canonical roots (`bench/results/`, `logs/`, `tmp/`).
