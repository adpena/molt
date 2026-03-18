# Harness Engineering Alignment

Last updated: 2026-03-05

This document translates OpenAI's Harness Engineering guidance into Molt's
current repository operating model.

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
- developer guide: `docs/DEVELOPER_GUIDE.md`
- contribution workflow: `CONTRIBUTING.md`
- active plan registry: `docs/exec-plans/`

Readiness enforcement:
- `tools/linear_hygiene.py docs-audit`

### 2) Executable quality gates

Quality gate enforcement is deterministic and reproducible:
- doc + issue hygiene: `tools/linear_hygiene.py docs-audit`
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

Observability and intervention remain explicit and reviewable:
- issue/state synchronization: `tools/linear_hygiene.py`
- formal verification status: `tools/check_formal_methods.py`
- benchmark and throughput artifacts under canonical artifact roots
- execution evidence captured in docs, benchmark JSON, and CI outputs

### 5) Entropy cleanup loop

Harness health requires recurring doc gardening and entropy cleanup:
- Linear hygiene loop: `uv run --group dev --python 3.12 python3 tools/linear_hygiene.py full-pass --apply`
- doc sync loop: update `docs/spec/STATUS.md` + `ROADMAP.md` when behavior/scope move

### 6) Recursive continual learning loop

The recursive and continual learning loop is:
1. Seed/refresh backlog from canonical docs and TODO contracts.
2. Normalize and route issues with `tools/linear_hygiene.py`.
3. Execute changes through normal repo workflows and execution plans.
4. Gather evidence (tests/perf/formal/docs updates).
5. Score and triage via this quality score rubric plus deterministic evidence.
6. Bundle deterministic artifacts in canonical locations for review.
7. Feed learnings back into docs, manifests, and workflows and repeat.

Readiness keeps this loop measurable:
- execution plans track tranche state and blockers
- quality gates track deterministic proof, test, and performance evidence
- Linear hygiene sync keeps the external backlog aligned with canonical docs

## Canonical Score Target

See [docs/QUALITY_SCORE.md](docs/QUALITY_SCORE.md).
Current target:
- Harness Engineering Score (`harness_engineering.score`) >= 90
- no fail-level readiness findings

## Autonomy Boundary

Automation can maximize execution autonomy, but the human remains accountable for:
- prioritization and scope tradeoffs
- policy/risk acceptance
- final completion and release decisions
