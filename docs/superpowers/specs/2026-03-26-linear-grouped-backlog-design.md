# Linear Grouped Backlog Design

**Date:** 2026-03-26

**Problem**

`ops/linear` currently mirrors repo TODO contracts almost one-for-one into Linear issue candidates. That preserves detail, but it overwhelms the live workspace, exceeds Linear's free active-issue cap, and makes prioritization noisy because many related leaf items compete at the same level.

**Goals**

- Keep the repo-derived TODO inventory as the source of truth.
- Emit a much smaller, deterministic set of grouped Linear issues.
- Preserve strong priority and impact signal after grouping.
- Make local `ops/linear` artifacts and live Linear sync converge to the grouped model.
- Avoid reintroducing drift on subsequent refreshes/syncs.

**Non-Goals**

- Do not discard leaf-level TODO detail.
- Do not hand-curate project-specific grouping rules that cannot be reproduced from repo state.
- Do not depend on live Linear state to build grouped manifests.

## Design

### 1. Two-layer backlog model

The seed backlog remains the full normalized/deduped leaf inventory derived from repo TODO contracts. A new grouping stage reduces those leaf items into canonical umbrella issues that become the live-manifest shape.

The new model has:

- **Leaf inventory:** the current normalized TODO-level issue records.
- **Grouped manifest:** a deterministic reduction of leaf items by project plus a stable category key.

The grouped manifest becomes the canonical input for `sync-index`.

### 2. Stable grouping key

Each leaf item is assigned to a project as today, then grouped by a stable category key derived from:

- project
- owner lane
- area family
- milestone family

The grouping rule should be deterministic and conservative:

- same project is mandatory
- same owner lane is mandatory
- same milestone family is preferred
- related areas may fold together under a shared family when they represent one operational workstream

Examples:

- Runtime + stdlib intrinsic migration leaf items collapse into a small set of runtime/stdlib umbrellas instead of hundreds of per-module leaves.
- Compiler optimization hardening items collapse into a few optimization/lowering/coverage umbrellas.
- Tooling diff/daemon/cache items collapse into a few tooling throughput umbrellas.

### 3. Strong priority and impact rollup

Grouped issues must preserve urgency rather than averaging it away.

Every grouped issue will carry:

- **Linear priority:** the highest urgency leaf in the group.
- **Impact score:** weighted rollup derived from leaf count, urgency mix, and partial/missing status pressure.
- **Pressure summary:** explicit counts for `P0`, `P1`, `missing`, `partial`, and total leaf items.
- **Representative sources:** stable sample of source TODO locations.

The grouped title should start with a strong signal:

- `[P0][High Impact][RT2] Runtime async core parity backlog`
- `[P1][Medium Impact][TL2] Tooling throughput + cache backlog`

The grouped description should make the signal concrete:

- why this group is high impact
- counts by priority/status
- first-order work buckets
- leaf checklist or compact bullet inventory

### 4. Grouped manifest schema

Grouped manifest items keep the existing shape so current tooling can keep working with minimal churn, but metadata expands.

Required fields:

- `title`
- `description`
- `priority`
- `metadata`

New metadata fields:

- `kind: grouped`
- `group_key`
- `impact`
- `impact_score`
- `leaf_count`
- `priority_counts`
- `status_counts`
- `milestones`
- `owners`
- `sources`

Descriptions should include a compact operator-facing structure:

1. summary paragraph
2. impact/pressure summary
3. grouped sub-buckets
4. compact leaf checklist

### 5. Sync and consolidation behavior

`sync-index` should sync grouped manifests, not leaf manifests.

Consolidation rules:

- existing grouped issue with the same title/key is canonical
- matching duplicates close as duplicates/canceled as today
- leaf issues superseded by a grouped issue should be considered non-canonical drift
- local hygiene should be able to rebuild grouped manifests without depending on live Linear contents

The live migration path is:

1. rebuild grouped local manifests
2. sync grouped issues into Linear
3. close or permanently delete superseded leaf issues not represented in grouped manifests

### 6. Cap-aware output

The grouped stage should explicitly target a bounded active-issue count.

Default behavior:

- emit all groups when they fit
- otherwise rank groups by priority and impact score and emit the top cap-fitting set

This keeps the live workspace within the free-tier ceiling while still making the highest-value grouped backlog visible.

### 7. Verification

Local verification must prove:

- grouping is deterministic
- grouped manifests preserve worst-case priority correctly
- impact scoring is stable and explainable
- refresh-local-artifacts converges to no-op after apply
- sync planning prefers grouped issues and reduces create volume sharply

Live verification must prove:

- active issue count stays below the workspace cap
- grouped issues exist with correct project/priority/description metadata
- superseded leaf drift is removed

## Recommended Implementation Order

1. Add grouping primitives and tests in `tools/linear_seed_backlog.py`.
2. Teach `tools/linear_hygiene.py refresh-local-artifacts` to emit grouped manifests/index rows.
3. Add grouped-manifest aware sync tests in `tools/linear_workspace.py`.
4. Regenerate local `ops/linear` artifacts.
5. Migrate the live Linear workspace to the grouped/category model.
