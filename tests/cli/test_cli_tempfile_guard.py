from __future__ import annotations

import os
import subprocess
from pathlib import Path
from typing import Any

from molt.cli import command_runtime as COMMAND_RUNTIME


def test_run_subprocess_captured_to_tempfiles_delegates_to_shared_guard(
    monkeypatch,
    tmp_path: Path,
) -> None:
    captured: dict[str, Any] = {}

    class FakeHarnessMemoryGuard:
        def guarded_completed_process_to_tempfiles(self, cmd, **kwargs):  # type: ignore[no-untyped-def]
            captured["cmd"] = cmd
            captured["kwargs"] = kwargs
            return subprocess.CompletedProcess(list(cmd), 0, b"out", b"err")

    monkeypatch.setattr(
        COMMAND_RUNTIME,
        "_load_cli_harness_memory_guard",
        lambda cwd: FakeHarnessMemoryGuard(),
    )

    result = COMMAND_RUNTIME._run_subprocess_captured_to_tempfiles(
        ["backend", "--emit"],
        input=b"ir",
        cwd=tmp_path,
        env={"PATH": os.environ.get("PATH", "")},
        timeout=12.0,
        progress_label="Backend compile",
    )

    assert result.returncode == 0
    assert result.stdout == b"out"
    assert captured["cmd"] == ["backend", "--emit"]
    assert captured["kwargs"]["prefix"] == "MOLT_CLI"
    assert captured["kwargs"]["input"] == b"ir"
    assert captured["kwargs"]["cwd"] == tmp_path
    assert captured["kwargs"]["timeout"] == 12.0
    assert captured["kwargs"]["progress_label"] == "Backend compile"
