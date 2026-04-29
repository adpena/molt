from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
CI_GATE = REPO_ROOT / "tools" / "ci_gate.py"


def _load_ci_gate():
    spec = importlib.util.spec_from_file_location("molt_tools_ci_gate", CI_GATE)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_run_check_uses_memory_guard_by_default(monkeypatch) -> None:
    module = _load_ci_gate()
    calls: list[dict[str, object]] = []

    def fake_run_guarded(command, **kwargs):
        calls.append({"command": list(command), **kwargs})
        return module.memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="ok\n",
            stderr="",
        )

    monkeypatch.setattr(module.memory_guard, "run_guarded", fake_run_guarded)
    check = module.Check(
        name="unit",
        tier=1,
        cmd=["python3", "-c", "print('ok')"],
        timeout=7,
    )

    result = module._run_check(
        check,
        memory_limits=module.MemoryGuardLimits(
            max_rss_gb=2.0,
            max_total_rss_gb=3.0,
            poll_interval=0.5,
        ),
    )

    assert result.status == "pass"
    assert result.stdout == "ok\n"
    assert calls == [
        {
            "command": ["python3", "-c", "print('ok')"],
            "max_rss_kb": 2 * 1024 * 1024,
            "max_total_rss_kb": 3 * 1024 * 1024,
            "poll_interval": 0.5,
            "cwd": str(module.ROOT),
            "env": calls[0]["env"],
            "timeout": 7,
            "capture_output": True,
        }
    ]
    assert calls[0]["env"]["PYTHONPATH"] == str(module.ROOT / "src")


def test_run_check_can_opt_out_of_memory_guard(monkeypatch) -> None:
    module = _load_ci_gate()
    direct_calls: list[dict[str, object]] = []

    def fail_run_guarded(*_args, **_kwargs):
        raise AssertionError("memory guard should not run")

    def fake_subprocess_run(command, **kwargs):
        direct_calls.append({"command": list(command), **kwargs})
        return subprocess.CompletedProcess(command, 0, "direct\n", "")

    monkeypatch.setattr(module.memory_guard, "run_guarded", fail_run_guarded)
    monkeypatch.setattr(module.subprocess, "run", fake_subprocess_run)

    result = module._run_check(
        module.Check(name="unit", tier=1, cmd=["python3", "-c", "print('ok')"]),
        memory_limits=None,
    )

    assert result.status == "pass"
    assert result.stdout == "direct\n"
    assert direct_calls[0]["command"] == ["python3", "-c", "print('ok')"]


def test_run_check_acquires_compile_slot_for_rust_checks(monkeypatch) -> None:
    module = _load_ci_gate()
    slot_calls: list[dict[str, object]] = []

    class FakeSlot:
        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, tb) -> None:
            return None

    def fake_compile_slot(**kwargs):
        slot_calls.append(kwargs)
        return FakeSlot()

    def fake_run_guarded(command, **kwargs):
        return module.memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="rust ok\n",
            stderr="",
        )

    monkeypatch.setattr(module.compile_governor, "compile_slot", fake_compile_slot)
    monkeypatch.setattr(module.memory_guard, "run_guarded", fake_run_guarded)

    result = module._run_check(
        module.Check(
            name="rust-build",
            tier=1,
            cmd=["cargo", "build"],
            needs_rust=True,
        )
    )

    assert result.status == "pass"
    assert result.stdout == "rust ok\n"
    assert slot_calls == [
        {
            "env": slot_calls[0]["env"],
            "label": "ci_gate:rust-build",
        }
    ]
    assert slot_calls[0]["env"]["PYTHONPATH"] == str(module.ROOT / "src")


def test_launch_background_gate_strips_recursive_background_flag(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_ci_gate()
    popen_calls: list[dict[str, object]] = []

    class FakePopen:
        pid = 4242

        def __init__(self, command, **kwargs) -> None:
            popen_calls.append({"command": list(command), **kwargs})

    monkeypatch.setattr(module, "LOG_ROOT", tmp_path)
    monkeypatch.setattr(module.subprocess, "Popen", FakePopen)

    metadata = module.launch_background_gate(
        ["--tier", "2", "--background", "--parallel"]
    )

    assert metadata.pid == 4242
    assert metadata.log_path.parent == tmp_path
    assert metadata.metadata_path.parent == tmp_path
    assert "--background" not in popen_calls[0]["command"]
    assert popen_calls[0]["command"][:2] == [module.sys.executable, str(module.CI_GATE)]
    assert metadata.metadata_path.exists()


def test_ci_gate_script_runs_directly_without_pythonpath() -> None:
    env = os.environ.copy()
    env.pop("PYTHONPATH", None)

    result = subprocess.run(
        ["python3", "tools/ci_gate.py", "--tier", "1", "--dry-run", "--json"],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)
    assert payload["summary"]["success"] is True
