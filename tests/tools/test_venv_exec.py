from __future__ import annotations

import os
from pathlib import Path

import pytest

from tools import venv_exec


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
