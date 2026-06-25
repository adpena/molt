from __future__ import annotations

import importlib.util
import json
import os
import sys
from pathlib import Path

from tests.process_guard_common import run_guarded_test_process

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

    def fake_guarded_completed_process(command, **kwargs):
        calls.append({"command": list(command), **kwargs})
        return module.harness_memory_guard.GuardedCompletedProcess(
            command,
            0,
            "ok\n",
            "",
            elapsed_s=0.1,
        )

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    check = module.Check(
        name="unit",
        tier=1,
        cmd=["python3", "-c", "print('ok')"],
        timeout=7,
    )

    result = module._run_check(
        check,
        memory_limits=module.MemoryGuardLimits(
            enabled=True,
            max_process_rss_gb=2.0,
            max_total_rss_gb=3.0,
            max_global_rss_gb=9.0,
            poll_interval=0.5,
        ),
    )

    assert result.status == "pass"
    assert result.stdout == "ok\n"
    assert calls == [
        {
            "command": ["python3", "-c", "print('ok')"],
            "prefix": "MOLT_CI_GATE",
            "cwd": str(module.ROOT),
            "env": calls[0]["env"],
            "timeout": 7,
            "capture_output": True,
            "text": True,
            "limits": calls[0]["limits"],
        }
    ]
    limits = calls[0]["limits"]
    assert limits.max_process_rss_gb == 2.0
    assert limits.max_total_rss_gb == 3.0
    assert limits.max_global_rss_gb == 9.0
    assert limits.poll_interval == 0.5
    assert limits.child_rlimit_gb == 2.0
    assert calls[0]["env"]["PYTHONPATH"] == str(module.ROOT / "src")


def test_run_check_default_limits_resolve_adaptively(monkeypatch) -> None:
    module = _load_ci_gate()
    calls: list[dict[str, object]] = []

    def fake_adaptive_memory_budget(prefix, environ=None, *, accounted_rss_kb=0):
        assert prefix == "MOLT_CI_GATE"
        assert accounted_rss_kb == 0
        return module.memory_guard.AdaptiveMemoryBudget(
            max_process_rss_gb=4.0,
            max_total_rss_gb=6.0,
            max_global_rss_gb=10.0,
            reserve_gb=1.0,
            physical_gb=16.0,
            available_gb=12.0,
            source="test",
            accounted_rss_gb=0.0,
        )

    def fake_guarded_completed_process(command, **kwargs):
        calls.append({"command": list(command), **kwargs})
        return module.harness_memory_guard.GuardedCompletedProcess(
            command,
            0,
            "adaptive\n",
            "",
            elapsed_s=0.1,
        )

    monkeypatch.setattr(
        module.memory_guard, "adaptive_memory_budget", fake_adaptive_memory_budget
    )
    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    result = module._run_check(
        module.Check(name="unit", tier=1, cmd=["python3", "-c", "print('ok')"])
    )

    assert result.status == "pass"
    limits = calls[0]["limits"]
    assert limits.max_process_rss_gb == 4.0
    assert limits.max_total_rss_gb == 6.0
    assert limits.child_rlimit_gb == 4.0


def test_check_env_seeds_canonical_artifact_roots(monkeypatch) -> None:
    module = _load_ci_gate()
    for key in (
        "MOLT_EXT_ROOT",
        "CARGO_TARGET_DIR",
        "MOLT_DIFF_CARGO_TARGET_DIR",
        "MOLT_CACHE",
        "MOLT_DIFF_ROOT",
        "MOLT_DIFF_TMPDIR",
        "UV_CACHE_DIR",
        "TMPDIR",
        "MOLT_SESSION_ID",
        "CARGO_BUILD_JOBS",
    ):
        monkeypatch.delenv(key, raising=False)

    env = module._check_env(module.Check(name="unit", tier=1, cmd=["true"]))

    assert env["MOLT_EXT_ROOT"] == str(module.ROOT)
    assert env["CARGO_TARGET_DIR"] == str(module.ROOT / "target")
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == env["CARGO_TARGET_DIR"]
    assert env["MOLT_CACHE"] == str(module.ROOT / ".molt_cache")
    assert env["MOLT_DIFF_ROOT"] == str(module.ROOT / "tmp" / "diff")
    assert env["MOLT_DIFF_TMPDIR"] == str(module.ROOT / "tmp")
    assert env["UV_CACHE_DIR"] == str(module.ROOT / ".uv-cache")
    assert env["TMPDIR"] == str(module.ROOT / "tmp")
    assert env["MOLT_SESSION_ID"].startswith("ci-gate-")
    assert env["CARGO_BUILD_JOBS"] == "2"


def test_check_env_preserves_explicit_artifact_roots(monkeypatch, tmp_path) -> None:
    module = _load_ci_gate()
    target = tmp_path / "target-custom"
    diff_target = tmp_path / "target-diff-custom"
    cache = tmp_path / "cache-custom"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target))
    monkeypatch.setenv("MOLT_DIFF_CARGO_TARGET_DIR", str(diff_target))
    monkeypatch.setenv("MOLT_CACHE", str(cache))
    monkeypatch.setenv("MOLT_SESSION_ID", "caller-session")
    monkeypatch.setenv("CARGO_BUILD_JOBS", "1")

    env = module._check_env(module.Check(name="unit", tier=1, cmd=["true"]))

    assert env["CARGO_TARGET_DIR"] == str(target)
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == str(diff_target)
    assert env["MOLT_CACHE"] == str(cache)
    assert env["MOLT_SESSION_ID"] == "caller-session"
    assert env["CARGO_BUILD_JOBS"] == "1"


def test_run_check_cannot_opt_out_of_memory_guard(monkeypatch) -> None:
    module = _load_ci_gate()
    guarded_calls: list[dict[str, object]] = []

    disabled_limits = module.MemoryGuardLimits(
        enabled=False,
        max_process_rss_gb=2.0,
        max_total_rss_gb=3.0,
        max_global_rss_gb=9.0,
        poll_interval=0.5,
    )

    def fake_guarded_completed_process(command, **kwargs):
        guarded_calls.append({"command": list(command), **kwargs})
        return module.harness_memory_guard.GuardedCompletedProcess(
            command,
            0,
            "guarded\n",
            "",
            elapsed_s=0.1,
        )

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    monkeypatch.setattr(
        module.subprocess,
        "run",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            AssertionError("ci gate used raw subprocess.run")
        ),
    )

    result = module._run_check(
        module.Check(name="unit", tier=1, cmd=["python3", "-c", "print('ok')"]),
        memory_limits=disabled_limits,
    )

    assert result.status == "pass"
    assert result.stdout == "guarded\n"
    assert guarded_calls[0]["command"] == ["python3", "-c", "print('ok')"]
    assert guarded_calls[0]["limits"].enabled is True


def test_parallel_workers_clamped_by_global_memory_budget() -> None:
    module = _load_ci_gate()
    limits = module.MemoryGuardLimits(
        enabled=True,
        max_process_rss_gb=2.0,
        max_total_rss_gb=8.0,
        max_global_rss_gb=17.0,
        poll_interval=0.5,
    )

    assert module._parallel_workers_for_memory_guard(4, memory_limits=limits) == 2
    assert module._parallel_workers_for_memory_guard(4, memory_limits=None) >= 1


def test_ci_gate_tier1_includes_perf_scoreboard_contract() -> None:
    module = _load_ci_gate()

    checks = {check.name: check for check in module._build_checks()}
    check = checks["perf-scoreboard-contract"]

    assert check.tier == 1
    assert check.required is True
    assert check.needs_pytest is True
    assert check.needs_rust is False
    assert check.timeout == 120
    assert str(module.TESTS / "tools" / "test_perf_causality.py") in check.cmd
    assert str(module.TESTS / "tools" / "test_pass_delta_dashboard.py") in check.cmd
    assert str(module.TESTS / "tools" / "test_perf_schema.py") in check.cmd
    assert str(module.TESTS / "tools" / "test_perf_scoreboard.py") in check.cmd


def test_ci_gate_tier1_includes_structural_audit_ratchet() -> None:
    module = _load_ci_gate()

    checks = {check.name: check for check in module._build_checks()}
    check = checks["structural-audit-ratchet"]

    assert check.tier == 1
    assert check.required is True
    assert check.needs_pytest is False
    assert check.needs_rust is False
    assert check.timeout == 60
    assert str(module.TOOLS / "structural_audit.py") in check.cmd
    assert "--check" in check.cmd


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

    def fake_guarded_completed_process(command, **kwargs):
        return module.harness_memory_guard.GuardedCompletedProcess(
            command,
            0,
            "rust ok\n",
            "",
            elapsed_s=0.1,
        )

    monkeypatch.setattr(module.compile_governor, "compile_slot", fake_compile_slot)
    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

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

    def fake_adaptive_memory_budget(prefix, environ=None, *, accounted_rss_kb=0):
        return module.memory_guard.AdaptiveMemoryBudget(
            max_process_rss_gb=2.0,
            max_total_rss_gb=3.0,
            max_global_rss_gb=4.0,
            reserve_gb=1.0,
            physical_gb=8.0,
            available_gb=6.0,
            source="test",
            accounted_rss_gb=accounted_rss_kb / (1024 * 1024),
        )

    monkeypatch.setattr(module, "LOG_ROOT", tmp_path)
    monkeypatch.setattr(
        module.memory_guard, "adaptive_memory_budget", fake_adaptive_memory_budget
    )
    monkeypatch.setattr(module.subprocess, "Popen", FakePopen)

    metadata = module.launch_background_gate(
        ["--tier", "2", "--background", "--parallel"]
    )

    assert metadata.pid == 4242
    assert metadata.log_path.parent == tmp_path
    assert metadata.metadata_path.parent == tmp_path
    assert "--background" not in popen_calls[0]["command"]
    command = popen_calls[0]["command"]
    assert command[:6] == [
        module.sys.executable,
        str(module.TOOLS / "guarded_exec.py"),
        "--prefix",
        "MOLT_CI_GATE",
        "--cwd",
        str(module.ROOT),
    ]
    assert command[6:9] == ["--", module.sys.executable, str(module.CI_GATE)]
    assert popen_calls[0]["start_new_session"] is True
    if os.name == "posix":
        assert callable(popen_calls[0]["preexec_fn"])
    else:
        assert "preexec_fn" not in popen_calls[0]
    assert metadata.metadata_path.exists()


def test_ci_gate_script_runs_directly_without_pythonpath() -> None:
    env = os.environ.copy()
    env.pop("PYTHONPATH", None)

    result = run_guarded_test_process(
        [sys.executable, "tools/ci_gate.py", "--tier", "1", "--dry-run", "--json"],
        prefix="MOLT_TEST_SUITE",
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)
    assert payload["summary"]["success"] is True


def test_ci_gate_help_reports_memory_guard_hard_cap() -> None:
    env = os.environ.copy()
    env.pop("PYTHONPATH", None)

    result = run_guarded_test_process(
        [sys.executable, "tools/ci_gate.py", "--help"],
        prefix="MOLT_TEST_SUITE",
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    assert "must be <112" in result.stdout
    assert "<4096" in result.stdout
    assert "must be <30" not in result.stdout
    assert "--no-memory-guard" not in result.stdout
