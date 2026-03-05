# Symphony Red-Team Checklist

Use this checklist before considering Symphony "internet-ready" or "hardened".

## 1. Identity + Secrets

- `LINEAR_API_KEY` is set only in ignored env files or shell env.
- `MOLT_SYMPHONY_API_TOKEN` (or auto-generated token file) is required for operator access.
- `tools/secret_guard.py --staged` is active through `.githooks/pre-commit`.
- Any previously exposed tokens have been rotated.

## 2. Network + Surface Area

- Default bind host remains loopback (`MOLT_SYMPHONY_BIND_HOST=127.0.0.1`).
- Non-loopback bind is disabled unless explicitly required (`MOLT_SYMPHONY_ALLOW_NONLOCAL_BIND=1`).
- Allowed origins are explicit for deployed environments.
- Query-token auth is disabled in production profile.

## 3. Browser/API Guardrails

- Origin checks and CSRF header checks are enabled.
- API responses include secure headers (`nosniff`, frame deny, COOP/CORP, CSP).
- Dashboard UI can be disabled for API-only production mode.

## 4. Capacity + Abuse Resistance

- HTTP request concurrency cap is set (`MOLT_SYMPHONY_MAX_HTTP_CONNECTIONS`).
- SSE client cap and max stream age are set.
- Orchestrator event queue is bounded (`MOLT_SYMPHONY_EVENT_QUEUE_MAX`).
- Dropped-event counters are monitored via dashboard.

## 5. Failure Handling

- Auth-required and rate-limit suspension states are tested.
- Auto-resume behavior is validated for both auth and quota pauses.
- Manual intervention actions produce visible feedback in dashboard.

## 6. Durable Memory Safety

- `tools/symphony_durable_admin.py check` passes.
- `backup` and `restore` paths are tested.
- `prune` retention policy is configured and exercised.
- `.duckdb` / `.parquet` files live on external storage.

## 7. Supply Chain + CI

- `cargo deny check`, `cargo audit`, and `pip-audit` pass.
- If `cargo audit` reports warning-only unmaintained crates, each one is tracked with a migration owner and target milestone.
- Symphony hardening tests pass in CI (`security_hardening.yml`).
- Security findings are triaged and documented.

## 8. Human Role (Operational)

- Humans triage only issues requiring intervention and avoid ad-hoc direct edits in agent workspaces.
- Human actions are performed through dashboard tools where possible.
- Human escalation paths are documented for: auth failure, quota exhaustion, corrupted durable data, and stuck retries.
