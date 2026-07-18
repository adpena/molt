from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

from molt import python_interpreter
from tests import molt_diff


def test_target_python_candidates_are_cross_platform_without_ambient_default():
    target = python_interpreter.parse_target_python_version("3.14")

    candidates = python_interpreter.target_python_command_candidates(target)

    assert [sys.executable] not in candidates
    assert ["python3.14"] in candidates
    assert ["python314"] in candidates
    if sys.platform.startswith("win"):
        assert ["py", "-3.14"] in candidates


def test_target_python_override_preserves_multi_token_command():
    target = python_interpreter.parse_target_python_version("3.13")

    assert python_interpreter.target_python_command_candidates(
        target,
        override="py -3.13",
    ) == [["py", "-3.13"]]


def test_selector_resolves_version_through_verified_command_authority(monkeypatch):
    calls: list[str] = []

    def fake_resolve(target, **kwargs):
        calls.append(target.short)
        assert kwargs["prefer_current"] is True
        return ("py", f"-{target.short}")

    monkeypatch.setattr(
        python_interpreter, "resolve_target_python_command", fake_resolve
    )

    assert python_interpreter.resolve_python_selector("3.14") == ("py", "-3.14")
    assert calls == ["3.14"]


def test_selector_preserves_explicit_path_and_command(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    executable = tmp_path / "python custom.exe"
    executable.write_text("", encoding="utf-8")

    assert python_interpreter.resolve_python_selector(str(executable)) == (
        str(executable),
    )
    assert python_interpreter.resolve_python_selector("py -3.13") == (
        "py",
        "-3.13",
    )
    assert python_interpreter.resolve_python_selector(f'"{executable}" -X dev') == (
        str(executable),
        "-X",
        "dev",
    )
    monkeypatch.chdir(tmp_path)
    relative = ".\\python custom.exe" if sys.platform.startswith("win") else "./python custom.exe"
    assert python_interpreter.resolve_python_selector(f'"{relative}"') == (relative,)


def test_selector_refuses_missing_explicit_path(tmp_path: Path):
    missing = tmp_path / "missing" / "python.exe"
    with pytest.raises(
        python_interpreter.PythonInterpreterError,
        match="Python interpreter not found",
    ):
        python_interpreter.resolve_python_selector(str(missing))


def test_target_python_resolution_fails_closed_with_attempts(monkeypatch):
    target = python_interpreter.parse_target_python_version("3.13")
    monkeypatch.setattr(python_interpreter.shutil, "which", lambda _name: None)

    def fake_run_command(command, **_kwargs):
        return "", f"missing {' '.join(command)}", 127

    monkeypatch.setattr(python_interpreter, "_run_command", fake_run_command)

    with pytest.raises(RuntimeError) as excinfo:
        python_interpreter.resolve_target_python_command(target)

    message = str(excinfo.value)
    assert "no verified CPython 3.13 command available" in message
    if sys.platform.startswith("win"):
        assert "py -3.13" in message
    assert "python3.13" in message


def test_molt_diff_python_version_uses_shared_multi_token_resolver(monkeypatch):
    calls: list[tuple[str, object]] = []

    def fake_resolve(target_python, **kwargs):
        calls.append((target_python.short, kwargs.get("cwd")))
        return ["py", "-3.13"]

    monkeypatch.setattr(
        molt_diff.python_interpreter,
        "resolve_target_python_command",
        fake_resolve,
    )

    assert molt_diff._resolve_diff_python_command("3.13") == ("py", "-3.13")
    assert calls == [("3.13", molt_diff._repo_root())]


def test_molt_diff_metadata_probe_preserves_multi_token_python_command(monkeypatch):
    commands: list[list[str]] = []
    molt_diff._python_command_version.cache_clear()

    def fake_probe(command):
        commands.append(command)
        return subprocess.CompletedProcess(command, 0, "(3, 13)\n", "")

    monkeypatch.setattr(molt_diff, "_run_metadata_probe", fake_probe)

    assert molt_diff._python_exe_version(("py", "-3.13")) == (3, 13)
    assert commands == [
        ["py", "-3.13", "-c", "import sys; print(sys.version_info[:2])"]
    ]
