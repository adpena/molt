"""The controlled perf authority must run real, blocking measurements."""

from __future__ import annotations

import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
TOOLS = REPO / "tools"
if str(TOOLS) not in sys.path:
    sys.path.insert(0, str(TOOLS))

import check_perf_gate_wiring as w  # noqa: E402


CONTROLLED_EVENTS = (
    "  workflow_dispatch:\n"
    "  schedule:\n"
    '    - cron: "0 6 * * 1"\n'
)


def _write(tmp: Path, body: str) -> Path:
    path = tmp / "perf-gate.yml"
    path.write_text(body, encoding="utf-8")
    return path


def _workflow(
    *,
    events: str = CONTROLLED_EVENTS,
    cancel: str | None = "false",
    job_extra: str = "",
    step: str = "      - run: python3 tools/perf_scoreboard.py --classify\n",
) -> str:
    concurrency = "concurrency:\n  group: perf-gate\n"
    if cancel is not None:
        concurrency += f"  cancel-in-progress: {cancel}\n"
    return (
        "on:\n"
        f"{events}"
        f"{concurrency}"
        "jobs:\n"
        "  scoreboard:\n"
        f"{job_extra}"
        "    steps:\n"
        f"{step}"
    )


def _check_against(monkeypatch, path: Path) -> list[str]:
    monkeypatch.setattr(w, "PERF_GATE", path)
    return w.check()


def test_controlled_scheduled_manual_measurement_passes(monkeypatch, tmp_path):
    assert _check_against(monkeypatch, _write(tmp_path, _workflow())) == []


def test_each_controlled_trigger_is_required(monkeypatch, tmp_path):
    body = _workflow(events="  workflow_dispatch:\n")
    problems = _check_against(monkeypatch, _write(tmp_path, body))
    assert any("missing controlled measurement" in problem for problem in problems)


def test_push_and_pull_request_measurement_are_forbidden(monkeypatch, tmp_path):
    for event in ("push", "pull_request", "pull_request_target"):
        body = _workflow(events=CONTROLLED_EVENTS + f"  {event}:\n")
        problems = _check_against(monkeypatch, _write(tmp_path, body))
        assert any("high-churn" in problem and event in problem for problem in problems)


def test_active_measurement_cancellation_is_forbidden(monkeypatch, tmp_path):
    for cancel in ("true", None):
        problems = _check_against(
            monkeypatch,
            _write(tmp_path, _workflow(cancel=cancel)),
        )
        assert any("cancel-in-progress: false" in problem for problem in problems)


def test_multiline_scoreboard_run_step_passes(monkeypatch, tmp_path):
    step = (
        "      - name: Run scoreboard\n"
        "        run: |\n"
        "          python3 tools/guarded_exec.py -- \\\n"
        "            uv run python3 tools/perf_scoreboard.py --classify\n"
    )
    assert _check_against(monkeypatch, _write(tmp_path, _workflow(step=step))) == []


def test_missing_scoreboard_is_flagged(monkeypatch, tmp_path):
    problems = _check_against(
        monkeypatch,
        _write(tmp_path, _workflow(step="      - run: echo hi\n")),
    )
    assert any("perf_scoreboard" in problem for problem in problems)


def test_scoreboard_comment_without_run_step_is_flagged(monkeypatch, tmp_path):
    body = _workflow(step="      - run: echo hi\n")
    body += "# tools/perf_scoreboard.py is the canonical authority\n"
    problems = _check_against(monkeypatch, _write(tmp_path, body))
    assert any("no executable run step" in problem for problem in problems)


def test_scoreboard_step_continue_on_error_is_flagged(monkeypatch, tmp_path):
    step = (
        "      - run: python3 tools/perf_scoreboard.py\n"
        "        continue-on-error: true\n"
    )
    problems = _check_against(monkeypatch, _write(tmp_path, _workflow(step=step)))
    assert any("continue-on-error" in problem for problem in problems)


def test_scoreboard_job_continue_on_error_is_flagged(monkeypatch, tmp_path):
    problems = _check_against(
        monkeypatch,
        _write(
            tmp_path,
            _workflow(job_extra="    continue-on-error: ${{ matrix.experimental }}\n"),
        ),
    )
    assert any("job 'scoreboard'" in problem for problem in problems)


def test_scoreboard_false_if_is_flagged(monkeypatch, tmp_path):
    step = (
        "      - if: ${{ false }}\n"
        "        run: python3 tools/perf_scoreboard.py\n"
    )
    problems = _check_against(monkeypatch, _write(tmp_path, _workflow(step=step)))
    assert any("trivially false" in problem for problem in problems)


def test_scoreboard_job_gated_away_from_measurement_is_flagged(
    monkeypatch, tmp_path
):
    body = _workflow(job_extra="    if: github.event_name == 'push'\n")
    problems = _check_against(monkeypatch, _write(tmp_path, body))
    assert any("gated away" in problem for problem in problems)


def test_yaml_on_keyword_gotcha_is_handled(monkeypatch, tmp_path):
    body = _workflow()
    yaml_shape = {
        True: {"workflow_dispatch": {}, "schedule": [{"cron": "0 6 * * 1"}]}
    }
    assert set(w._triggers(yaml_shape)) == w.MEASUREMENT_EVENTS
    assert set(w._triggers(w._load_yaml(_write(tmp_path, body)))) == (
        w.MEASUREMENT_EVENTS
    )


def test_live_tree_has_controlled_measurement_authority():
    problems = w.check()
    assert problems == [], f"live perf authority drifted: {problems}"
