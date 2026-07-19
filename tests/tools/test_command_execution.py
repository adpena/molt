from __future__ import annotations

import subprocess
from types import SimpleNamespace

import pytest

from tools import command_execution


def test_executor_routes_only_bounded_metadata_to_direct_probe(monkeypatch) -> None:
    calls: list[dict[str, object]] = []

    def fake_run(_command: list[str], **kwargs: object):
        calls.append(kwargs)
        return subprocess.CompletedProcess([], 0, "", "")

    authority = SimpleNamespace(run_completed_command=fake_run)
    monkeypatch.setattr(
        command_execution,
        "_process_guard_authority",
        lambda _root: authority,
    )
    executor = command_execution.CommandExecutor.for_file(__file__)

    executor.run(["git", "status", "--porcelain"], capture_output=True, text=True)
    executor.run(["python", "tool.py"], capture_output=True, text=True)

    assert calls[0]["memory_guard_prefix"] is None
    assert calls[1]["memory_guard_prefix"] == executor.prefix


def test_executor_rejects_shell_text() -> None:
    executor = command_execution.CommandExecutor.for_file(__file__)
    with pytest.raises(TypeError, match="typed argv"):
        executor.run("git status")  # type: ignore[arg-type]


def test_executor_rejects_capture_output_with_explicit_stream() -> None:
    executor = command_execution.CommandExecutor.for_file(__file__)
    with pytest.raises(ValueError, match="capture_output cannot be combined"):
        executor.run(
            ["git", "status"],
            capture_output=True,
            stdout=subprocess.PIPE,
        )


def test_read_only_git_classifier_excludes_mutations() -> None:
    assert command_execution._is_bounded_metadata_probe(["git", "rev-parse", "HEAD"])
    assert command_execution._is_bounded_metadata_probe(
        ["git", "-C", "repo", "status", "--porcelain"]
    )
    assert not command_execution._is_bounded_metadata_probe(
        ["git", "commit", "-m", "message"]
    )
