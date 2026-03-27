# Canonical Reconciliation 2026-03-27

## Goal

Preserve all meaningful repository signal from local and `Primary`, make GitHub the sole canonical source of truth, and then sync the machines back to that GitHub-backed state.

## Inputs Reviewed

- Local `main`: `7f9bf751` (`origin/main`)
- `Primary` pre-sync `main`: `691aa24a`
- Local unmerged branch heads:
  - `backup-broken-main` -> `f6a98c3b`
  - `fix-stdlib-intrinsics` -> `8c5f3827`
  - `fix/compilation-errors` -> `5783d268`
  - `parity-fixes` -> `81ca61dc`
  - `refactor/split-platform-importlib` -> `fecf5ee9`
  - `split-functions` -> `f6a98c3b`
  - `split-functions-rs` -> `b475aa71`
  - `split-ops-file` -> `b475aa71`
- Local merged branch head:
  - `fix/molt-runtime-compile` -> `d737aeaf` (already ancestor of `main`)
- Detached/prunable local worktree:
  - `/private/tmp/molt-parity-clean` pointed at `d737aeaf`, which is already merged into `main`

## Canonical Decision

Do not merge the stale side branches or `Primary`'s divergent optimization history directly into `main`.

Reason:

1. `Primary` carried a hand-written integration plan explicitly stating that old side branches should be replayed selectively by cherry-pick/manual reapplication, not merged wholesale, because they are far behind current `main` and include superseded/reverted history.
2. Local `main` already contains 45 commits absent from `Primary`, including newer correctness fixes, diagnostics cleanup, runtime safety fixes, and the wrapper artifact contract handoff.
3. `Primary`'s divergent `main` still contains 31 commits not on local `main`, but those commits are optimization-heavy and include explicit revert history; landing them wholesale without tranche verification would violate the repo quality bar.

Accordingly, canonical `main` stays on the current local/GitHub lineage, while non-canonical histories are preserved as archival GitHub tags plus tracked plan/data artifacts on `main`.

## Signal Preserved On Canonical `main`

- Imported from `Primary`:
  - `docs/superpowers/plans/2026-03-27-branch-integration-into-main.md`
  - `docs/superpowers/plans/2026-03-27-molt-stabilization-and-roadmap-continuation.md`
  - `bench/results/primary_pre_sync_20260325T172924-0500.json`
- Already present on local/GitHub `main`:
  - `PRIMARY_HANDOFF.md`
  - `docs/superpowers/plans/2026-03-27-wrapper-artifact-contract.md`

## Dirty Working Tree Interpretation On `Primary`

`Primary` had this additional uncommitted state before sync:

- deleted `DICT_BUG.md`
- deleted `SESSION_CHANGES.md`
- modified `bench/results.json`
- untracked `PRIMARY_HANDOFF.md`
- untracked `docs/superpowers/plans/2026-03-27-branch-integration-into-main.md`
- untracked `docs/superpowers/plans/2026-03-27-molt-stabilization-and-roadmap-continuation.md`
- untracked `docs/superpowers/plans/2026-03-27-wrapper-artifact-contract.md`

Reconciliation treatment:

- The two deleted markdown files were not promoted as deletions because their contents already exist on canonical `main` and `Primary` did not contain replacement content, only local removal.
- The benchmark JSON payload was preserved as a dated artifact under `bench/results/`.
- The untracked wrapper contract plan was identical to the tracked copy already on canonical `main`.
- The untracked `PRIMARY_HANDOFF.md` was already tracked on canonical `main`.

## Archival Strategy

Preserve all non-main commit lineages on GitHub as archival tags, then sync worktrees to canonical `main`.

Expected archival tags:

- `archive/2026-03-27/local/backup-broken-main`
- `archive/2026-03-27/local/fix-stdlib-intrinsics`
- `archive/2026-03-27/local/fix/compilation-errors`
- `archive/2026-03-27/local/parity-fixes`
- `archive/2026-03-27/local/refactor/split-platform-importlib`
- `archive/2026-03-27/local/split-functions`
- `archive/2026-03-27/local/split-functions-rs`
- `archive/2026-03-27/local/split-ops-file`
- `archive/2026-03-27/primary/main-pre-sync`

Local safety artifacts created outside the tracked tree:

- `tmp/reconcile/2026-03-27/local-all-refs.bundle`
- `Primary:/Users/adpena/Projects/molt/tmp/reconcile/2026-03-27/primary-all-refs.bundle`
- `Primary:/Users/adpena/Projects/molt/tmp/reconcile/2026-03-27/primary-working-tree.diff`
- `Primary:/Users/adpena/Projects/molt/tmp/reconcile/2026-03-27/primary-index.diff`
- `Primary:/Users/adpena/Projects/molt/tmp/reconcile/2026-03-27/primary-untracked.tgz`
