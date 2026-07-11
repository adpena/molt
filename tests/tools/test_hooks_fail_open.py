"""THE non-negotiable invariant: a hook must NEVER brick a session (A1).

Proves the fail-open wrapper: any exception in a hook body -> ALLOW / exit 0,
with the error logged (loud, not silent). Covers the generic wrapper and each
concrete hook's ``main`` when its core raises.
"""

from __future__ import annotations

import io
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import tools.hooks._common as common  # noqa: E402
import tools.hooks.bash_guard as bg  # noqa: E402
import tools.hooks.session_digest as sd  # noqa: E402
import tools.hooks.stop_gates as sg  # noqa: E402


def _raise():
    raise RuntimeError("boom -- simulated hook core failure")


def test_run_fail_open_swallows_exception_and_exits_zero(tmp_path, monkeypatch):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))
    with pytest.raises(SystemExit) as ei:
        common.run_fail_open("unittest_hook", _raise)
    assert ei.value.code == 0
    # And the error was logged (fail-open is never silent).
    log = tmp_path / ".molt" / "state" / "unittest_hook_errors.log"
    assert log.exists() and "boom" in log.read_text(encoding="utf-8")


def test_run_fail_open_passes_through_normal_exit_code():
    with pytest.raises(SystemExit) as ei:
        common.run_fail_open("unittest_hook", lambda: 2)
    assert ei.value.code == 2


def test_run_fail_open_deliberate_sys_exit_preserved():
    def body():
        raise SystemExit(2)

    with pytest.raises(SystemExit) as ei:
        common.run_fail_open("unittest_hook", body)
    assert ei.value.code == 2


def test_bash_guard_main_fail_open_when_decide_raises(monkeypatch, tmp_path):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))
    monkeypatch.setattr(bg, "decide", lambda *a, **k: _raise())
    monkeypatch.setattr(bg._common, "is_linked_worktree", lambda root: False)
    monkeypatch.setattr(
        sys,
        "stdin",
        io.StringIO(
            json.dumps(
                {"tool_name": "Bash", "tool_input": {"command": "git reset --hard"}}
            )
        ),
    )
    with pytest.raises(SystemExit) as ei:
        bg.main()
    assert ei.value.code == 0  # a crashing guard ALLOWS -- never wedges


def test_stop_gates_main_fail_open_when_input_raises(monkeypatch, tmp_path):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))
    monkeypatch.setattr(sg._common, "read_hook_input", _raise)
    with pytest.raises(SystemExit) as ei:
        sg.main()
    assert ei.value.code == 0


def test_stop_gates_one_leg_raising_does_not_sink_others(monkeypatch, tmp_path):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))
    monkeypatch.setattr(
        sys,
        "stdin",
        io.StringIO(json.dumps({"session_id": "s1", "cwd": str(tmp_path)})),
    )
    # A leg that raises must be caught (logged) and NOT block the stop.
    monkeypatch.setattr(sg, "GATES", [("boomer", lambda data, root: _raise())])
    assert sg.run() == 0


def test_session_digest_main_fail_open(monkeypatch, tmp_path):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))
    monkeypatch.setattr(sd._common, "read_hook_input", _raise)
    with pytest.raises(SystemExit) as ei:
        sd.main()
    assert ei.value.code == 0


def test_error_log_loud_escalation_after_threshold(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("CLAUDE_PROJECT_DIR", str(tmp_path))
    for _ in range(common.ERROR_ESCALATION_THRESHOLD):
        common.log_error("noisy_hook", RuntimeError("x"), tmp_path)
    err = capsys.readouterr().err
    assert "ALERT" in err  # escalates loudly once the threshold is crossed
