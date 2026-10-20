from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace
import shutil
import tempfile
import pytest
from molt.process_environment import NameRule, ProcessEnvironment
from tests.process_guard_common import install_module_view
from tools import cold_start_decompose


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_safe_run_elapsed_uses_typed_shared_measurement(monkeypatch, rule):
    from tools.safe_run import RunOutcome

    calls = []

    def measure(cmd, **kwargs):
        calls.append((cmd, kwargs))
        return RunOutcome(True, 0.042, 3, "ok", 0, stderr="guest stderr")

    monkeypatch.setattr(cold_start_decompose, "_run_measured_command", measure)
    environment = ProcessEnvironment({"BASE": "1", "Extra": "old"}, rule=rule)
    elapsed, stderr = cold_start_decompose._safe_run_elapsed(
        ["subject"],
        env=environment,
        rss_mb=64,
        timeout_s=2,
        label="probe",
        extra_env={"EXTRA": "2"},
    )
    assert elapsed == 0.042 and stderr == "guest stderr"
    cmd, kwargs = calls[0]
    assert cmd == ["subject"] and kwargs["timeout_s"] == 2
    assert isinstance(kwargs["env"], ProcessEnvironment)
    assert kwargs["env"].rule is rule
    assert dict(kwargs["env"]) == (
        {"BASE": "1", "Extra": "old", "EXTRA": "2"}
        if rule is NameRule.EXACT
        else {"BASE": "1", "Extra": "2"}
    )
    assert dict(environment) == {"BASE": "1", "Extra": "old"}
    assert kwargs["prefix"] == "MOLT_COLD_START"


def test_build_noop_c_uses_shared_guard(monkeypatch, tmp_path: Path) -> None:
    module = cold_start_decompose
    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        out = Path(cmd[cmd.index("-o") + 1])
        out.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        out.chmod(0o755)
        return SimpleNamespace(returncode=0, stdout="", stderr="", elapsed_s=0.1)

    def fake_mkdtemp(prefix):
        path = tmp_path / prefix
        path.mkdir()
        return str(path)

    install_module_view(
        monkeypatch, "shutil", shutil, module, which=lambda name: "/usr/bin/cc"
    )
    install_module_view(monkeypatch, "tempfile", tempfile, module, mkdtemp=fake_mkdtemp)
    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    result = module._build_noop_c()

    assert result is not None
    assert result.exists()
    assert calls[0]["cmd"][0] == "/usr/bin/cc"
    assert calls[0]["prefix"] == "MOLT_COLD_START"
    assert calls[0]["timeout"] == 60


def test_runtime_init_phases_reject_trace_from_failed_sample(monkeypatch):
    phase = cold_start_decompose.RUNTIME_INIT_PHASES[0]
    trace = f"[molt runtime_init] +9000us (d9000us) {phase}\n"
    results = iter([(None, trace), (0.01, trace.replace("9000", "1000"))])
    monkeypatch.setattr(
        cold_start_decompose, "_safe_run_elapsed", lambda *a, **kw: next(results)
    )
    assert cold_start_decompose._measure_runtime_init_phases(
        Path("subject"), env={}, samples=2, rss_mb=64, timeout_s=2
    ) == {phase: 1.0}


def test_measure_dyld_ms_uses_shared_guard(monkeypatch, tmp_path: Path) -> None:
    module = cold_start_decompose
    binary = tmp_path / "probe"
    binary.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        return SimpleNamespace(
            returncode=0,
            stdout="",
            stderr="total time: 3.25 milliseconds\n",
            elapsed_s=0.01,
        )

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    measured = module._measure_dyld_ms(
        binary,
        env={"BASE": "1"},
        samples=2,
        timeout_s=5.0,
    )

    assert measured == 3.25
    assert len(calls) == 2
    assert calls[0]["cmd"] == [str(binary)]
    assert calls[0]["prefix"] == "MOLT_COLD_START"
    assert calls[0]["env"]["DYLD_PRINT_STATISTICS"] == "1"
