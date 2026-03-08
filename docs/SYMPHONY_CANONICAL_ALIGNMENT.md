# Symphony Canonical Alignment (Molt)

Last audited: 2026-03-04 (against `openai/symphony` `main` SPEC fetched on 2026-03-04)

## Canonical Sources Of Truth

The canonical source of truth for Symphony behavior is upstream OpenAI Symphony:

1. [Symphony README](https://github.com/openai/symphony/tree/main)
2. [Symphony SPEC](https://github.com/openai/symphony/blob/main/SPEC.md)
3. [Symphony Elixir README](https://github.com/openai/symphony/blob/main/elixir/README.md)

Local implementation must remain semantically aligned with those documents. This file is the local conformance ledger and should be updated whenever the local implementation or upstream spec changes.

## Conformance Ledger (SPEC Section 18.1 Required)

- Workflow path selection supports explicit runtime path and cwd default: `PASS`
  - [`src/molt/symphony/workflow.py`](src/molt/symphony/workflow.py)
- `WORKFLOW.md` loader with YAML front matter + prompt body split: `PASS`
  - [`src/molt/symphony/workflow.py`](src/molt/symphony/workflow.py)
  - [`tests/test_symphony_workflow.py`](tests/test_symphony_workflow.py)
- Typed config layer with defaults and `$` resolution: `PASS`
  - [`src/molt/symphony/config.py`](src/molt/symphony/config.py)
  - [`tests/test_symphony_config_workspace.py`](tests/test_symphony_config_workspace.py)
- Dynamic `WORKFLOW.md` watch/reload/re-apply for config and prompt: `PASS`
  - [`src/molt/symphony/orchestrator.py`](src/molt/symphony/orchestrator.py)
- Polling orchestrator with single-authority mutable state: `PASS`
  - [`src/molt/symphony/orchestrator.py`](src/molt/symphony/orchestrator.py)
- Issue tracker client with candidate fetch + state refresh + terminal fetch: `PASS`
  - [`src/molt/symphony/linear.py`](src/molt/symphony/linear.py)
- Workspace manager with sanitized per-issue workspaces: `PASS`
  - [`src/molt/symphony/workspace.py`](src/molt/symphony/workspace.py)
- Workspace lifecycle hooks (`after_create`, `before_run`, `after_run`, `before_remove`): `PASS`
  - [`src/molt/symphony/workspace.py`](src/molt/symphony/workspace.py)
- Hook timeout config (`hooks.timeout_ms`, default `60000`): `PASS`
  - [`src/molt/symphony/config.py`](src/molt/symphony/config.py)
- Coding-agent app-server subprocess client with JSON line protocol: `PASS`
  - [`src/molt/symphony/app_server.py`](src/molt/symphony/app_server.py)
- Codex launch command config (`codex.command`, default `codex --yolo app-server`): `PASS`
  - [`src/molt/symphony/config.py`](src/molt/symphony/config.py)
- OpenAI app-server v2 `item/tool/requestUserInput` response shape (`result.answers` map): `PASS`
  - [`src/molt/symphony/app_server.py`](src/molt/symphony/app_server.py)
  - [`tests/test_symphony_app_server_usage.py`](tests/test_symphony_app_server_usage.py)
- Strict prompt rendering with `issue` and `attempt` variables: `PASS`
  - [`src/molt/symphony/template.py`](src/molt/symphony/template.py)
  - [`tests/test_symphony_template.py`](tests/test_symphony_template.py)
- Exponential retry queue with continuation retries after normal exit: `PASS`
  - [`src/molt/symphony/orchestrator.py`](src/molt/symphony/orchestrator.py)
- Configurable retry backoff cap (`agent.max_retry_backoff_ms`, default 5m): `PASS`
  - [`src/molt/symphony/config.py`](src/molt/symphony/config.py)
- Reconciliation that stops runs on terminal/non-active tracker states: `PASS`
  - [`src/molt/symphony/orchestrator.py`](src/molt/symphony/orchestrator.py)
- Workspace cleanup for terminal issues (startup sweep + active transition): `PASS`
  - [`src/molt/symphony/orchestrator.py`](src/molt/symphony/orchestrator.py)
- Structured logs with `issue_id`, `issue_identifier`, and `session_id`: `PASS`
  - [`src/molt/symphony/orchestrator.py`](src/molt/symphony/orchestrator.py)
  - [`src/molt/symphony/logging_utils.py`](src/molt/symphony/logging_utils.py)
- Operator-visible observability (structured logs; optional snapshot/status surface): `PASS`
  - [`src/molt/symphony/http_server.py`](src/molt/symphony/http_server.py)
  - [`tests/test_symphony_http_server.py`](tests/test_symphony_http_server.py)

## Extension Notes (SPEC Section 18.2)

- Optional HTTP server extension: implemented (`/`, `/api/v1/state`, `/api/v1/<issue>`, `/api/v1/refresh`).
- Optional `linear_graphql` tool-call handling: implemented in app-server client/tool handler.
- Tracker-write orchestration remains workflow/agent-driven by design (matches spec boundary).
- Runtime loop uses event-driven wakeups for retry/poll responsiveness and lower idle CPU (compatible with spec tick/retry semantics).
- Runtime feature detection (`sys._is_gil_enabled`, free-threaded build signal, subinterpreter availability) is observability-only and does not alter spec-required behavior.

## Audit Procedure

When updating Symphony:

1. Re-read upstream docs listed in “Canonical Sources Of Truth”.
2. Re-run Symphony tests:
   - `PYTHONPATH=src pytest -q tests/test_symphony_*.py`
3. Re-run static checks:
   - `ruff check src/molt/symphony tools/symphony_*.py tools/linear_*.py`
4. Re-verify upstream spec text:
   - `curl -fsSL https://raw.githubusercontent.com/openai/symphony/main/SPEC.md | less`
   - `curl -fsSL https://raw.githubusercontent.com/openai/symphony/main/README.md | less`
5. Update this file with any changed status.
6. If a required conformance item regresses, treat it as a release blocker.
