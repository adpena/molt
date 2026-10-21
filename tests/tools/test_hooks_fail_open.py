"""THE non-negotiable invariant: a hook must NEVER brick a session (A1).

Proves the fail-open wrapper: any exception in a hook body -> ALLOW / exit 0,
with the error logged (loud, not silent). Covers the generic wrapper and each
concrete hook's ``main`` when its core raises.
"""

from __future__ import annotations

import io
import json
import os
import shlex
import shutil
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
from tools.command_execution import CommandExecutor  # noqa: E402

_COMMANDS = CommandExecutor.for_file(__file__)


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


# --- the settings commands themselves (the launcher) ----------------------

_POSIX_SH = shutil.which("sh")
_NEEDS_POSIX_CHECKOUT = pytest.mark.skipif(
    _POSIX_SH is None or os.name == "nt",
    reason="builds a symlinked checkout and runs hook commands through a POSIX sh",
)
_HTTPS_PUSH = {
    "tool_name": "Bash",
    "tool_input": {"command": "git push https://github.com/example/repo.git main"},
}


def _settings_hook_commands() -> dict[str, list[str]]:
    data = json.loads((ROOT / ".claude" / "settings.json").read_text(encoding="utf-8"))
    return {
        event: [hook["command"] for group in groups for hook in group["hooks"]]
        for event, groups in data["hooks"].items()
    }


def _hook_checkout(tmp_path: Path, *, environment: Path | None) -> Path:
    """A project dir with this checkout's tools and its own hook state."""
    checkout = tmp_path / "checkout"
    checkout.mkdir()
    (checkout / "tools").symlink_to(ROOT / "tools", target_is_directory=True)
    if environment is not None:
        (checkout / ".venv").symlink_to(environment, target_is_directory=True)
    return checkout


def _run_settings_command(
    command: str,
    checkout: Path,
    tmp_path: Path,
    payload: dict,
    uv_project_environment: str | None = None,
):
    """Run one settings command as Claude Code does, with only `sh` on PATH.

    The desktop app's PATH has no ``python``. The test harness injects its own
    ``UV_PROJECT_ENVIRONMENT`` into every child, so the child shell states the
    app's value (normally unset) before it runs the command verbatim.
    """
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir(exist_ok=True)
    if not (bin_dir / "sh").exists():
        (bin_dir / "sh").symlink_to(_POSIX_SH)
    prelude = (
        "unset UV_PROJECT_ENVIRONMENT"
        if uv_project_environment is None
        else f"UV_PROJECT_ENVIRONMENT={shlex.quote(uv_project_environment)}; "
        "export UV_PROJECT_ENVIRONMENT"
    )
    return _COMMANDS.run(
        [_POSIX_SH, "-c", f"{prelude}\n{command}"],
        input=json.dumps({"session_id": "launcher", "cwd": str(checkout), **payload}),
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=120,
        env={
            "PATH": str(bin_dir),
            "HOME": str(tmp_path),
            "CLAUDE_PROJECT_DIR": str(checkout),
        },
    )


@_NEEDS_POSIX_CHECKOUT
def test_settings_hooks_run_without_python_on_path(tmp_path):
    """Each settings command runs its hook under the project's interpreter.

    The desktop app's PATH has no ``python``, and every hook failed with exit
    127 there. Only the guard itself can refuse an HTTPS push with exit 2, so
    the refusal proves the command ran the hook, not just a shell.
    """
    commands = _settings_hook_commands()
    assert set(commands) == {"SessionStart", "PreToolUse", "Stop"}
    assert all(len(event) == 1 for event in commands.values())
    checkout = _hook_checkout(tmp_path, environment=Path(sys.prefix))
    (guard,) = commands["PreToolUse"]
    refused = _run_settings_command(guard, checkout, tmp_path, _HTTPS_PUSH)
    assert refused.returncode == 2, refused.stderr
    assert "BLOCKED (https-push)" in refused.stderr
    allowed = _run_settings_command(
        guard,
        checkout,
        tmp_path,
        {"tool_name": "Bash", "tool_input": {"command": "git status"}},
    )
    assert (allowed.returncode, allowed.stderr) == (0, "")
    for event in ("SessionStart", "Stop"):
        (command,) = commands[event]
        completed = _run_settings_command(
            command, checkout, tmp_path, {"stop_hook_active": True}
        )
        assert (completed.returncode, completed.stderr) == (0, ""), event


@_NEEDS_POSIX_CHECKOUT
def test_launcher_follows_uv_environment_selection(tmp_path):
    """UV_PROJECT_ENVIRONMENT selects the interpreter, as it selects uv's."""
    checkout = _hook_checkout(tmp_path, environment=None)
    (guard,) = _settings_hook_commands()["PreToolUse"]
    refused = _run_settings_command(
        guard, checkout, tmp_path, _HTTPS_PUSH, uv_project_environment=sys.prefix
    )
    assert refused.returncode == 2, refused.stderr
    (checkout / "env").symlink_to(Path(sys.prefix), target_is_directory=True)
    relative = _run_settings_command(
        guard, checkout, tmp_path, _HTTPS_PUSH, uv_project_environment="env"
    )
    assert relative.returncode == 2, relative.stderr


@_NEEDS_POSIX_CHECKOUT
def test_launcher_fails_open_without_an_environment(tmp_path):
    """A hook that cannot start must not block the tool call: exit 2 would."""
    checkout = _hook_checkout(tmp_path, environment=None)
    (guard,) = _settings_hook_commands()["PreToolUse"]
    completed = _run_settings_command(guard, checkout, tmp_path, _HTTPS_PUSH)
    assert completed.returncode == 1
    assert "molt hook bash_guard did not run" in completed.stderr
    assert "uv sync" in completed.stderr
