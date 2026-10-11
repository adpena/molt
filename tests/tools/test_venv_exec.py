from __future__ import annotations

import os
import sys
from pathlib import Path

import pytest

from tests.process_guard_common import run_guarded_test_process
from molt.process_spawn import forward_command
from tools import venv_exec

ROOT = Path(__file__).resolve().parents[2]


def test_resolve_venv_prefers_explicit_path(tmp_path: Path) -> None:
    root = tmp_path / "repo"
    root.mkdir()
    explicit = tmp_path / "venv"

    assert (
        venv_exec.resolve_venv(repo_root=root, explicit=str(explicit), env={})
        == explicit
    )


def test_resolve_venv_uses_repo_default(tmp_path: Path) -> None:
    root = tmp_path / "repo"
    root.mkdir()

    assert venv_exec.resolve_venv(repo_root=root, env={}) == root / ".venv"


def test_resolve_venv_follows_the_uv_project_environment(tmp_path: Path) -> None:
    # CI custody syncs the project into UV_PROJECT_ENVIRONMENT outside the
    # checkout; the checkout has no .venv at all.
    root = tmp_path / "repo"
    custody = tmp_path / "custody" / "venv"
    assert (
        venv_exec.resolve_venv(
            repo_root=root, env={"UV_PROJECT_ENVIRONMENT": str(custody)}
        )
        == custody.resolve()
    )
    assert (
        venv_exec.resolve_venv(
            repo_root=root,
            env={
                "UV_PROJECT_ENVIRONMENT": str(custody),
                "MOLT_VENV": str(tmp_path / "m"),
            },
        )
        == (tmp_path / "m").resolve()
    )


def test_resolve_venv_ignores_unrelated_active_virtualenv(tmp_path: Path) -> None:
    root = tmp_path / "repo"
    root.mkdir()
    unrelated = tmp_path / "other-venv"

    assert (
        venv_exec.resolve_venv(
            repo_root=root,
            env={"VIRTUAL_ENV": str(unrelated)},
        )
        == root / ".venv"
    )


def test_venv_env_marks_virtualenv_and_prepends_bin(tmp_path: Path) -> None:
    venv = tmp_path / ".venv"
    base_env = {"PATH": os.pathsep.join(["/usr/bin", "/bin"])}

    env = venv_exec.venv_env(venv=venv, env=base_env)

    assert env["VIRTUAL_ENV"] == str(venv)
    assert env["PATH"].split(os.pathsep)[0] == str(venv_exec.venv_bin_dir(venv))
    assert env["PATH"].endswith(base_env["PATH"])
    # Gates must never inherit the host console codec (Windows cp1252 class).
    assert env["PYTHONUTF8"] == "1"
    assert env["PYTHONIOENCODING"] == "utf-8"


def test_resolve_command_rewrites_python_to_venv_python(tmp_path: Path) -> None:
    venv = tmp_path / ".venv"
    bin_dir = venv_exec.venv_bin_dir(venv)
    bin_dir.mkdir(parents=True)
    python = bin_dir / ("python.exe" if os.name == "nt" else "python3")
    python.write_text("", encoding="utf-8")

    command = venv_exec.resolve_command(["python3", "-m", "pytest"], venv=venv)

    assert command == [str(python), "-m", "pytest"]


def test_resolve_command_reports_missing_venv_python(tmp_path: Path) -> None:
    with pytest.raises(venv_exec.VenvExecError, match="virtualenv Python not found"):
        venv_exec.resolve_command(["python3", "-m", "pytest"], venv=tmp_path / ".venv")


def test_resolve_command_leaves_non_python_command_intact(tmp_path: Path) -> None:
    command = ["cargo", "test"]

    assert venv_exec.resolve_command(command, venv=tmp_path / ".venv") == command


@pytest.mark.parametrize("status", [0, 7])
def test_wrapped_command_exit_status_propagates(status: int) -> None:
    # On Windows os.exec* exits the caller with 0 at once; the wrapper must
    # still report the wrapped gate's own status on every platform.
    result = run_guarded_test_process(
        [
            sys.executable,
            str(ROOT / "tools" / "venv_exec.py"),
            "python",
            "-c",
            f"raise SystemExit({status})",
        ],
        cwd=ROOT,
        timeout=120,
    )
    assert result.returncode == status, result.stderr


@pytest.mark.skipif(os.name == "nt", reason="POSIX failed exec changes cwd temporarily")
@pytest.mark.parametrize("missing_cwd", [False, True])
def test_forward_command_failed_exec_preserves_caller_state(
    tmp_path: Path, missing_cwd: bool
) -> None:
    child_cwd = tmp_path / "child"
    if not missing_cwd:
        child_cwd.mkdir()
    # Run the real failure boundary in an isolated process; no global cwd or
    # os.exec monkeypatch can interfere with the session's custody threads.
    code = """
import os
from pathlib import Path
import sys
from molt.process_spawn import forward_command
before = Path.cwd()
environment = dict(os.environ)
try:
    forward_command([str(Path(sys.argv[1]) / "missing-command")],
                    cwd=sys.argv[1], env={"FORWARDED_ONLY": "1"})
except FileNotFoundError:
    assert Path.cwd() == before
    assert dict(os.environ) == environment
else:
    raise AssertionError("missing executable or cwd was accepted")
"""
    result = run_guarded_test_process(
        [sys.executable, "-c", code, str(child_cwd)], cwd=ROOT, timeout=60
    )
    assert result.returncode == 0, result.stderr


@pytest.mark.parametrize("command", ["python --version", b"python --version"])
def test_forward_command_rejects_shell_text(command: str | bytes) -> None:
    with pytest.raises(TypeError, match="typed argv"):
        forward_command(command, env={})  # type: ignore[arg-type]


def test_forward_command_rejects_empty_argv() -> None:
    with pytest.raises(ValueError, match="command is required"):
        forward_command([], env={})
