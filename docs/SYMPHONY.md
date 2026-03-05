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
- `src/molt/symphony/http_server.py`: hardened dashboard/API transport layer (auth, rate limits, SSE, cache/ETag)
- `src/molt/symphony/observability_presenter.py`: observability payload projection + redaction + security-event summary helpers
- `src/molt/symphony/dashboard_assets.py`: embedded dashboard static assets + precomputed weak ETags
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

Optional hasher A/B micro-benchmark (Python vs helper process):

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_perf.py WORKFLOW.md \
  --hash-bench-iterations 2000 --hash-bench-bytes 65536 \
  --hash-helper-cmd "/usr/bin/python3 tools/symphony_state_hasher.py"
```

Compile the helper with Molt (release + max optimization) and wire it:

```bash
PYTHONPATH=src uv run --python 3.12 python3 -m molt.cli build \
  --profile release --optimize max \
  --output /Volumes/APDataStore/Molt/bin/symphony_state_hasher_molt \
  tools/symphony_state_hasher.py
export MOLT_SYMPHONY_STATE_HASH_HELPER="/Volumes/APDataStore/Molt/bin/symphony_state_hasher_molt"
```

## Security + Secret Hygiene

- Keep secrets in environment variables only.
- Do not commit `.env` or token files.
- Do not put raw secrets in `WORKFLOW.md`, docs, or Linear issue descriptions.
- Use `.gitignore`-protected local files for machine-specific overrides (`WORKFLOW.local.md`, `.env`, `ops/linear/*.secret*`).
- `tools/secret_guard.py --staged` is enforced by `.githooks/pre-commit` (installed by `tools/symphony_bootstrap.py` via `core.hooksPath=.githooks` unless a custom hooks path already exists).
- For intentional fake fixtures, add `# secret-guard: allow` on the specific test line.
- Secret-guard blocked commits emit security events to `MOLT_SYMPHONY_SECURITY_EVENTS_FILE` (default `/Volumes/APDataStore/Molt/logs/symphony/security/events.jsonl`), surfaced in dashboard `Security Telemetry`.
- Network bind is loopback-only by default (`MOLT_SYMPHONY_BIND_HOST=127.0.0.1`). Non-loopback bind requires explicit opt-in (`MOLT_SYMPHONY_ALLOW_NONLOCAL_BIND=1`).
- `MOLT_SYMPHONY_SECURITY_PROFILE=production` enables stricter startup rules (API token required; dashboard UI disabled by default via `MOLT_SYMPHONY_DISABLE_DASHBOARD_UI=1`; query-token auth disabled by default via `MOLT_SYMPHONY_ALLOW_QUERY_TOKEN=0`).
- Full adversarial review checklist: [docs/SYMPHONY_RED_TEAM_CHECKLIST.md](docs/SYMPHONY_RED_TEAM_CHECKLIST.md).

### Hardening Gate (Local)

```bash
uv run --python 3.12 ruff check .
uv run --python 3.12 ty check src
uv run --python 3.12 pytest -q tests/test_symphony_http_server.py tests/test_symphony_observability_presenter.py tests/test_symphony_orchestrator_retry.py tests/test_symphony_runtime_tools.py tests/test_symphony_bootstrap_tool.py tests/test_secret_guard.py tests/test_secret_guard_tool.py tests/test_symphony_durable_memory.py tests/test_symphony_durable_admin_tool.py
cargo deny check
cargo audit
uv run --python 3.12 pip-audit
```

### Supply-Chain Status (Current)

- `cargo deny check` is green for `advisories`, `bans`, `licenses`, and `sources`.
- `cargo audit` currently reports warning-only unmaintained crates (no known exploitable CVE lane in this set):
  - `egg` transitive stack (`fxhash`, `instant`)
  - `serde_cbor`
  - `rustpython-parser` transitive `unic-*` crates
- These are migration tasks, not ignored secrets or policy bypasses. Treat them as scheduled risk retirement work.

### Durable Memory Admin

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_durable_admin.py summary
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_durable_admin.py check
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_durable_admin.py backup --reason manual
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_durable_admin.py restore
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_durable_admin.py prune --keep-latest 20 --max-age-days 30
```

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
- Dashboard HTTP now uses a short shared snapshot cache for `/api/v1/state` and `/api/v1/stream` to reduce duplicate serialization under concurrent readers.
- Observability projection is split from transport: `http_server.py` owns protocol/security handling while `observability_presenter.py` owns payload projection, secret redaction, and security-event summary shaping.
- `/api/v1/stream` now emits `state` events only when the serialized snapshot changes (plus heartbeats), reducing UI churn and endpoint pressure.
- Fallback polling now uses adaptive backoff (error/not-modified aware) to avoid endpoint thrash while preserving realtime responsiveness.
- Protected dashboard/API requests now support per-principal HTTP rate limiting with explicit `429` + `Retry-After` (`MOLT_SYMPHONY_HTTP_RATE_LIMIT_MAX_REQUESTS`, `MOLT_SYMPHONY_HTTP_RATE_LIMIT_WINDOW_SECONDS`).
- Optional state-hash helper integration (`MOLT_SYMPHONY_STATE_HASH_HELPER`) supports a compiled-Molt helper binary (`tools/symphony_state_hasher.py`) with transparent fallback to Python hashing.
- `symphony_state` defaults to compact payload mode with short TTL caching for lower token burn; use `{ "detail": "full" }` when agents need full raw state.
- `symphony_state` also supports `{ "detail": "telemetry" }` for agent-native, token-efficient MCP telemetry.
- Codex event profiling counters are cardinality-bounded (`MOLT_SYMPHONY_MAX_CODEX_EVENT_COUNTERS`, default `64`) to avoid unbounded metric growth.
- Durable memory files are external-volume first (`MOLT_SYMPHONY_DURABLE_MEMORY=1`), with auto-materialization into DuckDB/Parquet when `duckdb` is available.
- Dashboard/API hardening controls:
  - `MOLT_SYMPHONY_API_TOKEN` (or `MOLT_SYMPHONY_DASHBOARD_TOKEN`) enables authenticated API access (`Authorization: Bearer <token>`).
  - `tools/symphony_run.py` auto-provisions `MOLT_SYMPHONY_API_TOKEN` into `MOLT_SYMPHONY_API_TOKEN_FILE` when no token is supplied (external-volume default path, mode `0600` best-effort).
  - `MOLT_SYMPHONY_ENFORCE_ORIGIN=1` enforces mutating-request origin checks; `MOLT_SYMPHONY_ALLOWED_ORIGINS` can pin explicit origins.
  - `MOLT_SYMPHONY_REQUIRE_CSRF_HEADER=1` requires `X-Symphony-CSRF: 1` on browser-origin mutating requests.
  - `MOLT_SYMPHONY_MAX_HTTP_CONNECTIONS` bounds concurrent HTTP request handling.
  - `MOLT_SYMPHONY_MAX_STREAM_CLIENTS` and `MOLT_SYMPHONY_STREAM_MAX_AGE_SECONDS` bound SSE fanout and stream lifetime.
  - `MOLT_SYMPHONY_HTTP_RATE_LIMIT_MAX_REQUESTS` + `MOLT_SYMPHONY_HTTP_RATE_LIMIT_WINDOW_SECONDS` bound authenticated API/dashboard request burst rates per principal.
- Orchestrator event ingestion is bounded by `MOLT_SYMPHONY_EVENT_QUEUE_MAX`; dropped non-critical events are counted via profiling counters (`events_dropped*`) rather than unbounded memory growth.
- `runtime.event_queue` in `/api/v1/state` now exposes queue depth/capacity/utilization plus dropped-event counts for fast operational diagnosis.
