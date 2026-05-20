from __future__ import annotations

import importlib.util
import os
import sys
import threading
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "tests" / "molt_diff.py"


def _load_diff_module():
    spec = importlib.util.spec_from_file_location(
        "molt_diff_memory_guard_under_test", SCRIPT_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _configure_guard(
    module,
    monkeypatch,
    tmp_path: Path,
    *,
    process_gb: float,
    tree_gb: float,
    global_gb: float,
) -> object:
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(tmp_path / "diff"))
    monkeypatch.setenv("MOLT_DIFF_TMPDIR", str(tmp_path / "tmp"))
    monkeypatch.setenv("MOLT_DIFF_MAX_PROCESS_RSS_GB", str(process_gb))
    monkeypatch.setenv("MOLT_DIFF_MAX_TREE_RSS_GB", str(tree_gb))
    monkeypatch.setenv("MOLT_DIFF_GLOBAL_RSS_LIMIT_GB", str(global_gb))
    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD_POLL_SEC", "0.02")
    config = module._diff_memory_guard_config()
    module._prepare_memory_guard_run(config)
    return config


def test_run_subprocess_guard_kills_fast_allocator(tmp_path: Path, monkeypatch) -> None:
    module = _load_diff_module()
    _configure_guard(
        module,
        monkeypatch,
        tmp_path,
        process_gb=0.03,
        tree_gb=0.20,
        global_gb=0.30,
    )
    script = """
import time
chunks = []
for _ in range(16):
    chunks.append(bytearray(4 * 1024 * 1024))
    time.sleep(0.02)
time.sleep(10)
"""

    result = module._run_subprocess(
        [sys.executable, "-c", script],
        env=os.environ.copy(),
        timeout=10.0,
    )

    assert result.returncode == module._DIFF_MEMORY_GUARD_RETURN_CODE
    assert "molt_diff memory guard: RSS limit exceeded" in result.stderr


def test_run_subprocess_guard_accounts_recursive_children(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_diff_module()
    _configure_guard(
        module,
        monkeypatch,
        tmp_path,
        process_gb=0.08,
        tree_gb=0.08,
        global_gb=0.30,
    )
    child = "import time; buf = bytearray(36 * 1024 * 1024); time.sleep(10)"
    script = f"""
import subprocess
import sys
children = [
    subprocess.Popen([sys.executable, "-c", {child!r}])
    for _ in range(2)
]
try:
    for proc in children:
        proc.wait()
finally:
    for proc in children:
        proc.kill()
"""

    result = module._run_subprocess(
        [sys.executable, "-c", script],
        env=os.environ.copy(),
        timeout=10.0,
    )

    assert result.returncode == module._DIFF_MEMORY_GUARD_RETURN_CODE
    assert "scope=process_tree" in result.stderr


def test_global_monitor_kills_cumulative_parallel_trees(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_diff_module()
    config = _configure_guard(
        module,
        monkeypatch,
        tmp_path,
        process_gb=1.0,
        tree_gb=1.0,
        global_gb=0.06,
    )
    script = "import time; buf = bytearray(36 * 1024 * 1024); time.sleep(10)"
    results = []

    def run_one() -> None:
        results.append(
            module._run_subprocess(
                [sys.executable, "-c", script],
                env=os.environ.copy(),
                timeout=10.0,
            )
        )

    monitor = module._DiffGlobalMemoryMonitor(config)
    monitor.__enter__()
    try:
        threads = [threading.Thread(target=run_one) for _ in range(2)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(timeout=10.0)
    finally:
        monitor.__exit__(None, None, None)

    assert len(results) == 2
    assert any(
        result.returncode == module._DIFF_MEMORY_GUARD_RETURN_CODE for result in results
    )
    assert module._memory_guard_trip_message() is not None


def test_memory_guard_clamps_parallel_jobs(tmp_path: Path, monkeypatch) -> None:
    module = _load_diff_module()
    config = _configure_guard(
        module,
        monkeypatch,
        tmp_path,
        process_gb=0.02,
        tree_gb=0.03,
        global_gb=0.07,
    )

    assert module._constrain_jobs_for_memory_guard(16, config=config, log=False) == 2


def test_memory_guard_jsonl_rotation_preserves_recent_file(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_diff_module()
    path = tmp_path / "global_samples.jsonl"
    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD_MAX_SAMPLE_MB", "0.001")
    path.write_text("x" * 1024, encoding="utf-8")

    module._append_memory_guard_jsonl(path, {"event": "sample", "total_gb": 1.0})

    assert path.with_name("global_samples.jsonl.1").exists()
    payload = path.read_text(encoding="utf-8")
    assert '"event": "sample"' in payload
    assert '"total_gb": 1.0' in payload


def test_memory_guard_sample_interval_env_is_bounded(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD_SAMPLE_INTERVAL_SEC", "120")

    assert module._diff_memory_guard_sample_interval_sec() == 60.0


def test_diff_memory_guard_defaults_are_adaptive(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_DIFF_TOTAL_MEMORY_GB", "128")
    monkeypatch.setenv("MOLT_DIFF_MEM_AVAILABLE_GB", "96")
    monkeypatch.delenv("MOLT_DIFF_GLOBAL_RSS_LIMIT_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_GLOBAL_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_TREE_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_TOTAL_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_PROCESS_RSS_GB", raising=False)

    config = module._diff_memory_guard_config()

    assert config.global_gb == pytest.approx(85.6704)
    assert config.max_tree_gb == pytest.approx(51.40224)
    assert config.max_process_gb == pytest.approx(46.262016)
    assert config.child_rlimit_gb == pytest.approx(102.80448)


def test_diff_memory_guard_refresh_accounts_active_tree_rss(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_DIFF_TOTAL_MEMORY_GB", "128")
    monkeypatch.setenv("MOLT_DIFF_MEM_AVAILABLE_GB", "46")
    monkeypatch.delenv("MOLT_DIFF_GLOBAL_RSS_LIMIT_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_GLOBAL_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_TREE_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_TOTAL_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_PROCESS_RSS_GB", raising=False)

    config = module._diff_memory_guard_config(accounted_rss_kb=50 * 1024 * 1024)

    assert config.global_gb == pytest.approx(85.6704)
    assert config.max_tree_gb == pytest.approx(51.40224)
    assert config.max_process_gb == pytest.approx(46.262016)
    assert config.child_rlimit_gb == pytest.approx(102.80448)


def test_global_monitor_refreshes_limits_from_active_tree_rss(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_DIFF_TOTAL_MEMORY_GB", "128")
    monkeypatch.setenv("MOLT_DIFF_MEM_AVAILABLE_GB", "46")
    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD_ACTIVE_DIR", str(tmp_path / "active"))
    monkeypatch.setenv(
        "MOLT_DIFF_MEMORY_GUARD_TRIP_FILE", str(tmp_path / "tripped.json")
    )
    active_dir = tmp_path / "active"
    active_dir.mkdir()
    (active_dir / "worker-200.json").write_text(
        '{"pid": 200, "worker_pid": 100, "command": ["build"]}\n',
        encoding="utf-8",
    )
    gb = 1024 * 1024
    samples = {
        200: module.memory_guard.ProcessSample(200, 1, 1 * gb, "root", pgid=200),
        201: module.memory_guard.ProcessSample(201, 200, 25 * gb, "rustc-a", pgid=200),
        202: module.memory_guard.ProcessSample(202, 200, 24 * gb, "rustc-b", pgid=200),
    }
    killed: list[int] = []
    sample_payloads: list[dict[str, object]] = []
    monkeypatch.setattr(module.memory_guard, "sample_processes", lambda: samples)
    monkeypatch.setattr(module, "_pid_alive", lambda pid: True)
    monkeypatch.setattr(
        module, "_terminate_pid_tree", lambda pid, grace=1.0: killed.append(pid)
    )
    monkeypatch.setattr(
        module, "_record_memory_guard_sample", lambda payload: sample_payloads.append(payload)
    )

    module._DiffGlobalMemoryMonitor(module._diff_memory_guard_config())._sample_once()

    assert killed == []
    assert not (tmp_path / "tripped.json").exists()
    assert sample_payloads
    limits = sample_payloads[-1]["limits"]
    assert isinstance(limits, dict)
    assert limits["global_gb"] == pytest.approx(85.6704)


def test_diff_scheduler_uses_memory_scaled_job_budget(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_DIFF_TOTAL_MEMORY_GB", "128")
    monkeypatch.setenv("MOLT_DIFF_MEM_AVAILABLE_GB", "96")
    monkeypatch.delenv("MOLT_DIFF_MEM_PER_JOB_GB", raising=False)
    monkeypatch.setattr(module.os, "cpu_count", lambda: 12)

    config = module._diff_memory_guard_config()

    assert module._memory_guard_scheduler_per_job_gb(config) == pytest.approx(7.1392)
    assert module._memory_guard_max_jobs(config) == 12
    assert module._default_jobs() == 12


def test_diff_memory_guard_inherits_shared_parent_overrides(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.delenv("MOLT_DIFF_MAX_PROCESS_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_TOTAL_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_GLOBAL_RSS_LIMIT_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_GLOBAL_RSS_GB", raising=False)
    monkeypatch.setenv("MOLT_MAX_PROCESS_RSS_GB", "7")
    monkeypatch.setenv("MOLT_MAX_TOTAL_RSS_GB", "8")
    monkeypatch.setenv("MOLT_MAX_GLOBAL_RSS_GB", "9")
    monkeypatch.setenv("MOLT_CHILD_RLIMIT_GB", "10")

    config = module._diff_memory_guard_config()

    assert config.max_process_gb == pytest.approx(7)
    assert config.max_tree_gb == pytest.approx(8)
    assert config.global_gb == pytest.approx(9)
    assert config.child_rlimit_gb == pytest.approx(10)


def test_diff_memory_guard_family_overrides_parent_controls(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_MAX_PROCESS_RSS_GB", "7")
    monkeypatch.setenv("MOLT_MAX_TOTAL_RSS_GB", "8")
    monkeypatch.setenv("MOLT_MAX_GLOBAL_RSS_GB", "9")
    monkeypatch.setenv("MOLT_CHILD_RLIMIT_GB", "10")
    monkeypatch.setenv("MOLT_DIFF_MAX_PROCESS_RSS_GB", "3")
    monkeypatch.setenv("MOLT_DIFF_MAX_TREE_RSS_GB", "4")
    monkeypatch.setenv("MOLT_DIFF_GLOBAL_RSS_LIMIT_GB", "5")
    monkeypatch.setenv("MOLT_DIFF_CHILD_RLIMIT_GB", "6")

    config = module._diff_memory_guard_config()

    assert config.max_process_gb == pytest.approx(3)
    assert config.max_tree_gb == pytest.approx(4)
    assert config.global_gb == pytest.approx(5)
    assert config.child_rlimit_gb == pytest.approx(6)


def test_diff_memory_guard_global_disable_can_be_overridden(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_MEMORY_GUARD", "0")
    monkeypatch.delenv("MOLT_DIFF_MEMORY_GUARD", raising=False)

    assert module._diff_memory_guard_enabled() is False

    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD", "1")
    assert module._diff_memory_guard_enabled() is True


def test_diff_rlimit_defaults_to_adaptive_child_budget(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_DIFF_TOTAL_MEMORY_GB", "128")
    monkeypatch.setenv("MOLT_DIFF_MEM_AVAILABLE_GB", "96")
    monkeypatch.delenv("MOLT_DIFF_RLIMIT_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_RLIMIT_MB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_CHILD_RLIMIT_GB", raising=False)

    config = module._diff_memory_guard_config()

    assert config.child_rlimit_gb == pytest.approx(config.max_tree_gb * 2.0)
    assert module._memory_limit_bytes() == config.child_rlimit_kb * 1024
    assert module._memory_limit_bytes() > config.max_process_kb * 1024


def test_diff_measure_rss_is_enabled_by_default(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.delenv("MOLT_DIFF_MEASURE_RSS", raising=False)

    assert module._diff_measure_rss() is True

    monkeypatch.setenv("MOLT_DIFF_MEASURE_RSS", "0")
    assert module._diff_measure_rss() is False


def test_popen_group_kwargs_applies_child_rlimit(monkeypatch) -> None:
    module = _load_diff_module()
    if module.os.name == "nt":
        return
    applied: list[int] = []
    monkeypatch.setenv("MOLT_DIFF_MAX_PROCESS_RSS_GB", "0.5")
    monkeypatch.setenv("MOLT_DIFF_MAX_TREE_RSS_GB", "1.0")
    monkeypatch.setenv("MOLT_DIFF_GLOBAL_RSS_LIMIT_GB", "2.0")
    monkeypatch.setenv("MOLT_DIFF_CHILD_RLIMIT_GB", "0.5")
    monkeypatch.setattr(
        module.memory_guard,
        "_apply_child_resource_limit",
        lambda limit_kb: applied.append(limit_kb),
    )

    kwargs = module._popen_group_kwargs()

    assert kwargs["start_new_session"] is True
    assert callable(kwargs["preexec_fn"])
    kwargs["preexec_fn"]()
    assert applied == [512 * 1024]


def test_popen_group_kwargs_can_disable_child_rlimit(monkeypatch) -> None:
    module = _load_diff_module()
    if module.os.name == "nt":
        return
    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD", "1")
    monkeypatch.setenv("MOLT_DIFF_CHILD_RLIMIT_GB", "0")

    kwargs = module._popen_group_kwargs()

    assert kwargs == {"start_new_session": True}


def test_popen_group_kwargs_omits_child_rlimit_when_guard_disabled(monkeypatch) -> None:
    module = _load_diff_module()
    if module.os.name == "nt":
        return
    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD", "0")

    kwargs = module._popen_group_kwargs()

    assert kwargs == {"start_new_session": True}
