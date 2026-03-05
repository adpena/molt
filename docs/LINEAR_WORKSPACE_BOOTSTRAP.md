# Linear Workspace Bootstrap (Molt)

This document explains how to set up and operate a Molt Linear workspace for Symphony.

## 1. Canonical Planning Inputs (Read First)

Use these documents as planning source-of-truth before creating/updating issues:

- `docs/spec/STATUS.md` (current capability truth)
- `ROADMAP.md` (active forward backlog)
- `OPTIMIZATIONS_PLAN.md` (optimization execution)
- `CONTRIBUTING.md` (change impact + evidence expectations)
- `docs/spec/areas/compat/README.md`
- `docs/spec/areas/compat/surfaces/language/language_surface_matrix.md`
- `docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_index.md`
- `docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md`
- `docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md`

## 2. Prerequisites

- Linear workspace URL: `https://linear.app/moltlang/`
- One-time auth:
  - Codex MCP OAuth: `codex mcp login linear`
  - Personal API key (required for direct GraphQL scripts and Symphony tracker runtime):

```bash
export LINEAR_API_KEY="<your-token>"
```

Notes:
- Linear API keys are managed at `Settings -> API -> Personal API keys`.
- If you do not see API key creation, ask a workspace admin to enable member API keys in workspace Administration settings.

## 3. One-command bootstrap (recommended)

Run this once per machine (or anytime you want to revalidate setup):

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_bootstrap.py \
  --project-slug "<linear-project-slug>" \
  --install-launchd \
  --launchd-port 8089
```

What this bootstraps automatically:

- verifies required harness commands (`codex`, `rg`, `uv`, `python3`)
- ensures Linear MCP server registration in Codex (`codex mcp add linear ...` if missing)
- writes `ops/linear/runtime/symphony.env` with external-volume defaults
- creates external runtime/cache/log directories under `/Volumes/APDataStore/Molt`
- auto-seeds official `lin` CLI credentials (`~/.lin/store.apiKey.json`) from `LINEAR_API_KEY` when `lin` is installed
- reports `formal_toolchain` status (Node/Quint/Lake direct and fallback probes)
- optionally installs persistent launchd service via `tools/symphony_launchd.py`

After bootstrap, recurring runs do not require manual setup each time.

If required keys are missing, copy the template and fill once:

```bash
cp ops/linear/runtime/symphony.env.example ops/linear/runtime/symphony.env
```

## 4. Inspect workspace state

Team discovery:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/linear_workspace.py whoami
PYTHONPATH=src uv run --python 3.12 python3 tools/linear_workspace.py list-projects --team <team-key-or-name>
PYTHONPATH=src uv run --python 3.12 python3 tools/linear_workspace.py list-states --team <team-key-or-name>
PYTHONPATH=src uv run --python 3.12 python3 tools/linear_workspace.py list-issues --team <team-key-or-name>
PYTHONPATH=src uv run --python 3.12 python3 tools/linear_workspace.py get-issue --team <team-key-or-name> --issue MOL-123
PYTHONPATH=src uv run --python 3.12 python3 tools/linear_workspace.py list-comments --team <team-key-or-name> --issue MOL-123
PYTHONPATH=src uv run --python 3.12 python3 tools/linear_workspace.py checkout-branch --team <team-key-or-name> --issue MOL-123 --create-if-missing
```

Direct non-interactive issue operations:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/linear_workspace.py create-issue \
  --team <team-key-or-name> --project <project-id-or-slug-or-name> \
  --title "Issue title" --description "Issue body"
PYTHONPATH=src uv run --python 3.12 python3 tools/linear_workspace.py update-issue \
  --team <team-key-or-name> --issue MOL-123 --state "In Progress"
PYTHONPATH=src uv run --python 3.12 python3 tools/linear_workspace.py comment-issue \
  --team <team-key-or-name> --issue MOL-123 --body "Progress update"
```

## 5. Build a repo-derived backlog (clean seeding)

Generate first-pass issues from TODO contracts:

```bash
uv run --python 3.12 python3 tools/linear_seed_backlog.py --repo-root . --output ops/linear/seed_backlog.json --max-items 200
```

Seed generation now drops obvious non-actionable TODO text and deduplicates aggressively.

## 6. Create/sync issues in Linear

One-shot create:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/linear_workspace.py bulk-create \
  --team <team-key-or-name> \
  --project <project-id-or-slug-or-name> \
  --manifest ops/linear/seed_backlog.json
```

Idempotent sync (safe to rerun):

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/linear_workspace.py sync-manifest \
  --team <team-key-or-name> \
  --project <project-id-or-slug-or-name> \
  --manifest ops/linear/manifests/runtime_and_intrinsics.json \
  --update-existing \
  --close-duplicates
```

Sync all categorized manifests:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/linear_workspace.py sync-index \
  --team <team-key-or-name> \
  --index ops/linear/manifests/index.json \
  --update-existing \
  --close-duplicates
```

Use `--dry-run` first for no-write planning output.

## 7. Run Symphony against Linear

Required env:

```bash
set -a
source ops/linear/runtime/symphony.env
set +a
```

Note: `MOLT_LINEAR_PROJECT_SLUG` may be a single slug or a comma-separated list of `project.slugId` values for workspace-wide dispatch.

Run once:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_run.py WORKFLOW.md --once
```

Run continuously:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_run.py WORKFLOW.md --port 8089
```

Dashboard/API:

- `http://127.0.0.1:8089/`
- `http://127.0.0.1:8089/api/v1/state`
- `http://127.0.0.1:8089/api/v1/stream` (realtime SSE)
- `http://127.0.0.1:8089/api/v1/<issue_identifier>`
- `POST http://127.0.0.1:8089/api/v1/refresh`

## 8. Optional persistent launchd service

Status:

```bash
uv run --python 3.12 python3 tools/symphony_launchd.py status
```

Install:

```bash
uv run --python 3.12 python3 tools/symphony_launchd.py install --repo-root . --port 8089 --exec-mode molt-bin --molt-profile dev
```

Uninstall:

```bash
uv run --python 3.12 python3 tools/symphony_launchd.py uninstall
```

Logs:

- `logs/symphony_launchd.out.log`
- `logs/symphony_launchd.err.log`

`tools/symphony_launchd.py install` now supports `--env-file` and `--ext-root`.
It also installs a watchdog service by default (`com.molt.symphony.watchdog`) to auto-restart Symphony when workflow/runtime files change.

## 9. Continuous readiness audit

Run the comprehensive readiness audit after sync/bootstrap and before long runs:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_readiness_audit.py --team Moltlang
```

This emits:

- `/Volumes/APDataStore/Molt/logs/symphony/readiness/latest.json`
- `/Volumes/APDataStore/Molt/logs/symphony/readiness/latest.md`

The audit checks Linear hygiene, manifest quality, launchd/watchdog wiring,
durable memory readability, and required docs/tooling coverage.

For hard autonomy gating (includes formal-suite inventory by default):

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_readiness_audit.py --team Moltlang --strict-autonomy --fail-on warn
```

For full formalization signal in readiness:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_readiness_audit.py --team Moltlang --strict-autonomy --fail-on warn --formal-suite all
```

To repair manifests/issues + ensure project assignment + bootstrap labels/routing in one pass:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/linear_hygiene.py full-pass --team Moltlang --apply --formal-suite all
```

Bootstrap writes this fallback by default (prefers local Node 22 binary, otherwise
`npx -y node@22`); if your local env is missing it, configure:

```bash
export MOLT_QUINT_NODE_FALLBACK='npx -y node@22'
```

For full `quint verify` lanes, Java is required by Apalache. Bootstrap now
auto-detects Homebrew OpenJDK and seeds `JAVA_HOME` when available; if needed:

```bash
export JAVA_HOME=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home
```

Quint/Apalache output is also routed via `MOLT_APALACHE_WORK_DIR` (default:
`/Volumes/APDataStore/Molt/tmp/apalache`) to keep `_apalache-out` artifacts off
the repo root.

## 10. Human operating loop (required)

- Human role and responsibilities are defined in:
  - `docs/SYMPHONY_HUMAN_ROLE.md`
  - `docs/SYMPHONY_OPERATOR_PLAYBOOK.md`
- Short form:
  1. Curate and prioritize issues from canonical planning docs.
  2. Let Symphony execute active issues.
  3. Review evidence (tests, benchmarks, docs sync).
  4. Move issues to terminal state only after gates pass.

## 11. Harness extras

- Repo-native code search helper:

```bash
uv run --python 3.12 python3 tools/code_search.py "TODO\\(" ROADMAP.md docs/spec/STATUS.md
uv run --python 3.12 python3 tools/code_search.py "molt_importlib_find_spec_orchestrate" src runtime --json
```
