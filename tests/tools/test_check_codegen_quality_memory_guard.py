from __future__ import annotations

import subprocess

import pytest

from tools import check_codegen_quality


def test_check_output_text_uses_memory_guard(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def fake_guarded_completed_process(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, stdout="ok\n", stderr="")

    monkeypatch.setattr(
        check_codegen_quality.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    assert check_codegen_quality._check_output_text(["tool", "--version"]) == "ok\n"
    assert captured["cmd"] == ["tool", "--version"]
    assert captured["kwargs"]["prefix"] == "MOLT_CODEGEN_QUALITY"
    assert captured["kwargs"]["capture_output"] is True


def test_check_output_text_preserves_check_output_failure(monkeypatch) -> None:
    def fake_guarded_completed_process(cmd, **kwargs):
        return subprocess.CompletedProcess(cmd, 3, stdout="out", stderr="bad")

    monkeypatch.setattr(
        check_codegen_quality.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    with pytest.raises(subprocess.CalledProcessError) as exc_info:
        check_codegen_quality._check_output_text(["objdump", "-h", "binary"])

    assert exc_info.value.returncode == 3
    assert exc_info.value.output == "out"
    assert exc_info.value.stderr == "bad"
