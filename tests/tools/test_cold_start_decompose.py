from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace
import shutil
import tempfile
from tests.process_guard_common import install_module_view
from tools import cold_start_decompose


def test_safe_run_elapsed_uses_shared_guard(monkeypatch, tmp_path: Path) -> None:
    module = cold_start_decompose
    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        return SimpleNamespace(
            returncode=0,
            stdout="ignored\n",
            stderr='SAFE_RUN {"status":"ok","elapsed_s":0.042}\n',
            elapsed_s=0.05,
        )

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    elapsed, stderr = module._safe_run_elapsed(
        ["/bin/echo", "ok"],
        env={"MOLT_SESSION_ID": "test"},
        rss_mb=64,
        timeout_s=2.0,
        label="probe",
        extra_env={"EXTRA": "1"},
    )

    assert elapsed == 0.042
    assert "SAFE_RUN" in stderr
    call = calls[0]
    assert call["cmd"][:2] == [module.sys.executable, str(module.SAFE_RUN)]
    assert call["cmd"][-2:] == ["/bin/echo", "ok"]
    assert call["prefix"] == "MOLT_COLD_START"
    assert call["capture_output"] is True
    assert call["text"] is True
    assert call["timeout"] == 32.0
    assert call["env"] == {"MOLT_SESSION_ID": "test", "EXTRA": "1"}


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
