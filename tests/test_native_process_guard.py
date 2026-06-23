from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Any

from tests import native_process_guard


def test_run_native_test_process_uses_shared_memory_guard(
    monkeypatch, tmp_path: Path
) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    monkeypatch.setattr(
        native_process_guard.process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    result = native_process_guard.run_native_test_process(
        ["python3", "-m", "molt.cli", "run", "probe.py"],
        cwd=tmp_path,
        env={"PYTHONPATH": "src"},
        timeout=60,
    )

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    assert captured["cmd"] == ["python3", "-m", "molt.cli", "run", "probe.py"]
    assert captured["kwargs"]["prefix"] == "MOLT_NATIVE_TEST"
    assert captured["kwargs"]["cwd"] == tmp_path
    assert captured["kwargs"]["timeout"] == 60


def test_run_native_test_process_exposes_env_overridable_default_timeout(
    monkeypatch,
) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    monkeypatch.setattr(
        native_process_guard.process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    native_process_guard.run_native_test_process(
        ["true"],
        env={"MOLT_NATIVE_TEST_TIMEOUT_SEC": "1200"},
        default_timeout=600,
    )

    assert captured["kwargs"]["timeout"] == 1200


def test_run_native_test_process_uses_custom_default_timeout(monkeypatch) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    monkeypatch.setattr(
        native_process_guard.process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    native_process_guard.run_native_test_process(["true"], default_timeout=600)

    assert captured["kwargs"]["timeout"] == 600


def test_run_native_test_process_preserves_check_semantics(monkeypatch) -> None:
    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        return subprocess.CompletedProcess(cmd, 9, "out", "err")

    monkeypatch.setattr(
        native_process_guard.process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    try:
        native_process_guard.run_native_test_process(["false"], check=True)
    except subprocess.CalledProcessError as exc:
        assert exc.returncode == 9
        assert exc.output == "out"
        assert exc.stderr == "err"
    else:  # pragma: no cover - assertion clarity
        raise AssertionError("expected CalledProcessError")
