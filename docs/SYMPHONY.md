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

Harness engineering alignment:
- [docs/HARNESS_ENGINEERING.md](docs/HARNESS_ENGINEERING.md)
- [docs/QUALITY_SCORE.md](docs/QUALITY_SCORE.md)
- [docs/exec-plans/TEMPLATE.md](docs/exec-plans/TEMPLATE.md)

Canonical storage layout:
- compiler/build artifacts stay under `MOLT_EXT_ROOT` (default `/Volumes/APDataStore/Molt`)
- long-lived Symphony logs/state/artifacts live under the shared parent
  `/Volumes/APDataStore/symphony/<project>` with Molt defaulting to
  `/Volumes/APDataStore/symphony/molt`

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
  - `/api/v1/activity`
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
- `tools/symphony_run.py`: launch helper with external-volume checks and optional wait-for-mount startup behavior for launchd
- `tools/symphony_git_sync.sh`: best-effort workspace git sync with author allowlist gating
- `tools/symphony_launchd.py`: launchd lifecycle helper for Symphony + watchdog
- `tools/symphony_watchdog.py`: file-watch daemon that auto-restarts Symphony service on changes and repairs unhealthy launchd jobs via authenticated health probes
- `tools/symphony_dlq.py`: inspect and replay recursive-loop dead-letter items
- `tools/symphony_taste_memory.py`: inspect and distill recursive learning memory
- `tools/symphony_tool_promotion.py`: distill recurring successful actions into explicit tool-promotion candidates

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

BAT00 host orchestration and machine-control tooling is maintained in the private
`fleet` repository (not in public `molt`). Use Fleet MCP tools for remote
operations:

- `molt_symphony_bat00_doctor`
- `molt_symphony_bat00_repair`
- `molt_symphony_bat00_start`
- `molt_symphony_bat00_stop`
- `molt_symphony_bat00_status`
- `molt_symphony_bat00_telemetry`
- `molt_symphony_bat00_set_concurrency`

Bootstrap output now includes `formal_toolchain` with direct/fallback probes for
Node + Quint + Lake so you can confirm formal tooling health before long runs.

`MOLT_LINEAR_PROJECT_SLUG` supports a comma-separated list of Linear `project.slugId` values, so one Symphony service can dispatch across multiple projects.
For scripted Linear operations, use `tools/linear_workspace.py` as the canonical CLI path.

3. Open dashboard:

- [http://127.0.0.1:8089/](http://127.0.0.1:8089/)
- [http://127.0.0.1:8089/api/v1/activity](http://127.0.0.1:8089/api/v1/activity)
- [http://127.0.0.1:8089/api/v1/health](http://127.0.0.1:8089/api/v1/health)
- [http://127.0.0.1:8089/api/v1/stream](http://127.0.0.1:8089/api/v1/stream)

Launchd hardening details:
- launchd control-plane stdout/stderr now live under `~/Library/Logs/Molt/symphony-launchd/` so the jobs can start even when the external volume is temporarily absent
- the main service waits for the canonical external roots instead of crash-looping on missing mounts
- watchdog health repair uses the authenticated lightweight `/api/v1/health` endpoint instead of the heavier `/api/v1/state` payload
- watchdog busy/perf deferral now uses the authenticated lightweight `/api/v1/activity` endpoint instead of polling full state projections

4. Compare runtime modes (self-improvement perf loop):

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_perf.py WORKFLOW.md --iterations 3
```

This writes JSON reports under `/Volumes/APDataStore/symphony/molt/logs/` by default.

Optional client-side WASM lane (Python -> Molt -> WASM) for dashboard kernels:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_dashboard_wasm.py \
  --profile release \
  --output /Volumes/APDataStore/Molt/wasm/symphony/dashboard_kernel.wasm
```

Kernel source is `src/molt/symphony/dashboard_kernel.py`.

Dashboard JS checks for optional runtime hooks on `window.__MOLT_SYMPHONY_KERNEL__`
(or `window.MoltSymphonyKernel`) with:
- `classifyEventTone(eventName: str) -> str`
- `classifyTraceStatus(status: str) -> str`
- `compactRecentEvents(rows, limit) -> list`

This allows a future WASM bridge to swap in without changing dashboard UI code.

Bridge runtime:
- static bridge asset: `/dashboard-kernel-bridge.js`
- wasm fetch path (default): `/dashboard-kernel.wasm`
- server env override: `MOLT_SYMPHONY_DASHBOARD_KERNEL_WASM_PATH=/abs/path/dashboard_kernel.wasm`
- optional adapter hook: `window.__MOLT_SYMPHONY_KERNEL_WASM_ADAPTER__` (maps wasm exports to hook functions)

Bridge profiling is emitted to `window.__MOLT_SYMPHONY_KERNEL_PROFILE__` and included under
`window.__MOLT_SYMPHONY_CLIENT_TELEMETRY__.kernel`.

Optional dashboard API efficiency baseline in the same run:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_perf.py WORKFLOW.md \
  --dashboard-url http://127.0.0.1:8089 --api-samples 80 --api-interval-ms 250
```

Compare current run vs a previous report to detect regressions:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_perf.py WORKFLOW.md \
  --iterations 3 \
  --compare-with /Volumes/APDataStore/symphony/molt/logs/symphony_perf_<previous>.json
```

Strict regression gate (CI/local hard-fail budget):

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_perf.py WORKFLOW.md \
  --iterations 3 \
  --compare-with /Volumes/APDataStore/symphony/molt/logs/symphony_perf_<previous>.json \
  --max-avg-regression-ratio 0.15 \
  --max-p95-regression-ratio 0.20 \
  --max-dashboard-avg-latency-regression-ms 5 \
  --fail-on-regression
```

Exit codes:
- `0`: success (no sample failures; no threshold breach when gating enabled)
- `2`: benchmark execution failure in at least one sample
- `3`: regression threshold breach with `--fail-on-regression`

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
- Secret-guard blocked commits emit security events to `MOLT_SYMPHONY_SECURITY_EVENTS_FILE` (default `/Volumes/APDataStore/symphony/molt/logs/security/events.jsonl`), surfaced in dashboard `Security Telemetry`.
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

### Readiness Audit

Run the comprehensive readiness audit before/after major orchestration changes:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_readiness_audit.py --team Moltlang
```

Outputs are written under:

- `/Volumes/APDataStore/symphony/molt/logs/readiness/latest.json`
- `/Volumes/APDataStore/symphony/molt/logs/readiness/latest.md`
- `/Volumes/APDataStore/symphony/molt/logs/readiness/next_tranche.json`
- `/Volumes/APDataStore/symphony/molt/logs/readiness/next_tranche.md`
- `sections.dlq_health` reports replay backlog and recurring unresolved fingerprints
- `sections.tool_promotion` reports latest candidate and ready-candidate counts
- top-level `improvement_issue_sync` emits a dry-run or applied Linear issue sync plan for DLQ backlog and promotion-ready candidates

### Recursive Loop Runner

Run deterministic harness cycles that bundle readiness, Linear hygiene, and
trend deltas into per-cycle artifacts:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_recursive_loop.py --quick
```

Full lane with strict-autonomy + formal-all, optional perf guard, and optional
execution of `next_tranche.actions`:

```bash
PYTHONPATH=src uv run --group dev --python 3.12 python3 tools/symphony_recursive_loop.py \
  --formal-suite all \
  --run-perf-guard \
  --execute-next-tranche
```

Artifacts are written under:

- `/Volumes/APDataStore/symphony/molt/logs/recursive_loop/<timestamp>-cycleXX/summary.json`
- `/Volumes/APDataStore/symphony/molt/logs/recursive_loop/<timestamp>-cycleXX/summary.md`

Recursive-loop self-improvement surfaces:

- dead-letter queue: `/Volumes/APDataStore/symphony/molt/state/dlq/events.jsonl`
- taste-memory events: `/Volumes/APDataStore/symphony/molt/state/taste_memory/events.jsonl`
- taste-memory distillations:
  `/Volumes/APDataStore/symphony/molt/state/taste_memory/distillations/`
- tool-promotion events:
  `/Volumes/APDataStore/symphony/molt/state/tool_promotion/events.jsonl`
- tool-promotion distillations:
  `/Volumes/APDataStore/symphony/molt/state/tool_promotion/distillations/`
- optional typed hook command: `MOLT_SYMPHONY_LOOP_HOOK_CMD`

DLQ inspection/replay:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_dlq.py summary --limit 20
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_dlq.py replay --fingerprint <id> --dry-run
```

The DLQ summary now includes replay health:
- `open_failure_count`
- `recurring_open_fingerprints`
- `replay_success_count`
- `replay_failure_count`
- `recommended_replay_target`

Taste-memory distillation:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_taste_memory.py distill --limit 50
```

Tool-promotion distillation:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_tool_promotion.py distill --limit 200 --min-success-count 3
```

Ready candidates now also emit reviewable manifest files under the tool-promotion
state root (`.../state/tool_promotion/manifests/`).

Optional Linear improvement issue sync planning is part of readiness by default,
and can be applied explicitly:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_readiness_audit.py \
  --team Moltlang \
  --sync-improvement-issues \
  --improvement-issue-project "Tooling & DevEx"
```

The audit covers Linear workspace hygiene, manifest quality, docs/tooling coverage,
launchd/watchdog wiring, durable memory readability, and a harness engineering
score (`sections.harness_engineering.score`, target `>= 90`). DuckDB lock
contention while the live service is writing is reported as warning-only
(`duckdb_locked_by_writer`).

Readiness also computes trend-aware guardrails and synthesizes deterministic
next-tranche actions:
- `sections.trend_analysis` includes harness score delta, active-flow ratio,
  formal pass ratio, and recurring durable-growth pressure over a bounded window.
- `next_tranche.actions` emits prioritized remediation commands mapped from
  current findings (`P0`..`P3`).
- tune thresholds with:
  - `--trend-window`
  - `--max-harness-score-drop`
  - `--min-active-flow-ratio`
  - `--min-formal-pass-ratio`
  - `--max-durable-growth-ratio`

Strict autonomy mode (promotes metadata/title drift and no-active-flow warnings
to failures). This also runs formal-suite inventory by default:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_readiness_audit.py --team Moltlang --strict-autonomy --fail-on warn
```

For full formalization signal (Lean + Quint) in readiness:

```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_readiness_audit.py --team Moltlang --strict-autonomy --fail-on warn --formal-suite all
```

Bootstrap/env defaults now set this automatically (and use a local Node 22 binary
path when available, otherwise `npx -y node@22`):

```bash
export MOLT_QUINT_NODE_FALLBACK='npx -y node@22'
```

This remains the recommended override if local environment drift removes the key.
For full Quint verify runs, Java is also required (Apalache backend). Bootstrap now
auto-detects Homebrew OpenJDK and sets `JAVA_HOME` when possible; if needed:

```bash
export JAVA_HOME=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home
```

Bootstrap/runtime also seed `MOLT_APALACHE_WORK_DIR` (default:
`/Volumes/APDataStore/Molt/tmp/apalache`) so Quint/Apalache `_apalache-out`
artifacts stay off the repo root.

### Linear Hygiene + Swarm Routing

Run the full Linear hygiene pass (manifest title repair, seeded metadata backfill,
label taxonomy bootstrap, role-label routing, and active-flow promotion):

```bash
PYTHONPATH=src uv run --group dev --python 3.12 python3 tools/linear_hygiene.py full-pass --team Moltlang --apply --formal-suite all
```

`tools/linear_hygiene.py` supports optional DSPy-assisted role routing.
Set:

- `uv sync --group dev --python 3.12`
- `MOLT_SYMPHONY_DSPY_ENABLE=1`
- `MOLT_SYMPHONY_DSPY_MODEL=<provider/model>`
- `MOLT_SYMPHONY_DSPY_API_KEY_ENV=<env-var-name>` (defaults to `OPENAI_API_KEY`)
- `<env-var-name>=<token>`

When DSPy is not configured, deterministic heuristics are used and all outputs
remain valid for swarm routing (`role:*` labels consumed by Symphony).
It also reports `linear_cli_compat` to explicitly flag npm `lin` schema drift and confirm native `tools/linear_workspace.py` health path.

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
- Dashboard/API state payload now includes `profiling_compare` (baseline checkpoint count, regressions, improvements, and optimization candidates).
- Dashboard/API state payload includes `agent_panes`, runtime role pool settings, token throughput (`codex_totals.tokens_per_second`), and suspension metadata (`suspension`).
- `/api/v1/state` now supports conditional reads via `ETag` + `If-None-Match` to avoid re-downloading unchanged state during fallback polling.
- Dashboard HTTP now uses a short shared snapshot cache for `/api/v1/state` and `/api/v1/stream` to reduce duplicate serialization under concurrent readers.
- Observability projection is split from transport: `http_server.py` owns protocol/security handling while `observability_presenter.py` owns payload projection, secret redaction, and security-event summary shaping.
- `/api/v1/stream` now emits `state` events only when the serialized snapshot changes (plus heartbeats), reducing UI churn and endpoint pressure.
- Fallback polling now uses adaptive backoff (error/not-modified aware) to avoid endpoint thrash while preserving realtime responsiveness.
- Protected dashboard/API requests now support per-principal HTTP rate limiting with explicit `429` + `Retry-After` (`MOLT_SYMPHONY_HTTP_RATE_LIMIT_MAX_REQUESTS`, `MOLT_SYMPHONY_HTTP_RATE_LIMIT_WINDOW_SECONDS`).
- Optional state-hash helper integration (`MOLT_SYMPHONY_STATE_HASH_HELPER`) supports a compiled-Molt helper binary (`tools/symphony_state_hasher.py`) with framed binary mode (`--stdio-frame`) for lower overhead, automatic legacy text fallback (`--stdio`), and transparent fallback to in-process Python hashing when helper execution fails.
- `MOLT_SYMPHONY_STATE_HASH_HELPER_PREFER_FRAME` controls protocol preference when no explicit helper mode is provided (`1` by default).
- `symphony_state` defaults to compact payload mode with short TTL caching for lower token burn; use `{ "detail": "full" }` when agents need full raw state.
- `symphony_state` also supports `{ "detail": "telemetry" }` for agent-native, token-efficient MCP telemetry.
- Codex event profiling counters are cardinality-bounded (`MOLT_SYMPHONY_MAX_CODEX_EVENT_COUNTERS`, default `64`) to avoid unbounded metric growth.
- Durable memory files are external-volume first (`MOLT_SYMPHONY_DURABLE_MEMORY=1`), with auto-materialization into DuckDB/Parquet when `duckdb` is available.
- Profiling checkpoints are sampled periodically and persisted into durable memory (`MOLT_SYMPHONY_PROFILING_CHECKPOINT_INTERVAL_SECONDS`, default `20`), then compared against a rolling historical baseline (`MOLT_SYMPHONY_PROFILING_BASELINE_MAX_EVENTS`, `MOLT_SYMPHONY_PROFILING_BASELINE_MAX_LABELS`).
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
