# Orchestration Harness Quality Score

Last updated: 2026-03-05

This defines the canonical quality score rubric for Orchestration harness maturity.

## Harness Engineering Score (HES)

The readiness audit computes:
- `sections.harness_engineering.score` (0-100)

Score weights:
- 60 points: required harness artifacts present
- 40 points: principle coverage present in `docs/HARNESS_ENGINEERING.md`

Target:
- `score >= 90`

Status thresholds:
- `pass`: `>= 90` and no critical harness artifact gaps
- `warn`: `70..89`
- `fail`: `< 70` or critical harness artifact missing

## Required Artifacts

- `docs/HARNESS_ENGINEERING.md`
- `docs/QUALITY_SCORE.md`
- `docs/exec-plans/TEMPLATE.md`
- `docs/exec-plans/active/README.md`
- `docs/exec-plans/completed/README.md`

## Operating Policy

- Daily operator/default lane:
  - `PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_readiness_audit.py --team Moltlang`
- Hard autonomy gate lane:
  - `PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_readiness_audit.py --team Moltlang --strict-autonomy --fail-on warn`

If HES drops below target:
1. Repair missing artifacts.
2. Repair missing principle coverage.
3. Re-run readiness until score is back to `>= 90`.

