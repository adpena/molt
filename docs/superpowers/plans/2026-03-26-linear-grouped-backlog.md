# Linear Grouped Backlog Residual Completion Plan

> Audited on 2026-03-30. The repo already contains grouped Linear manifests and supporting local artifacts, but the original plan still reads like untouched greenfield work. This version keeps only the remaining grouped-backlog closure work.

## Audit outcome

- Already landed:
  - grouped manifest artifacts exist under `ops/linear/manifests/`;
  - the backlog tooling already has enough structure to emit grouped data for local inspection.
- Still incomplete:
  - deterministic grouping and rollup behavior is not yet locked by focused tests;
  - refresh-local-artifacts and sync flows still need a single canonical grouped path;
  - live workspace migration/convergence has not been reduced to a small, repeatable verification loop.

## Parallel tracks

### Track L1 - Seed reduction and rollup rules (independent)

- Lock deterministic grouping/category keys in `tools/linear_seed_backlog.py`.
- Add impact scoring and pressure rollups without losing the leaf inventory needed for local auditing.
- Validation:
  - `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/test_linear_seed_backlog_tool.py`

### Track L2 - Local artifact refresh path (independent)

- Make `tools/linear_hygiene.py refresh-local-artifacts` produce grouped manifests and grouped index rows as the canonical local shape.
- Add/refresh tests for artifact counts, schema shape, and no-op reruns.
- Validation:
  - `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/test_linear_hygiene_tool.py`
  - `python3 tools/linear_hygiene.py refresh-local-artifacts --repo-root .`

### Track L3 - Sync/render stability (independent)

- Teach `tools/linear_workspace.py` to render grouped issues with stable sync keys, stable descriptions, and grouped metadata.
- Add focused tests for sync-key stability and grouped description rendering.
- Validation:
  - `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/test_linear_workspace_sync.py tests/test_linear_workspace_tool.py`

### Track L4 - Live migration and convergence (depends on L1-L3)

- Export the current live issue set.
- Run grouped sync into the Linear workspace.
- Close or replace leaf issues that no longer correspond to grouped manifests.
- Repeat dry-run sync until convergence is achieved or the remaining create pressure is only the intentional grouped residual.
- Validation:
  - `env -u LINEAR_API_KEY python3 tools/linear_workspace.py sync-index --team Moltlang --index ops/linear/manifests/index.json --update-existing --close-duplicates --close-missing --duplicate-state Canceled --dry-run`

## Exit gate

- `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/test_linear_hygiene_tool.py tests/test_linear_workspace_sync.py tests/test_linear_workspace_tool.py tests/test_linear_seed_backlog_tool.py`
- `python3 tools/linear_hygiene.py refresh-local-artifacts --repo-root .` is a no-op on a clean grouped backlog.
- The live sync dry run shows grouped convergence rather than leaf-issue churn.
