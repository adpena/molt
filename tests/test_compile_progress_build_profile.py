from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = ROOT / "tools" / "compile_progress.py"


def _load_module():
    spec = importlib.util.spec_from_file_location(
        "compile_progress_under_test", MODULE_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_compile_progress_build_command_uses_build_profile_flag() -> None:
    module = _load_module()
    case = module.CaseSpec(
        name="dev_cold",
        profile="dev",
        cache_mode="cache-report",
        daemon=True,
    )

    cmd = module._build_molt_build_cmd(
        case=case,
        python_version="3.12",
        script_path="examples/hello.py",
        out_dir=Path("bench/results/out"),
        diagnostics_path=None,
    )

    assert "--build-profile" in cmd
    assert "dev" in cmd
    assert "--profile" not in cmd


def test_compile_progress_run_case_uses_memory_guard(
    monkeypatch, tmp_path: Path
) -> None:
    module = _load_module()
    case = module.CaseSpec(
        name="dev_cold",
        profile="dev",
        cache_mode="cache-report",
        daemon=True,
    )
    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": cmd, "kwargs": kwargs})
        return subprocess.CompletedProcess(cmd, 0, stdout="Cache: hit\n", stderr="")

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    logs_root = tmp_path / "logs"
    logs_root.mkdir()

    result = module._run_case(
        case=case,
        python_version="3.12",
        script_path="examples/hello.py",
        out_root=tmp_path / "out",
        logs_root=logs_root,
        repo_root=module._repo_root(),
        env_base={"CARGO_TARGET_DIR": str(tmp_path / "target")},
        timeout_sec=30.0,
        diagnostics=False,
        max_retries=0,
        retry_backoff_sec=0.0,
        build_lock_timeout_sec=None,
    )

    assert result.returncode == 0
    assert result.cache_state == "hit"
    assert calls[0]["kwargs"]["prefix"] == "MOLT_COMPILE_PROGRESS"
    assert calls[0]["kwargs"]["timeout"] == 30.0
    assert calls[0]["kwargs"]["capture_output"] is True
    assert (logs_root / "dev_cold.attempt1.stdout.log").read_text(
        encoding="utf-8"
    ) == "Cache: hit\n"


def test_compile_progress_timeout_uses_guard_cleanup(
    monkeypatch, tmp_path: Path
) -> None:
    module = _load_module()
    case = module.CaseSpec(
        name="dev_cold",
        profile="dev",
        cache_mode="cache-report",
        daemon=True,
    )

    def fake_guarded_completed_process(cmd, **kwargs):
        return subprocess.CompletedProcess(
            cmd,
            module.harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE,
            stdout="",
            stderr="timed out",
        )

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    logs_root = tmp_path / "logs"
    logs_root.mkdir()

    result = module._run_case(
        case=case,
        python_version="3.12",
        script_path="examples/hello.py",
        out_root=tmp_path / "out",
        logs_root=logs_root,
        repo_root=module._repo_root(),
        env_base={"CARGO_TARGET_DIR": "target-marker"},
        timeout_sec=30.0,
        diagnostics=False,
        max_retries=0,
        retry_backoff_sec=0.0,
        build_lock_timeout_sec=None,
    )

    assert result.timed_out is True
    assert (
        result.returncode
        == module.harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE
    )
    assert "handled_by=harness_memory_guard" in (
        logs_root / "dev_cold.attempt1.stderr.log"
    ).read_text(encoding="utf-8")
