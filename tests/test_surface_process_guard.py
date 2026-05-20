from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Any

from tests import surface_process_guard


def test_run_surface_test_process_uses_shared_memory_guard(
    monkeypatch, tmp_path: Path
) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    monkeypatch.setattr(
        surface_process_guard.process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    result = surface_process_guard.run_surface_test_process(
        ["python3", "-c", "print('ok')"],
        cwd=tmp_path,
        timeout=5,
        check=True,
    )

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    assert captured["cmd"] == ["python3", "-c", "print('ok')"]
    assert captured["kwargs"]["prefix"] == "MOLT_SURFACE_TEST"
    assert captured["kwargs"]["cwd"] == tmp_path
    assert captured["kwargs"]["timeout"] == 5


def test_run_surface_test_process_preserves_check_semantics(monkeypatch) -> None:
    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        return subprocess.CompletedProcess(cmd, 11, "out", "err")

    monkeypatch.setattr(
        surface_process_guard.process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    try:
        surface_process_guard.run_surface_test_process(["false"], check=True)
    except subprocess.CalledProcessError as exc:
        assert exc.returncode == 11
        assert exc.output == "out"
        assert exc.stderr == "err"
    else:  # pragma: no cover - assertion clarity
        raise AssertionError("expected CalledProcessError")
