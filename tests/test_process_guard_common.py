from __future__ import annotations

import subprocess
from typing import Any

from tests import process_guard_common


def test_run_guarded_test_process_preserves_prefix_and_timeout(monkeypatch) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    result = process_guard_common.run_guarded_test_process(
        ["python3", "-c", "print('ok')"],
        prefix="MOLT_UNIT_TEST",
        env={"MOLT_UNIT_TEST_TIMEOUT_SEC": "12"},
    )

    assert result.returncode == 0
    assert captured["cmd"] == ["python3", "-c", "print('ok')"]
    assert captured["kwargs"]["prefix"] == "MOLT_UNIT_TEST"
    assert captured["kwargs"]["timeout"] == 12


def test_run_guarded_test_process_preserves_check_semantics(monkeypatch) -> None:
    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        return subprocess.CompletedProcess(cmd, 17, "out", "err")

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    try:
        process_guard_common.run_guarded_test_process(
            ["false"],
            prefix="MOLT_UNIT_TEST",
            check=True,
        )
    except subprocess.CalledProcessError as exc:
        assert exc.returncode == 17
        assert exc.output == "out"
        assert exc.stderr == "err"
    else:  # pragma: no cover - assertion clarity
        raise AssertionError("expected CalledProcessError")


def test_run_guarded_test_process_preserves_timeout_semantics(monkeypatch) -> None:
    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        return subprocess.CompletedProcess(
            cmd,
            process_guard_common.harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE,
            "",
            "memory_guard: timeout after 5s\n",
        )

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    try:
        process_guard_common.run_guarded_test_process(
            ["sleep", "10"],
            prefix="MOLT_UNIT_TEST",
            timeout=5,
        )
    except subprocess.TimeoutExpired as exc:
        assert exc.timeout == 5
        assert exc.stderr == "memory_guard: timeout after 5s\n"
    else:  # pragma: no cover - assertion clarity
        raise AssertionError("expected TimeoutExpired")
