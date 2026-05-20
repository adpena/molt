from __future__ import annotations

import subprocess

import pytest

from tools import quint_trace_to_tests


def test_run_quint_trace_uses_memory_guard(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def fake_guarded_completed_process(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, stdout="[State 0]\n{}\n", stderr="")

    monkeypatch.setattr(
        quint_trace_to_tests.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    raw = quint_trace_to_tests._run_quint_trace(
        "formal/quint/model.qnt",
        max_steps=3,
        invariant="Inv",
    )

    assert raw == "[State 0]\n{}\n"
    assert captured["cmd"] == [
        "quint",
        "run",
        "formal/quint/model.qnt",
        "--max-steps=3",
        "--invariant=Inv",
    ]
    assert captured["kwargs"]["prefix"] == "MOLT_TEST_SUITE"
    assert captured["kwargs"]["timeout"] == 120


def test_run_quint_trace_reports_guard_failure(monkeypatch) -> None:
    def fake_guarded_completed_process(cmd, **kwargs):
        return subprocess.CompletedProcess(cmd, 124, stdout="", stderr="timeout")

    monkeypatch.setattr(
        quint_trace_to_tests.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    with pytest.raises(RuntimeError, match="quint run failed"):
        quint_trace_to_tests._run_quint_trace("formal/quint/model.qnt")
