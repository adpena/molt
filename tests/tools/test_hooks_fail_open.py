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
    monkeypatch.setattr(
        sg, "LEGS", (sg.StopLeg("boomer", lambda data, root: _raise()),)
    )
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


def test_queue_count_tells_an_idle_queue_from_an_unreadable_one(tmp_path, monkeypatch):
    from tools.proof_queue_pkg import state

    db = tmp_path / "proof_queue.sqlite3"
    monkeypatch.setattr(state, "default_db_path", lambda root, environ: db)
    # No queue has run here: nothing is active.
    assert common.proof_queue_active_count(tmp_path) == 0
    # Unreadable state is unknown, never "idle": the guard and the landing
    # gate choose their own fail-open direction for None.
    db.write_bytes(b"not a database")
    assert common.proof_queue_active_count(tmp_path) is None


@pytest.mark.parametrize("state", ["warn", "strict"])
def test_a_stop_leg_blocks_only_under_strict_and_records_under_warn(
    tmp_path, monkeypatch, capsys, state
):
    monkeypatch.setattr(sg._common, "repo_root", lambda cwd=None: tmp_path)
    monkeypatch.setattr(
        sg._common,
        "read_hook_input",
        lambda: {"cwd": str(tmp_path), "session_id": "s7"},
    )
    leg = sg.StopLeg("probe_leg", lambda data, root: "drifted")
    monkeypatch.setattr(sg, "LEGS", (leg,))
    monkeypatch.setattr(sg, "flip_state", lambda name: {"probe_leg": state}[name])
    assert sg.run() == 0
    out = capsys.readouterr().out
    shadow = common.shadow_records(tmp_path, sg.SHADOW_NAME)
    if state == "strict":
        assert json.loads(out) == {"decision": "block", "reason": "[probe_leg] drifted"}
        assert shadow == []
    else:
        assert out == ""
        assert [(r["leg"], r["reason"], r["session_id"]) for r in shadow] == [
            ("probe_leg", "drifted", "s7")
        ]


def test_a_stop_leg_without_a_flip_never_reengages(tmp_path, monkeypatch, capsys):
    monkeypatch.setattr(sg._common, "repo_root", lambda cwd=None: tmp_path)
    monkeypatch.setattr(sg._common, "read_hook_input", lambda: {"cwd": str(tmp_path)})
    monkeypatch.setattr(sg, "LEGS", (sg.StopLeg("unflipped", lambda d, r: "x"),))
    monkeypatch.setattr(sg, "flip_state", lambda name: None)
    assert sg.run() == 0
    assert capsys.readouterr().out == ""


def test_shadow_records_rotate_past_their_bound(tmp_path, monkeypatch):
    monkeypatch.setattr(common, "SHADOW_MAX_BYTES", 200)
    for index in range(20):
        common.record_shadow(tmp_path, "probe.jsonl", {"n": index})
    records = common.shadow_records(tmp_path, "probe.jsonl")
    numbers = [record["n"] for record in records]
    # The newest records survive in order; older generations retire.
    assert numbers == sorted(numbers) and numbers[-1] == 19 and len(numbers) < 20
    assert all("recorded_at" in record for record in records)
    current = common.state_dir(tmp_path) / "probe.jsonl"
    assert current.stat().st_size <= 200 + 64


# --- the settings commands themselves (the launcher) ----------------------

# The hook path runs exactly these utilities: the shell, Git, `cat` for the
# bash_guard prefilter and `dirname` in .githooks/molt-hook-launch.sh.
_HOOK_UTILITIES = {name: shutil.which(name) for name in ("sh", "git", "cat", "dirname")}
_NEEDS_POSIX_CHECKOUT = pytest.mark.skipif(
    None in _HOOK_UTILITIES.values() or os.name == "nt",
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


def _shadow_rules(project: Path) -> list[str]:
    shadow = project / ".molt" / "state" / bg.SHADOW_NAME
    if not shadow.exists():
        return []
    lines = shadow.read_text(encoding="utf-8").splitlines()
    return [json.loads(line)["rule"] for line in lines]


@_NEEDS_POSIX_CHECKOUT
def test_settings_hooks_run_without_python_on_path(tmp_path, checkout_run_context):
    """Each settings command runs its hook under the project's interpreter.

    The desktop app's PATH has no ``python``, and every hook failed with exit
    127 there. bash_guard observes, so its shadow record proves the command
    ran the guard itself, not just a shell; the digest header proves the same
    for SessionStart.
    """
    commands = _settings_hook_commands()
    assert set(commands) == {"SessionStart", "PreToolUse", "Stop"}
    assert all(len(event) == 1 for event in commands.values())
    checkout = _hook_checkout(tmp_path, environment=Path(sys.prefix))
    (guard,) = commands["PreToolUse"]
    observed = _run_settings_command(
        guard, checkout, tmp_path, _HTTPS_PUSH, run_context=checkout_run_context
    )
    assert (observed.returncode, observed.stderr) == (0, "")
    assert _shadow_rules(checkout) == ["https-push"]
    allowed = _run_settings_command(
        guard,
        checkout,
        tmp_path,
        {"tool_name": "Bash", "tool_input": {"command": "git status"}},
        run_context=checkout_run_context,
    )
    assert (allowed.returncode, allowed.stderr) == (0, "")
    assert _shadow_rules(checkout) == ["https-push"]
    (digest,) = commands["SessionStart"]
    started = _run_settings_command(
        digest, checkout, tmp_path, {}, run_context=checkout_run_context
    )
    assert (started.returncode, started.stderr) == (0, "")
    assert started.stdout.startswith("== molt session start ==\n")
    (stop,) = commands["Stop"]
    stopped = _run_settings_command(
        stop,
        checkout,
        tmp_path,
        {"stop_hook_active": True},
        run_context=checkout_run_context,
    )
    assert (stopped.returncode, stopped.stdout, stopped.stderr) == (0, "", "")


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
    (guard,) = _settings_hook_commands()["PreToolUse"]
    decoy = tmp_path / "no-interpreter"
    decoy.mkdir()
    observed = _run_settings_command(
        guard,
        linked,
        tmp_path,
        _HTTPS_PUSH,
        prelude=f"UV_PROJECT_ENVIRONMENT={shlex.quote(str(decoy))}; "
        "export UV_PROJECT_ENVIRONMENT",
        run_context=checkout_run_context,
    )
    assert (observed.returncode, observed.stderr) == (0, "")
    assert _shadow_rules(linked) == ["https-push"]


@_NEEDS_POSIX_CHECKOUT
def test_launcher_fails_open_without_an_environment(tmp_path, checkout_run_context):
    """A hook that cannot start must not block the tool call: exit 2 would."""
    checkout = _hook_checkout(tmp_path, environment=None)
    (guard,) = _settings_hook_commands()["PreToolUse"]
    completed = _run_settings_command(
        guard, checkout, tmp_path, _HTTPS_PUSH, run_context=checkout_run_context
    )
    assert completed.returncode == 1
    assert "molt hook bash_guard did not run" in completed.stderr
    assert "uv sync" in completed.stderr


@_NEEDS_POSIX_CHECKOUT
def test_launcher_fails_open_outside_a_git_checkout(tmp_path, checkout_run_context):
    project = tmp_path / "plain"
    project.mkdir()
    (project / "tools").symlink_to(ROOT / "tools", target_is_directory=True)
    (project / ".githooks").symlink_to(ROOT / ".githooks", target_is_directory=True)
    (guard,) = _settings_hook_commands()["PreToolUse"]
    completed = _run_settings_command(
        guard, project, tmp_path, _HTTPS_PUSH, run_context=checkout_run_context
    )
    assert completed.returncode == 1
    assert "is not a Git checkout" in completed.stderr


@_NEEDS_POSIX_CHECKOUT
@pytest.mark.parametrize("event", ["SessionStart", "PreToolUse", "Stop"])
def test_settings_commands_fail_open_without_the_launcher(
    tmp_path, event, checkout_run_context
):
    # A project dir without tools/hooks/run.sh: `sh` on a missing file is 127,
    # and a PreToolUse hook must never exit 2.
    project = tmp_path / "bare"
    project.mkdir()
    (command,) = _settings_hook_commands()[event]
    completed = _run_settings_command(
        command, project, tmp_path, _HTTPS_PUSH, run_context=checkout_run_context
    )
    assert completed.returncode == 1


@_NEEDS_POSIX_CHECKOUT
def test_launcher_refuses_an_unknown_hook(tmp_path, checkout_run_context):
    checkout = _hook_checkout(tmp_path, environment=Path(sys.prefix))
    command = 'sh "${CLAUDE_PROJECT_DIR}/tools/hooks/run.sh" no_such_hook'
    completed = _run_settings_command(
        command, checkout, tmp_path, _HTTPS_PUSH, run_context=checkout_run_context
    )
    assert completed.returncode == 1
    assert "no tools/hooks/no_such_hook.py" in completed.stderr


@_NEEDS_POSIX_CHECKOUT
def test_launcher_starts_python_exactly_for_a_prefilter_token(
    tmp_path, checkout_run_context
):
    """run.sh's generated test and PREFILTER_TOKENS agree, in any case.

    The checkout's interpreter records each start, so the oracle is whether
    Python ran, not what the guard decided.
    """
    environment = tmp_path / "recording-env"
    (environment / "bin").mkdir(parents=True)
    starts = tmp_path / "starts"
    python = environment / "bin" / "python"
    python.write_text(
        f"#!/bin/sh\necho started >> {shlex.quote(str(starts))}\n"
        f'exec {shlex.quote(sys.executable)} "$@"\n',
        encoding="utf-8",
    )
    python.chmod(0o755)
    checkout = _hook_checkout(tmp_path, environment=environment)
    (guard,) = _settings_hook_commands()["PreToolUse"]

    def started_for(command: str) -> bool:
        before = starts.read_text().count("started") if starts.exists() else 0
        completed = _run_settings_command(
            guard,
            checkout,
            tmp_path,
            {"tool_name": "Bash", "tool_input": {"command": command}},
            run_context=checkout_run_context,
        )
        assert completed.returncode == 0, completed.stderr
        after = starts.read_text().count("started") if starts.exists() else 0
        return after > before

    for token in bg.PREFILTER_TOKENS:
        for spelling in (token, token.upper(), token.capitalize()):
            assert started_for(f"echo {spelling}"), spelling
    assert not started_for("ls -la docs")
