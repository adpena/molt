# Harness Engineering Alignment (Molt + Symphony)

Last updated: 2026-03-05

This document translates OpenAI's Harness Engineering guidance into Molt's
Symphony operating model.

Primary source:
- [Harness Engineering](https://openai.com/index/harness-engineering/)

## Objective

Create an agent-first execution system that compounds engineering quality over
time while preserving Molt's non-negotiable requirements:
- deterministic verification
- Rust-first lowering and runtime ownership
- explicit human accountability for risk and acceptance decisions

## Principle-To-Control Mapping

### 1) Agent-first repository legibility

The harness requires agent-first repository legibility:
- canonical map: `docs/INDEX.md`, `docs/CANONICALS.md`
- orchestration map: `docs/SYMPHONY.md`
- human authority contract: `docs/SYMPHONY_HUMAN_ROLE.md`
- execution playbook: `docs/SYMPHONY_OPERATOR_PLAYBOOK.md`

Readiness enforcement:
- `tools/symphony_readiness_audit.py` (`docs_and_tools` + `harness_engineering`)

### 2) Executable quality gates

Quality gate enforcement is deterministic and reproducible:
- readiness + strict autonomy: `tools/symphony_readiness_audit.py`
- policy/matrix gates:
  - `docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md`
  - `docs/spec/areas/tooling/0014_DETERMINISM_SECURITY_ENFORCEMENT_CHECKLIST.md`
- formal gate lane: `tools/check_formal_methods.py`

### 3) Execution plan discipline

Execution plan artifacts are first-class harness inputs:
- template: `docs/exec-plans/TEMPLATE.md`
- active plans: `docs/exec-plans/active/`
- completed plans: `docs/exec-plans/completed/`

### 4) Observability and intervention

Symphony keeps observability and intervention surfaces always-on:
- dashboard + APIs in `src/molt/symphony/http_server.py`
- operator actions:
  - `POST /api/v1/refresh`
  - `POST /api/v1/interventions/retry-now`
  - `POST /api/v1/tools/run`
- durable telemetry:
  - `/Volumes/APDataStore/Molt/logs/symphony/durable_memory/`

### 5) Entropy cleanup loop

Harness health requires recurring doc gardening and entropy cleanup:
- Linear hygiene loop: `tools/linear_hygiene.py full-pass --apply`
- readiness loop: `tools/symphony_readiness_audit.py`
- doc sync loop: update `docs/spec/STATUS.md` + `ROADMAP.md` when behavior/scope move

### 6) Recursive continual learning loop

The recursive and continual learning loop is:
1. Seed/refresh backlog from canonical docs and TODO contracts.
2. Normalize and route issues with `tools/linear_hygiene.py`.
3. Execute with Symphony (`tools/symphony_run.py`).
4. Gather evidence (tests/perf/formal/docs updates).
5. Score and triage via readiness audit and quality score rubric.
6. Feed learnings back into docs/manifests/workflows and repeat.

## Canonical Score Target

See [docs/QUALITY_SCORE.md](docs/QUALITY_SCORE.md).
Current target:
- Harness Engineering Score (`harness_engineering.score`) >= 90
- no fail-level readiness findings

## Autonomy Boundary

Symphony maximizes execution autonomy, but the human remains accountable for:
- prioritization and scope tradeoffs
- policy/risk acceptance
- final completion and release decisions

Reference:
- `docs/SYMPHONY_HUMAN_ROLE.md`

