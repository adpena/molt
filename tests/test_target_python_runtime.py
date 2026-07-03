from __future__ import annotations

import subprocess
import sys

import pytest

from tools import target_python_runtime
from tests import molt_diff


def test_target_python_candidates_are_cross_platform_without_ambient_default():
    target = target_python_runtime.parse_target_python_version("3.14")

    candidates = target_python_runtime.target_python_command_candidates(target)

    assert [sys.executable] not in candidates
    assert ["python3.14"] in candidates
    assert ["python314"] in candidates
    if sys.platform.startswith("win"):
        assert ["py", "-3.14"] in candidates


def test_target_python_override_preserves_multi_token_command():
    target = target_python_runtime.parse_target_python_version("3.13")

    assert target_python_runtime.target_python_command_candidates(
        target,
        override="py -3.13",
    ) == [["py", "-3.13"]]


def test_target_python_resolution_fails_closed_with_attempts(monkeypatch):
    target = target_python_runtime.parse_target_python_version("3.13")
    monkeypatch.setattr(target_python_runtime.shutil, "which", lambda _name: None)

    def fake_run_command(command, **_kwargs):
        return "", f"missing {' '.join(command)}", 127

    monkeypatch.setattr(target_python_runtime, "_run_command", fake_run_command)

    with pytest.raises(RuntimeError) as excinfo:
        target_python_runtime.resolve_target_python_command(target)

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
        molt_diff.target_python_runtime,
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
