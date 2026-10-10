"""A guard failure never reaches a CLI caller as the child's exit status (HF-29).

The memory guard reports its own failure (for example incomplete temporary
artifact custody) as exit code 125, and a timeout as 124. Before HF-29 the
guarded runners returned those codes as if the child had produced them, so
``molt build`` blamed the child: ``uv lock --check`` passed, the guard failed,
and the build reported a stale ``uv.lock``. These tests drive the real runners
with a fake guard result at the harness boundary; the oracle is the typed
outcome each caller receives.
"""

from __future__ import annotations

import io
import json
import shutil
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from molt import process_guard
from molt.cli import command_runtime, entrypoint, lockfiles
from molt.process_guard import GuardInfrastructureError
from tests.process_guard_common import install_module_view
from tools import harness_memory_guard
from tools.memory_guard_core.process_custody import GuardInfrastructureFailure


def _guard_result(
    command: list[str],
    *,
    child_returncode: int,
    infrastructure: bool = False,
    timed_out: bool = False,
    stdout: Any = "",
    stderr: Any = "",
) -> harness_memory_guard.GuardedCompletedProcess:
    failure = (
        GuardInfrastructureFailure(
            phase="temporary_artifact_custody",
            details=("temporary artifact terminal state is 'cleanup-error'",),
        )
        if infrastructure
        else None
    )
    if timed_out:
        returncode = 124
    elif infrastructure and child_returncode == 0:
        returncode = 125
    else:
        returncode = child_returncode
    return harness_memory_guard.GuardedCompletedProcess(
        command,
        returncode,
        stdout,
        stderr,
        elapsed_s=0.01,
        child_stderr=stderr,
        timed_out=timed_out,
        child_returncode=child_returncode,
        infrastructure_failure=failure,
    )


def _install_guard(monkeypatch: pytest.MonkeyPatch, result_for: Any) -> list[list[str]]:
    """Answer every guarded completed command with ``result_for(command)``."""
    calls: list[list[str]] = []

    class Context:
        @classmethod
        def from_env(cls, prefix: str, env: Any, *, repo_root: Path) -> "Context":
            del prefix, env, repo_root
            return cls()

        def run(self, command: list[str], **_kwargs: Any) -> Any:
            calls.append(list(command))
            return result_for(list(command))

    fake = SimpleNamespace(HarnessExecutionContext=Context)
    monkeypatch.setattr(
        command_runtime, "_load_cli_harness_memory_guard", lambda _cwd: fake
    )
    return calls


def _lock_project(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    project = tmp_path / "project"
    project.mkdir()
    (project / "pyproject.toml").write_text("[project]\nname='p'\n", encoding="utf-8")
    (project / "uv.lock").write_text("version = 1\n", encoding="utf-8")
    # The lock-check memo lives under the Cargo target root; keep it private.
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "target"))
    install_module_view(
        monkeypatch,
        "shutil",
        shutil,
        lockfiles,
        which=lambda name: f"/usr/bin/{name}",
    )
    return project


def test_uv_lock_check_reports_guard_failure_not_a_stale_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = _lock_project(tmp_path, monkeypatch)
    calls = _install_guard(
        monkeypatch,
        lambda command: _guard_result(
            command,
            child_returncode=0,
            infrastructure=True,
            stderr="memory_guard: temporary artifact custody incomplete\n",
        ),
    )

    with pytest.raises(GuardInfrastructureError) as raised:
        lockfiles._verify_uv_lock(project)

    assert calls == [["uv", "lock", "--check"]]
    error = raised.value
    message = str(error)
    assert "out of date" not in message
    assert message.startswith(
        "memory guard infrastructure failure (temporary_artifact_custody) "
        "while running uv lock --check: "
    )
    assert message.endswith("the child exited with 0")
    assert error.child_returncode == 0
    assert not hasattr(error, "returncode")
    assert error.guarded_result.returncode == 125


def test_uv_lock_check_still_reports_a_stale_lock_from_the_child(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = _lock_project(tmp_path, monkeypatch)
    _install_guard(
        monkeypatch,
        lambda command: _guard_result(
            command, child_returncode=1, stderr="error: lockfile needs update\n"
        ),
    )

    assert lockfiles._verify_uv_lock(project) == (
        "uv.lock is out of date or invalid: error: lockfile needs update"
    )


@pytest.mark.parametrize("child_returncode", [0, 3])
def test_completed_command_never_returns_a_guard_failure(
    monkeypatch: pytest.MonkeyPatch, child_returncode: int
) -> None:
    _install_guard(
        monkeypatch,
        lambda command: _guard_result(
            command, child_returncode=child_returncode, infrastructure=True
        ),
    )

    with pytest.raises(GuardInfrastructureError) as raised:
        command_runtime._run_completed_command(
            ["tool", "--flag"],
            env=None,
            cwd=None,
            capture_output=True,
            memory_guard_prefix="MOLT_BUILD",
        )

    assert raised.value.child_returncode == child_returncode
    assert str(raised.value).endswith(f"the child exited with {child_returncode}")
    # Callers that catch the subprocess family keep working.
    assert isinstance(raised.value, subprocess.SubprocessError)


def _install_tempfile_guard(monkeypatch: pytest.MonkeyPatch, result: Any) -> None:
    fake = SimpleNamespace(
        guarded_completed_process_to_tempfiles=lambda command, **_kwargs: result
    )
    monkeypatch.setattr(
        command_runtime, "_load_cli_harness_memory_guard", lambda _cwd: fake
    )


def test_tempfile_runner_raises_a_guard_timeout_instead_of_returning_124(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    command = ["molt-backend", "--ir-file", "ir.json"]
    _install_tempfile_guard(
        monkeypatch,
        _guard_result(
            command, child_returncode=-9, timed_out=True, stdout=b"", stderr=b""
        ),
    )

    with pytest.raises(subprocess.TimeoutExpired) as raised:
        command_runtime._run_subprocess_captured_to_tempfiles(command, timeout=5.0)

    assert raised.value.timeout == 5.0
    assert raised.value.guarded_result.returncode == 124  # type: ignore[attr-defined]


def test_tempfile_runner_raises_a_guard_failure_instead_of_returning_125(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    command = ["molt-backend", "--ir-file", "ir.json"]
    _install_tempfile_guard(
        monkeypatch,
        _guard_result(
            command, child_returncode=0, infrastructure=True, stdout=b"", stderr=b""
        ),
    )

    with pytest.raises(GuardInfrastructureError) as raised:
        command_runtime._run_subprocess_captured_to_tempfiles(command, timeout=5.0)

    assert raised.value.command == tuple(command)


def test_tempfile_runner_returns_a_child_exit_of_124_as_its_result(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    command = ["molt-backend"]
    _install_tempfile_guard(
        monkeypatch,
        _guard_result(command, child_returncode=124, stdout=b"", stderr=b""),
    )

    result = command_runtime._run_subprocess_captured_to_tempfiles(command, timeout=5.0)

    assert result.returncode == 124


@pytest.mark.parametrize("json_output", [False, True])
def test_entrypoint_reports_a_guard_failure_with_the_guard_exit_code(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    json_output: bool,
) -> None:
    error = GuardInfrastructureError(
        ["uv", "lock", "--check"],
        _guard_result(
            ["uv", "lock", "--check"], child_returncode=0, infrastructure=True
        ),
    )

    def dispatch(*_args: Any, **_kwargs: Any) -> int:
        raise error

    entry = tmp_path / "app.py"
    entry.write_text("print(1)\n", encoding="utf-8")
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(entrypoint, "check_process_environment", lambda: None)
    monkeypatch.setattr(entrypoint, "_dispatch_entrypoint_command", dispatch)
    argv = ["molt", "build", *(["--json"] if json_output else []), str(entry)]
    monkeypatch.setattr(sys, "argv", argv)
    monkeypatch.setattr(sys, "stdin", io.StringIO(""))

    rc = entrypoint.main(build_fn=lambda *_a, **_k: 0)

    assert rc == process_guard.guard_infrastructure_exit_code() == 125
    captured = capsys.readouterr()
    if json_output:
        payload = json.loads(captured.out)
        assert payload["status"] == "error"
        assert payload["errors"] == [str(error)]
        assert payload["data"]["returncode"] == 125
    else:
        assert captured.err.strip() == str(error)
