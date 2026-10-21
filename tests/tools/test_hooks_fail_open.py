"""THE non-negotiable invariant: a hook must NEVER brick a session (A1).

Proves the fail-open wrapper: any exception in a hook body -> ALLOW / exit 0,
with the error logged (loud, not silent). Covers the generic wrapper and each
concrete hook's ``main`` when its core raises.
"""

from __future__ import annotations

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
import tools.hooks.session_digest as sd  # noqa: E402
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


def test_queue_count_tells_an_idle_queue_from_an_unreadable_one(tmp_path, monkeypatch):
    from tools.proof_queue_pkg import state

    db = tmp_path / "proof_queue.sqlite3"
    monkeypatch.setattr(state, "default_db_path", lambda root, environ: db)
    # No queue has run here: nothing is active.
    assert common.proof_queue_active_count(tmp_path) == 0
    # Unreadable state is unknown, never "idle": the digest reports it as
    # undeterminable.
    db.write_bytes(b"not a database")
    assert common.proof_queue_active_count(tmp_path) is None


# --- the settings commands themselves (the launcher) ----------------------

# The hook path runs exactly these utilities: the shell, Git and `dirname` in
# .githooks/molt-hook-launch.sh.
_HOOK_UTILITIES = {name: shutil.which(name) for name in ("sh", "git", "dirname")}
_NEEDS_POSIX_CHECKOUT = pytest.mark.skipif(
    None in _HOOK_UTILITIES.values() or os.name == "nt",
    reason="builds a symlinked checkout and runs hook commands through a POSIX sh",
)


def _settings_hook_commands() -> dict[str, list[str]]:
    data = json.loads((ROOT / ".claude" / "settings.json").read_text(encoding="utf-8"))
    return {
        event: [hook["command"] for group in groups for hook in group["hooks"]]
        for event, groups in data["hooks"].items()
    }


def _git(*args: str, cwd: Path) -> None:
    completed = _COMMANDS.run(
        [
            _HOOK_UTILITIES["git"],
            "-c",
            "user.name=hook-test",
            "-c",
            "user.email=hook-test@example.invalid",
            *args,
        ],
        cwd=str(cwd),
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    assert completed.returncode == 0, completed.stderr


def _hook_checkout(tmp_path: Path, *, environment: Path | None) -> Path:
    """A committed Git checkout that projects this checkout's hook sources.

    The projection is committed, so a linked worktree of it has the same
    sources and no environment of its own.
    """
    checkout = tmp_path / "checkout"
    checkout.mkdir()
    _git("init", "-q", cwd=checkout)
    for name in ("tools", "src", ".githooks"):
        (checkout / name).symlink_to(ROOT / name, target_is_directory=True)
    _git("add", "--", "tools", "src", ".githooks", cwd=checkout)
    _git("commit", "-q", "-m", "hook sources", cwd=checkout)
    if environment is not None:
        (checkout / ".venv").symlink_to(environment, target_is_directory=True)
    return checkout


def _run_settings_command(
    command: str,
    project: Path,
    tmp_path: Path,
    payload: dict,
    *,
    run_context: dict[str, str],
    prelude: str = "",
):
    """Run one settings command as Claude Code does, on a PATH without Python.

    The desktop app's PATH has no ``python``; this PATH holds only the
    utilities the hook path runs. The hooks run the checkout's own tools, so
    the child gets the session's run context (``checkout_run_context``):
    without it those tools keep their state in the checkout's ``target/``.
    """
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir(exist_ok=True)
    for name, tool in _HOOK_UTILITIES.items():
        if not (bin_dir / name).exists():
            (bin_dir / name).symlink_to(tool)
    return _COMMANDS.run(
        [_HOOK_UTILITIES["sh"], "-c", f"{prelude}\n{command}"],
        input=json.dumps({"session_id": "launcher", "cwd": str(project), **payload}),
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=120,
        env={
            **run_context,
            "PATH": str(bin_dir),
            "HOME": str(tmp_path),
            "CLAUDE_PROJECT_DIR": str(project),
            "GIT_CEILING_DIRECTORIES": str(tmp_path),
        },
    )


_SESSION_START = {"source": "startup"}
_DIGEST_HEADER = "== molt session start ==\n"


@_NEEDS_POSIX_CHECKOUT
def test_settings_hooks_run_without_python_on_path(tmp_path, checkout_run_context):
    """The settings command runs its hook under the project's interpreter.

    The desktop app's PATH has no ``python``, and every hook failed with exit
    127 there. The digest's header proves the hook itself ran, not a shell.
    """
    commands = _settings_hook_commands()
    assert set(commands) == {"SessionStart"}
    (digest,) = commands["SessionStart"]
    checkout = _hook_checkout(tmp_path, environment=Path(sys.prefix))
    started = _run_settings_command(
        digest, checkout, tmp_path, _SESSION_START, run_context=checkout_run_context
    )
    assert (started.returncode, started.stderr) == (0, "")
    assert started.stdout.startswith(_DIGEST_HEADER)


@_NEEDS_POSIX_CHECKOUT
def test_linked_worktree_hooks_run_under_the_common_checkout_environment(
    tmp_path, checkout_run_context
):
    """A linked worktree has no .venv: the hook uses the common checkout's.

    ``UV_PROJECT_ENVIRONMENT`` names a directory with no interpreter, so a
    launcher that followed it, or the worktree's own .venv, would fail.
    """
    checkout = _hook_checkout(tmp_path, environment=Path(sys.prefix))
    linked = tmp_path / "linked"
    _git("worktree", "add", "-q", str(linked), cwd=checkout)
    assert not (linked / ".venv").exists()
    (digest,) = _settings_hook_commands()["SessionStart"]
    decoy = tmp_path / "no-interpreter"
    decoy.mkdir()
    started = _run_settings_command(
        digest,
        linked,
        tmp_path,
        _SESSION_START,
        prelude=f"UV_PROJECT_ENVIRONMENT={shlex.quote(str(decoy))}; "
        "export UV_PROJECT_ENVIRONMENT",
        run_context=checkout_run_context,
    )
    assert (started.returncode, started.stderr) == (0, "")
    assert started.stdout.startswith(_DIGEST_HEADER)


@_NEEDS_POSIX_CHECKOUT
def test_launcher_fails_open_without_an_environment(tmp_path, checkout_run_context):
    """A hook that cannot start exits 1, a visible and non-blocking notice."""
    checkout = _hook_checkout(tmp_path, environment=None)
    (digest,) = _settings_hook_commands()["SessionStart"]
    completed = _run_settings_command(
        digest, checkout, tmp_path, _SESSION_START, run_context=checkout_run_context
    )
    assert completed.returncode == 1
    assert "molt hook session_digest did not run" in completed.stderr
    assert "uv sync" in completed.stderr


@_NEEDS_POSIX_CHECKOUT
def test_launcher_fails_open_outside_a_git_checkout(tmp_path, checkout_run_context):
    project = tmp_path / "plain"
    project.mkdir()
    (project / "tools").symlink_to(ROOT / "tools", target_is_directory=True)
    (project / ".githooks").symlink_to(ROOT / ".githooks", target_is_directory=True)
    (digest,) = _settings_hook_commands()["SessionStart"]
    completed = _run_settings_command(
        digest, project, tmp_path, _SESSION_START, run_context=checkout_run_context
    )
    assert completed.returncode == 1
    assert "is not a Git checkout" in completed.stderr


@_NEEDS_POSIX_CHECKOUT
def test_settings_command_fails_open_without_the_launcher(
    tmp_path, checkout_run_context
):
    # A project dir without tools/hooks/run.sh: `sh` on a missing file is 127.
    project = tmp_path / "bare"
    project.mkdir()
    (command,) = _settings_hook_commands()["SessionStart"]
    completed = _run_settings_command(
        command, project, tmp_path, _SESSION_START, run_context=checkout_run_context
    )
    assert completed.returncode == 1
    assert f"no tools/hooks/run.sh in {project}" in completed.stderr


@_NEEDS_POSIX_CHECKOUT
def test_launcher_names_a_missing_launch_script(tmp_path, checkout_run_context):
    project = tmp_path / "no-launch-script"
    project.mkdir()
    (project / "tools").symlink_to(ROOT / "tools", target_is_directory=True)
    (digest,) = _settings_hook_commands()["SessionStart"]
    completed = _run_settings_command(
        digest, project, tmp_path, _SESSION_START, run_context=checkout_run_context
    )
    assert completed.returncode == 1
    assert "no .githooks/molt-hook-launch.sh in" in completed.stderr


@_NEEDS_POSIX_CHECKOUT
def test_launcher_refuses_an_unknown_hook(tmp_path, checkout_run_context):
    checkout = _hook_checkout(tmp_path, environment=Path(sys.prefix))
    command = 'sh "${CLAUDE_PROJECT_DIR}/tools/hooks/run.sh" no_such_hook'
    completed = _run_settings_command(
        command, checkout, tmp_path, _SESSION_START, run_context=checkout_run_context
    )
    assert completed.returncode == 1
    assert "no tools/hooks/no_such_hook.py" in completed.stderr
