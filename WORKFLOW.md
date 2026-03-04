---
tracker:
  kind: linear
  endpoint: https://api.linear.app/graphql
  api_key: $LINEAR_API_KEY
  project_slug: $MOLT_LINEAR_PROJECT_SLUG
  active_states:
    - Todo
    - In Progress
    - Rework
  terminal_states:
    - Done
    - Closed
    - Cancelled
    - Canceled
    - Duplicate
polling:
  interval_ms: 30000
workspace:
  root: $MOLT_EXT_ROOT/symphony_workspaces
hooks:
  after_create: |
    git clone --depth 1 "$MOLT_SOURCE_REPO_URL" .
  before_run: |
    git fetch --all --prune
    git checkout main
    git pull --ff-only origin main
  after_run: |
    git status --short
  timeout_ms: 120000
agent:
  max_concurrent_agents: 4
  max_turns: 20
  max_retry_backoff_ms: 300000
  default_role: executor
  role_pools:
    executor: 3
    triage: 1
    formalizer: 1
    reviewer: 1
  max_concurrent_agents_by_state:
    in progress: 4
    todo: 4
    rework: 2
codex:
  command: ${CODEX_BIN:-codex} app-server
  approval_policy:
    reject:
      sandbox_approval: true
      rules: true
      mcp_elicitations: true
  thread_sandbox: workspace-write
  turn_sandbox_policy:
    type: workspaceWrite
  turn_timeout_ms: 3600000
  read_timeout_ms: 5000
  stall_timeout_ms: 300000
---
You are working on a Linear issue for the Molt compiler/runtime project.

Issue: {{ issue.identifier }}
Title: {{ issue.title }}
State: {{ issue.state }}
Priority: {{ issue.priority | default: "unranked" }}
URL: {{ issue.url | default: "n/a" }}
Attempt: {{ attempt | default: "first-run" }}

Description:
{{ issue.description | default: "(no description provided)" }}

Non-negotiable execution policy:
- Read and follow `AGENTS.md` in this repository before making changes.
- Do not add fake behavior, narrow test-only fixes, or host-Python fallback paths.
- Keep stdlib behavior Rust-lowered via intrinsics where applicable.
- Respect external-volume constraints (`/Volumes/APDataStore/Molt`) for heavy workflows.

Delivery requirements:
- Implement the issue end-to-end with production-quality code.
- Run targeted verification and report concrete command outcomes.
- Update docs when behavior or architecture changes.
- Keep unrelated files untouched.

Harness tooling defaults:
- Prefer `python3 tools/code_search.py "<pattern>" <paths...>` for fast scoped code search.
- Use `python3 tools/symphony_bootstrap.py` and `python3 tools/symphony_run.py` for orchestration setup/runtime.
- Use `python3 tools/linear_workspace.py sync-manifest|sync-index` for idempotent Linear backlog sync.
