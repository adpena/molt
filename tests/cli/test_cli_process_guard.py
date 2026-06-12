from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Any

from tests.cli import process_guard


def test_run_cli_test_process_uses_shared_memory_guard(
    monkeypatch, tmp_path: Path
) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    monkeypatch.setattr(
        process_guard.process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    result = process_guard.run_cli_test_process(
        ["python3", "-m", "molt.cli", "--help"],
        cwd=tmp_path,
        env={"PYTHONPATH": "src"},
        timeout=5,
    )

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    assert captured["cmd"] == ["python3", "-m", "molt.cli", "--help"]
    assert captured["kwargs"]["prefix"] == "MOLT_CLI_TEST"
    assert captured["kwargs"]["cwd"] == tmp_path
    assert captured["kwargs"]["timeout"] == 5


def test_run_cli_test_process_preserves_check_semantics(monkeypatch) -> None:
    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        return subprocess.CompletedProcess(cmd, 7, "out", "err")

    monkeypatch.setattr(
        process_guard.process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    try:
        process_guard.run_cli_test_process(["false"], check=True)
    except subprocess.CalledProcessError as exc:
        assert exc.returncode == 7
        assert exc.output == "out"
        assert exc.stderr == "err"
    else:  # pragma: no cover - assertion clarity
        raise AssertionError("expected CalledProcessError")


def test_cli_test_popen_kwargs_applies_child_rlimit(monkeypatch) -> None:
    monkeypatch.setenv("MOLT_CLI_TEST_MAX_PROCESS_RSS_GB", "1")

    kwargs = process_guard.cli_test_popen_kwargs({"MOLT_CLI_TEST_MEMORY_GUARD": "1"})

    assert kwargs.get("start_new_session") is True
    assert callable(kwargs.get("preexec_fn"))


def test_guarded_cli_test_popen_enters_memory_guard_wrapper(
    monkeypatch, tmp_path: Path
) -> None:
    captured: dict[str, Any] = {}

    class FakePopen:
        def __init__(self, args, **kwargs):  # type: ignore[no-untyped-def]
            captured["args"] = list(args)
            captured["kwargs"] = kwargs

    monkeypatch.setattr(process_guard.subprocess, "Popen", FakePopen)
    monkeypatch.setattr(
        process_guard.harness_memory_guard,
        "batch_process_group_kwargs",
        lambda *_args, **_kwargs: {"start_new_session": True},
    )

    proc = process_guard.guarded_cli_test_popen(
        ["python3", "-c", "print('ok')"],
        cwd=tmp_path,
        env={"MOLT_EXT_ROOT": str(tmp_path)},
    )

    assert isinstance(proc, FakePopen)
    args = captured["args"]
    assert isinstance(args, list)
    assert args[0] == process_guard.sys.executable
    assert args[1] == str(process_guard.ROOT / "tools" / "memory_guard.py")
    assert "--summary-json" in args
    assert args[-4:] == ["--", "python3", "-c", "print('ok')"]
    kwargs = captured["kwargs"]
    assert kwargs["cwd"] == tmp_path
    assert kwargs["text"] is True
    assert kwargs["start_new_session"] is True
