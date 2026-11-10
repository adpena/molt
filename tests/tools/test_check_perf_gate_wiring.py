"""The controlled candidate-cost workflow must run real, blocking measurements."""

from __future__ import annotations

from pathlib import Path

import pytest

from tools import check_perf_gate_wiring as w


def _workflow(
    *,
    events: str = "  workflow_dispatch:\n",
    cancel: str | None = "false",
    job: str = "",
    step: str = (
        "      - run: python3 tools/proof_plan.py --run-family "
        "runtime_candidate_costs --receipt result.json\n"
    ),
    extra_jobs: str = "",
) -> str:
    concurrency = "concurrency:\n  group: perf-gate\n"
    if cancel is not None:
        concurrency += f"  cancel-in-progress: {cancel}\n"
    return (
        f"on:\n{events}{concurrency}jobs:\n  candidate-runtime-costs:\n"
        f"{job}    steps:\n{step}{extra_jobs}"
    )


def _check(tmp_path: Path, body: str) -> list[str]:
    path = tmp_path / "perf-gate.yml"
    path.write_text(body, encoding="utf-8")
    return w.check(path)


def test_live_tree_has_controlled_candidate_authority() -> None:
    assert w.check() == [], "live perf-gate.yml drifted"


def test_explicit_blocking_candidate_dispatch_passes(tmp_path: Path) -> None:
    assert _check(tmp_path, _workflow()) == []
    # The dependency-free parser reaches the same verdict as PyYAML.
    assert w.candidate_problems(w._load_workflow_yaml_subset(_workflow())) == []


@pytest.mark.parametrize("event", ["push", "pull_request", "schedule"])
def test_only_an_explicit_dispatch_allocates_a_measurement(
    tmp_path: Path, event: str
) -> None:
    problems = _check(tmp_path, _workflow(events=f"  workflow_dispatch:\n  {event}:\n"))
    assert any("admits only workflow_dispatch" in p for p in problems)


@pytest.mark.parametrize("cancel", ["true", None])
def test_active_measurement_cancellation_is_forbidden(
    tmp_path: Path, cancel: str | None
) -> None:
    problems = _check(tmp_path, _workflow(cancel=cancel))
    assert any("cancel-in-progress: false" in p for p in problems)


@pytest.mark.parametrize(
    "change,expected",
    [
        ({"job": "    continue-on-error: true\n"}, "remain blocking"),
        (
            {"job": "    if: github.event_name == 'schedule'\n"},
            "never under a condition",
        ),
        ({"step": "      - run: python3 tools/proof_plan.py --tier pr\n"}, "one fixed"),
        (
            {
                "step": "      - run: python3 tools/proof_plan.py --run-family "
                "runtime_candidate_costs --receipt r.json || true\n"
            },
            "propagate failure",
        ),
        (
            {"extra_jobs": "  scoreboard:\n    steps:\n      - run: true\n"},
            "runs only",
        ),
    ],
)
def test_candidate_drift_is_refused(
    tmp_path: Path, change: dict[str, str], expected: str
) -> None:
    assert any(expected in p for p in _check(tmp_path, _workflow(**change)))


def test_yaml_on_keyword_gotcha_is_handled() -> None:
    # YAML 1.1 reads the bare key `on` as True; the audit reads both spellings.
    assert set(w._triggers({True: {"workflow_dispatch": {}}})) == w.ADMITTED_EVENTS
