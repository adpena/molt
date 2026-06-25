"""The perf-gate-wiring audit must FAIL on an un-wired gate (proving it is not
itself a proxy-measurement meta-bug) and PASS on the real, wired tree."""

from __future__ import annotations

import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
TOOLS = REPO / "tools"
if str(TOOLS) not in sys.path:
    sys.path.insert(0, str(TOOLS))

import check_perf_gate_wiring as w  # noqa: E402


def _write(tmp: Path, body: str) -> Path:
    p = tmp / "perf-gate.yml"
    p.write_text(body, encoding="utf-8")
    return p


def _check_against(monkeypatch, path: Path) -> list[str]:
    monkeypatch.setattr(w, "PERF_GATE", path)
    return w.check()


def test_unwired_gate_is_flagged(monkeypatch, tmp_path):
    # The EXACT meta-bug shape: workflow_dispatch + weekly cron only, no main trigger.
    body = (
        'on:\n  workflow_dispatch:\n  schedule:\n    - cron: "0 6 * * 1"\n'
        "jobs:\n  s:\n    steps:\n      - run: python3 tools/perf_scoreboard.py --classify\n"
    )
    problems = _check_against(monkeypatch, _write(tmp_path, body))
    assert problems, "an un-wired perf gate MUST be flagged"
    assert any("does not fire" in p for p in problems)


def test_wired_to_main_push_passes(monkeypatch, tmp_path):
    body = (
        "on:\n  workflow_dispatch:\n  push:\n    branches: [main]\n"
        "jobs:\n  s:\n    steps:\n      - run: python3 tools/perf_scoreboard.py --classify\n"
    )
    assert _check_against(monkeypatch, _write(tmp_path, body)) == []


def test_pull_request_trigger_passes(monkeypatch, tmp_path):
    body = (
        "on:\n  pull_request:\n"
        "jobs:\n  s:\n    steps:\n      - run: python3 tools/perf_scoreboard.py\n"
    )
    assert _check_against(monkeypatch, _write(tmp_path, body)) == []


def test_missing_scoreboard_is_flagged(monkeypatch, tmp_path):
    body = "on:\n  push:\n    branches: [main]\njobs:\n  s:\n    steps:\n      - run: echo hi\n"
    problems = _check_against(monkeypatch, _write(tmp_path, body))
    assert any("perf_scoreboard" in p for p in problems)


def test_yaml_on_keyword_gotcha_is_handled(monkeypatch, tmp_path):
    # PyYAML coerces the bare `on` key to True; the audit must not be fooled by it.
    body = (
        "on:\n  push:\n    branches: [main]\n"
        "jobs:\n  s:\n    steps:\n      - run: python3 tools/perf_scoreboard.py\n"
    )
    import yaml

    doc = yaml.safe_load(_write(tmp_path, body).read_text(encoding="utf-8"))
    # Confirm the gotcha is live (key is True, not "on") and the helper still finds triggers.
    assert (True in doc) or ("on" in doc)
    assert "push" in w._triggers(doc)


def test_live_tree_is_wired():
    # The REAL repo gate must be wired -- this is the drift-gate that keeps
    # perf-gate.yml from silently regressing to cron-only again.
    problems = w.check()
    assert problems == [], f"live perf-gate.yml is not wired to main: {problems}"
