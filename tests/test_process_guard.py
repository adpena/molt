from __future__ import annotations

import subprocess

import pytest

from molt import process_guard


def test_typed_argv_boundary_rejects_shell_text() -> None:
    with pytest.raises(TypeError, match="typed argv"):
        process_guard.run_completed_command("git status")  # type: ignore[arg-type]


def test_bounded_unguarded_probe_preserves_subprocess_contract(monkeypatch) -> None:
    calls: list[tuple[list[str], dict[str, object]]] = []

    def fake_run(command: list[str], **kwargs: object):
        calls.append((command, kwargs))
        return subprocess.CompletedProcess(command, 0, "head\n", "")

    monkeypatch.setattr(process_guard.subprocess, "run", fake_run)

    result = process_guard.run_completed_command(
        ["git", "rev-parse", "HEAD"],
        memory_guard_prefix=None,
        capture_output=True,
        text=True,
        check=True,
    )

    assert result.stdout == "head\n"
    assert calls[0][0] == ["git", "rev-parse", "HEAD"]
    assert "shell" not in calls[0][1]
    assert calls[0][1]["timeout"] == 30.0


def test_guarded_boundary_rejects_false_stderr_interleaving_contract() -> None:
    with pytest.raises(ValueError, match="preserve stdout/stderr separately"):
        process_guard.run_completed_command(
            ["python", "tool.py"],
            memory_guard_prefix="MOLT_TEST",
            stdout=subprocess.PIPE,
            text=True,
            stderr=subprocess.STDOUT,
        )


def test_capture_output_rejects_explicit_streams_before_dispatch() -> None:
    with pytest.raises(ValueError, match="capture_output cannot be combined"):
        process_guard.run_completed_command(
            ["git", "status"],
            memory_guard_prefix=None,
            capture_output=True,
            stdout=subprocess.PIPE,
        )


def test_guarded_timeout_without_requested_timeout_fails_closed() -> None:
    class FakeContext:
        @classmethod
        def from_env(cls, *_args: object, **_kwargs: object) -> "FakeContext":
            return cls()

        def run(self, command: list[str], **_kwargs: object) -> object:
            return type(
                "GuardedResult",
                (),
                {
                    "timed_out": True,
                    "stdout": "partial",
                    "stderr": "guard timed out",
                    "returncode": 124,
                },
            )()

    harness = type(
        "FakeHarness",
        (),
        {"HarnessExecutionContext": FakeContext},
    )

    with pytest.raises(RuntimeError, match="timeout custody is inconsistent"):
        process_guard.run_completed_command(
            ["compiler", "input.py"],
            memory_guard_prefix="MOLT_TEST",
            timeout=None,
            guard_loader=lambda _cwd: harness,
        )
