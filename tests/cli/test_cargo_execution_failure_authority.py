from __future__ import annotations

import importlib
import inspect
import json
from pathlib import Path
import subprocess
from types import SimpleNamespace

import pytest

from molt.cli.models import _RuntimeArtifactState


CARGO = importlib.import_module("molt.cli.cargo_execution")
RUNTIME = importlib.import_module("molt.cli.runtime_build")


def _completed(
    command: list[str],
    returncode: int,
    *,
    stdout: str | bytes = "",
    stderr: str | bytes = "",
    elapsed_s: float = 0.01,
    peak_process_kb: int = 32,
    peak_tree_kb: int = 64,
) -> subprocess.CompletedProcess[object]:
    result: subprocess.CompletedProcess[object] = subprocess.CompletedProcess(
        command, returncode, stdout, stderr
    )
    result.elapsed_s = elapsed_s  # type: ignore[attr-defined]
    result.peak = SimpleNamespace(rss_kb=peak_process_kb)  # type: ignore[attr-defined]
    result.peak_total = SimpleNamespace(rss_kb=peak_tree_kb)  # type: ignore[attr-defined]
    result.timed_out = False  # type: ignore[attr-defined]
    result.guard_signal = None  # type: ignore[attr-defined]
    return result


def test_real_rustc_failure_with_sccache_command_is_never_retry_authority(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    command = ["cargo", "rustc"]
    calls: list[dict[str, str]] = []
    stderr = "\n".join(
        (
            "error: could not compile `molt-runtime` (lib)",
            "Caused by:",
            "  process didn't exit successfully: `/usr/bin/sccache /usr/bin/rustc "
            "--crate-name molt_runtime` (signal: 9, SIGKILL: kill)",
        )
    )

    def run(_cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[object]:
        calls.append(dict(kwargs["env"]))  # type: ignore[arg-type]
        return _completed(command, 101, stderr=stderr)

    monkeypatch.setattr(CARGO, "_run_completed_command", run)
    result = CARGO._run_cargo_with_sccache_retry(
        command,
        cwd=Path.cwd(),
        env={"RUSTC_WRAPPER": "/usr/bin/sccache"},
        timeout=1.0,
        json_output=True,
        label="Runtime build",
    )

    assert result.returncode == 101
    assert len(calls) == 1
    assert result.retry_reason is None
    evidence = CARGO.cargo_execution_evidence(result)
    assert evidence["attempt_count"] == 1
    assert evidence["signal"] == {
        "number": 9,
        "name": "SIGKILL",
        "source": "cargo-diagnostic",
    }


@pytest.mark.parametrize(
    ("stderr", "reason"),
    [
        ("sccache: error: cache server unavailable", "explicit-sccache-error"),
        (
            "error: failed to execute process `/opt/bin/sccache /usr/bin/rustc -vV`",
            "sccache-launch-failure",
        ),
    ],
)
def test_explicit_wrapper_failure_retries_once_and_retains_both_attempts(
    monkeypatch: pytest.MonkeyPatch,
    stderr: str,
    reason: str,
) -> None:
    command = ["cargo", "build"]
    calls: list[dict[str, str]] = []

    def run(_cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[object]:
        calls.append(dict(kwargs["env"]))  # type: ignore[arg-type]
        if len(calls) == 1:
            return _completed(
                command,
                2,
                stderr=stderr,
                elapsed_s=0.2,
                peak_process_kb=10,
                peak_tree_kb=20,
            )
        return _completed(
            command,
            101,
            stderr="error[E0425]: retry reached rustc",
            elapsed_s=0.3,
            peak_process_kb=30,
            peak_tree_kb=40,
        )

    monkeypatch.setattr(CARGO, "_run_completed_command", run)
    result = CARGO._run_cargo_with_sccache_retry(
        command,
        cwd=Path.cwd(),
        env={"RUSTC_WRAPPER": "C:/tools/sccache.exe"},
        timeout=1.0,
        json_output=True,
        label="Runtime build",
    )

    assert result.returncode == 101
    assert result.retry_reason == reason
    assert ["RUSTC_WRAPPER" in env for env in calls] == [True, False]
    evidence = CARGO.cargo_execution_evidence(result)
    assert evidence["attempt_count"] == 2
    assert evidence["duration_seconds"] == pytest.approx(0.5)
    assert evidence["peak_process_rss_bytes"] == 30 * 1024
    assert evidence["peak_tree_rss_bytes"] == 40 * 1024
    attempts = evidence["attempts"]
    assert isinstance(attempts, list)
    assert attempts[0]["failure_kind"] == reason
    assert stderr in attempts[0]["stderr"]
    assert "retry reached rustc" in attempts[1]["stderr"]


def test_tempfile_cargo_path_uses_same_retry_and_evidence_authority(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    command = ["cargo", "rustc"]
    calls: list[dict[str, str]] = []

    def fail_pipe_runner(*_args: object, **_kwargs: object) -> object:
        raise AssertionError("tempfile cargo path used the pipe runner")

    def tempfile_runner(
        _cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        calls.append(dict(kwargs["env"]))  # type: ignore[arg-type]
        if len(calls) == 1:
            return _completed(  # type: ignore[return-value]
                command, 1, stderr=b"sccache: error: transport reset"
            )
        return _completed(command, 0, stdout=b"cargo-json\n")  # type: ignore[return-value]

    monkeypatch.setattr(CARGO, "_run_completed_command", fail_pipe_runner)
    result = CARGO._run_cargo_with_sccache_retry(
        command,
        cwd=Path.cwd(),
        env={"RUSTC_WRAPPER": "/usr/bin/sccache"},
        timeout=1.0,
        json_output=True,
        label="Runtime wasm build",
        tempfile_runner=tempfile_runner,
        progress_label=None,
    )

    assert result.returncode == 0
    assert result.stdout == "cargo-json\n"
    assert len(result.attempts) == 2


def test_guard_timeout_is_preserved_in_execution_evidence(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    command = ["cargo", "build"]
    timed_out = _completed(command, 124, stderr="memory_guard: timeout")
    timed_out.timed_out = True  # type: ignore[attr-defined]
    monkeypatch.setattr(
        CARGO, "_run_completed_command", lambda *_args, **_kwargs: timed_out
    )

    result = CARGO._run_cargo_with_sccache_retry(
        command,
        cwd=Path.cwd(),
        env={},
        timeout=1.0,
        json_output=True,
        label="Runtime build",
    )

    evidence = CARGO.cargo_execution_evidence(result)
    assert evidence["timed_out"] is True
    assert evidence["attempts"][0]["timed_out"] is True


def test_terminal_cargo_summary_preserves_signal_after_long_command() -> None:
    command = "/usr/bin/sccache /usr/bin/rustc " + ("--extern dependency " * 500)
    summary = RUNTIME._native_runtime_first_error(
        cargo_stdout="",
        cargo_stderr=(
            "error: could not compile `molt-runtime` (lib)\n"
            f"process didn't exit successfully: `{command}` (signal: 9, SIGKILL: kill)\n"
        ),
        fallback="Cargo exited with code 101",
    )

    assert "could not compile `molt-runtime`" in summary
    assert "signal: 9, SIGKILL" in summary
    assert "Cargo exited with code 101" in summary
    assert len(summary) <= RUNTIME._NATIVE_RUNTIME_SUMMARY_LIMIT


def test_native_failure_receipt_carries_attempts_signal_timing_and_rss(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    command = ["cargo", "rustc"]
    results = iter(
        (
            _completed(
                command,
                2,
                stderr="sccache: error: reset",
                elapsed_s=1.25,
                peak_process_kb=100,
                peak_tree_kb=200,
            ),
            _completed(
                command,
                101,
                stderr=(
                    "error: could not compile `molt-runtime`\n"
                    "process didn't exit successfully (signal: 9, SIGKILL: kill)"
                ),
                elapsed_s=2.5,
                peak_process_kb=300,
                peak_tree_kb=400,
            ),
        )
    )
    monkeypatch.setattr(
        CARGO, "_run_completed_command", lambda *_args, **_kwargs: next(results)
    )
    cargo_result = CARGO._run_cargo_with_sccache_retry(
        command,
        cwd=tmp_path,
        env={"RUSTC_WRAPPER": "/usr/bin/sccache"},
        timeout=10.0,
        json_output=True,
        label="Runtime build",
    )
    monkeypatch.setattr(RUNTIME, "_build_state_root", lambda _root: tmp_path / "state")
    state = _RuntimeArtifactState()

    assert not RUNTIME._record_native_runtime_failure(
        state,
        project_root=tmp_path,
        stage="cargo",
        summary="release runtime compile failed",
        command=command,
        cargo_stdout=cargo_result.stdout,
        cargo_stderr=cargo_result.stderr,
        returncode=cargo_result.returncode,
        cargo_result=cargo_result,
    )
    failure = state.native_runtime_build_failure
    assert failure is not None and failure.evidence_path is not None
    payload = json.loads(failure.evidence_path.read_text(encoding="utf-8"))
    assert payload["schema"] == "molt.native-runtime-build-failure.v2"
    assert payload["schema_version"] == 2
    execution = payload["cargo_execution"]
    assert execution["schema"] == "molt.cargo-execution.v1"
    assert execution["attempt_count"] == 2
    assert execution["retry_reason"] == "explicit-sccache-error"
    assert payload["duration_seconds"] == pytest.approx(3.75)
    assert payload["peak_process_rss_bytes"] == 300 * 1024
    assert payload["peak_tree_rss_bytes"] == 400 * 1024
    assert payload["signal"]["name"] == "SIGKILL"
    assert execution["attempts"][0]["schema"] == "molt.cargo-attempt.v1"
    assert "sccache: error" in execution["attempts"][0]["stderr"]
    assert "could not compile" in execution["attempts"][1]["stderr"]
    assert failure.json_payload()["attempt_count"] == 2


def test_runtime_wasm_builds_have_no_private_sccache_retry_lane() -> None:
    source = inspect.getsource(RUNTIME)
    assert "retry_env = env.copy()" not in source
    assert "retry_env = build_env.copy()" not in source
    assert 'Path(wrapper).name == "sccache"' not in source
    assert source.count("_run_cargo_with_sccache_retry(") == 4
