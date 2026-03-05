# Symphony For Molt

This repository includes a native Python Symphony implementation for Molt.

## Canonical Source Of Truth

Symphony behavior is derived from upstream OpenAI Symphony:

- [openai/symphony](https://github.com/openai/symphony/tree/main)
- [SPEC.md](https://github.com/openai/symphony/blob/main/SPEC.md)
- [elixir/README.md](https://github.com/openai/symphony/blob/main/elixir/README.md)

Local conformance ledger:
- [docs/SYMPHONY_CANONICAL_ALIGNMENT.md](docs/SYMPHONY_CANONICAL_ALIGNMENT.md)

Human operator contract:
- [docs/SYMPHONY_HUMAN_ROLE.md](docs/SYMPHONY_HUMAN_ROLE.md)
- [docs/SYMPHONY_OPERATOR_PLAYBOOK.md](docs/SYMPHONY_OPERATOR_PLAYBOOK.md)

## What is implemented

- `WORKFLOW.md` loader with YAML front matter + strict prompt rendering
- Typed runtime config with defaults, environment resolution, and preflight validation
- Linear tracker adapter (candidate fetch, state refresh, terminal-state startup cleanup)
- Orchestrator state machine (dispatch, claims, retries, backoff, reconciliation)
- Built-in profiling telemetry (turn latency, dispatch latency, retry backoff, loop/event hotspots, CPU/RSS high-water)
- Durable telemetry memory on external volume (`events.jsonl` + optional `events.duckdb` / `events.parquet`)
- Per-issue workspace lifecycle with safety checks and workflow hooks
- Codex app-server JSON-line client (`initialize`, `thread/start`, `turn/start`)
- Agent tool-call registry:
  - `linear_graphql`
  - `molt_code_search`
  - `molt_cli`
  - `molt_formal_check` (Lean + Quint)
  - `symphony_state`
- Execution modes in launcher:
  - `python`
  - `molt-run`
  - `molt-bin`
- Optional HTTP observability endpoints:
  - `/`
  - `/api/v1/state`
  - `/api/v1/durable` (durable telemetry summary + recent historical events)
  - `/api/v1/stream` (Server-Sent Events realtime state feed)
  - `/api/v1/<issue_identifier>`
  - `/api/v1/refresh`
  - `/api/v1/interventions/retry-now`
  - `/api/v1/tools/run`

- Dashboard UX:
  - OpenAI-style dark mode by default
  - top-level command-nav views (`Overview`, `Interventions`, `Agents`, `Performance`, `Memory`, `All`)
  - intervention action center with one-click retry + tool launcher panel
  - durable memory panel (JSONL/DuckDB/Parquet file health + recent persisted event trail)
  - live concurrency tuning via `set_max_concurrent_agents` in the tool launcher
  - transport controller (`auto|sse|poll`) with frame-coalesced rendering and low-flicker panel diffing

- Agent role orchestration:
  - role tags can be inferred from Linear labels like `role:triage` or `swarm:formalizer`
  - policy-driven pools via `agent.default_role` and `agent.role_pools` in `WORKFLOW.md`

## Files

- `src/molt/symphony/`: implementation package
- `WORKFLOW.md`: repository-owned workflow contract
- `tools/symphony_bootstrap.py`: one-command setup for MCP/env/launchd
- `tools/symphony_run.py`: launch helper with external-volume checks
- `tools/symphony_git_sync.sh`: best-effort workspace git sync with author allowlist gating
- `tools/symphony_launchd.py`: launchd lifecycle helper for Symphony + watchdog
- `tools/symphony_watchdog.py`: file-watch daemon that auto-restarts Symphony service on changes

## Quick start

1. Bootstrap once:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_bootstrap.py \
  --project-slug "<linear-project-slug>" \
  --install-launchd \
  --launchd-port 8089
```

2. Load runtime env and run Symphony:

```bash
cp ops/linear/runtime/symphony.env.example ops/linear/runtime/symphony.env
set -a
source ops/linear/runtime/symphony.env
set +a
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_run.py WORKFLOW.md --port 8089
```

`MOLT_LINEAR_PROJECT_SLUG` supports a comma-separated list of Linear `project.slugId` values, so one Symphony service can dispatch across multiple projects.

3. Open dashboard:

- [http://127.0.0.1:8089/](http://127.0.0.1:8089/)
- [http://127.0.0.1:8089/api/v1/stream](http://127.0.0.1:8089/api/v1/stream)

4. Compare runtime modes (self-improvement perf loop):

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_perf.py WORKFLOW.md --iterations 3
```

This writes a JSON report under `/Volumes/APDataStore/Molt/logs/symphony/` by default.

Optional dashboard API efficiency baseline in the same run:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_perf.py WORKFLOW.md \
  --dashboard-url http://127.0.0.1:8089 --api-samples 80 --api-interval-ms 250
```

## Security + Secret Hygiene

- Keep secrets in environment variables only.
- Do not commit `.env` or token files.
- Do not put raw secrets in `WORKFLOW.md`, docs, or Linear issue descriptions.
- Use `.gitignore`-protected local files for machine-specific overrides (`WORKFLOW.local.md`, `.env`, `ops/linear/*.secret*`).

## Operational notes

- Workspace paths are enforced under `workspace.root`.
- Issue workspace names are sanitized to `[A-Za-z0-9._-]`.
- `after_create` hook failures fail the attempt.
- `before_run` git sync is best-effort: dirty/diverged workspaces are logged and skipped (non-fatal) to avoid retry flicker loops.
- `after_run` and `before_remove` hook failures are logged and ignored.
- Automated sync/merge behavior is author-gated via `MOLT_SYMPHONY_AUTOMERGE_ALLOWED_AUTHORS` (default `adpena,symphony`).
- Unknown prompt variables and unknown filters fail rendering.
- Unsupported dynamic tool calls are rejected without stalling the session.
- Rate-limit exhaustion now activates a system suspension (`rate_limited`) and auto-resume window instead of hot-loop retries.
- Missing Codex auth / input-required states now activate a system suspension (`auth_required`) with human prompt text; Symphony retries automatically after the configured resume delay.
- Dashboard/API state payload now includes `profiling` and `runtime.exec_mode` fields.
- Dashboard/API state payload includes `agent_panes`, runtime role pool settings, token throughput (`codex_totals.tokens_per_second`), and suspension metadata (`suspension`).
- `/api/v1/state` now supports conditional reads via `ETag` + `If-None-Match` to avoid re-downloading unchanged state during fallback polling.
- `/api/v1/stream` now emits `state` events only when the serialized snapshot changes (plus heartbeats), reducing UI churn and endpoint pressure.
- `symphony_state` defaults to compact payload mode with short TTL caching for lower token burn; use `{ "detail": "full" }` when agents need full raw state.
- `symphony_state` also supports `{ "detail": "telemetry" }` for agent-native, token-efficient MCP telemetry.
- Codex event profiling counters are cardinality-bounded (`MOLT_SYMPHONY_MAX_CODEX_EVENT_COUNTERS`, default `64`) to avoid unbounded metric growth.
- Durable memory files are external-volume first (`MOLT_SYMPHONY_DURABLE_MEMORY=1`), with auto-materialization into DuckDB/Parquet when `duckdb` is available.
