# Symphony Autonomous-Loop Research (OpenClaw Family)

Last updated: 2026-03-05

This memo captures loop-closure patterns from the OpenClaw ecosystem and maps
them onto Molt Symphony controls.

## Scope

Primary sources reviewed (latest available on 2026-03-05):

- `openclaw/openclaw`
- `qwibitai/nanoclaw`
- `microclaw/microclaw`
- `nearai/ironclaw`

## Extracted Loop Patterns

### 1) Single authoritative agent loop + deterministic queueing

Observed:
- OpenClaw documents a serialized per-session loop and explicit `agent.wait`
  completion semantics.
- NanoClaw uses per-group queueing and explicit cursor rollback on failure.
- IronClaw splits loop orchestration from scheduler/routine execution lanes.

Molt mapping:
- Keep one canonical orchestration lane and avoid duplicate dispatch paths.
- Preserve explicit retry/suspension semantics in orchestrator state snapshots.
- Gate loop completion on deterministic end-state evidence, not heuristics.

### 2) Split periodic automation into heartbeat vs precise schedule lanes

Observed:
- OpenClaw explicitly separates heartbeat (context-aware periodic checks) from
  cron (exact-time, isolated jobs).
- MicroClaw scheduler aligns to minute boundaries and has task run logs + DLQ.

Molt mapping:
- Use readiness/hygiene as periodic heartbeat-like checks.
- Keep strict/autonomy and formal suites as exact scheduled gates.
- Persist run artifacts and failures as first-class loop outputs.

### 3) Doctor/health as first-class drift repair

Observed:
- OpenClaw and MicroClaw both expose `doctor` diagnostics with concrete repair
  output and non-interactive lanes.
- IronClaw also ships active dependency/config doctor checks.

Molt mapping:
- Treat readiness + linear hygiene + trend checks as doctor-equivalent gates.
- Keep machine-readable output and deterministic exit codes for automation.

### 4) Isolation boundaries before policy boundaries

Observed:
- NanoClaw emphasizes container isolation and mount allowlisting.
- IronClaw emphasizes sandboxed tools, capability allowlists, and leak checks.

Molt mapping:
- Keep external-volume artifact isolation and bounded event/memory queues.
- Keep capability-gated tool execution and explicit missing-capability failure.

### 5) Hook-based policy injection with bounded contracts

Observed:
- MicroClaw formalizes hook events (`BeforeLLMCall`, `BeforeToolCall`,
  `AfterToolCall`) with allow/block/modify contract.
- OpenClaw uses lifecycle hook points around prompt/tool boundaries.

Molt mapping:
- Keep deterministic intervention surfaces in dashboard/tools API.
- Prefer small, typed patch points over free-form runtime mutation.

### 6) Trace/eval fixtures as regression memory

Observed:
- IronClaw invests heavily in recorded trace fixtures and deterministic replay.
- MicroClaw exposes SLO-oriented metrics summaries with retained history.

Molt mapping:
- Keep readiness history and trend deltas as the default loop memory substrate.
- Expand deterministic replay/fixture coverage for orchestration-risk changes.

## Concrete Adoption In Molt

Implemented in this tranche:
- Added `tools/symphony_recursive_loop.py` to run deterministic cycle bundles:
  readiness audit, linear hygiene, harness trend, optional perf-guard, optional
  next-tranche command execution.
- Added test coverage in `tests/test_symphony_recursive_loop_tool.py`.
- Added operator/runbook wiring in `docs/SYMPHONY.md` and
  `docs/SYMPHONY_OPERATOR_PLAYBOOK.md`.
- Added typed loop-hook contract support (`src/molt/symphony/loop_hooks.py`) so
  external policy/taste/toolmaking logic can intervene at bounded events.
- Added DLQ persistence and replay (`src/molt/symphony/dlq.py`,
  `tools/symphony_dlq.py`) for failed recursive-loop actions.
- Added taste-memory persistence + deterministic distillation
  (`src/molt/symphony/taste_memory.py`, `tools/symphony_taste_memory.py`) so
  recurring failures, preferred tools, and successful remediations compound.

## Adoption Boundaries

- Preserve Molt governance constraints:
  - human authority remains the final acceptance gate.
  - deterministic verification remains mandatory.
- Do not copy ecosystem defaults that conflict with Molt policy (for example
  unconstrained dynamic execution or weak capability gates).
