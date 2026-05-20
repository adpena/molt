from __future__ import annotations

import subprocess
import sys
from pathlib import Path

from tools import harness_memory_guard


def test_limits_from_env_prefers_harness_prefix(monkeypatch) -> None:
    monkeypatch.setenv("MOLT_MEMORY_GUARD", "0")
    monkeypatch.setenv("MOLT_BENCH_MEMORY_GUARD", "1")
    monkeypatch.setenv("MOLT_BENCH_MAX_PROCESS_RSS_GB", "3")
    monkeypatch.setenv("MOLT_BENCH_MAX_TOTAL_RSS_GB", "4")
    monkeypatch.setenv("MOLT_BENCH_GLOBAL_RSS_LIMIT_GB", "5")
    monkeypatch.setenv("MOLT_BENCH_MEMORY_GUARD_POLL_SEC", "0.05")

    limits = harness_memory_guard.limits_from_env("MOLT_BENCH")

    assert limits.enabled is True
    assert limits.max_process_rss_gb == 3
    assert limits.max_total_rss_gb == 4
    assert limits.max_global_rss_gb == 5
    assert limits.poll_interval == 0.05
    assert limits.max_process_rss_kb == 3 * 1024 * 1024
    assert limits.max_total_rss_kb == 4 * 1024 * 1024
    assert limits.max_global_rss_kb == 5 * 1024 * 1024


def test_timeout_from_env_prefers_harness_prefix(monkeypatch) -> None:
    monkeypatch.setenv("MOLT_TEST_PROCESS_TIMEOUT_SEC", "99")
    monkeypatch.setenv("MOLT_CLI_TEST_TIMEOUT_SEC", "12.5")

    assert (
        harness_memory_guard.timeout_from_env(
            "MOLT_CLI_TEST",
            explicit=None,
            default=300,
        )
        == 12.5
    )
    assert (
        harness_memory_guard.timeout_from_env(
            "MOLT_NATIVE_TEST",
            explicit=None,
            default=300,
        )
        == 99
    )
    assert (
        harness_memory_guard.timeout_from_env(
            "MOLT_CLI_TEST",
            explicit=7,
            default=300,
        )
        == 7
    )


def test_limits_from_env_merges_parent_guard_controls(monkeypatch) -> None:
    monkeypatch.setenv("MOLT_MEMORY_GUARD", "0")
    monkeypatch.setenv("MOLT_MAX_PROCESS_RSS_GB", "6")

    limits = harness_memory_guard.limits_from_env(
        "MOLT_BENCH",
        {"PATH": "/usr/bin", "MOLT_BENCH_MEMORY_GUARD": "1"},
    )

    assert limits.enabled is True
    assert limits.max_process_rss_gb == 6


def test_limits_from_env_uses_fast_default_poll(monkeypatch) -> None:
    monkeypatch.delenv("MOLT_TEST_MEMORY_GUARD_POLL_SEC", raising=False)
    monkeypatch.delenv("MOLT_MEMORY_GUARD_POLL_SEC", raising=False)

    limits = harness_memory_guard.limits_from_env("MOLT_TEST", {})

    assert limits.poll_interval == harness_memory_guard.DEFAULT_POLL_INTERVAL_SEC
    assert limits.poll_interval == 0.10


def test_timeout_from_env_merges_parent_timeout_controls(monkeypatch) -> None:
    monkeypatch.setenv("MOLT_TEST_PROCESS_TIMEOUT_SEC", "42")

    assert (
        harness_memory_guard.timeout_from_env(
            "MOLT_BENCH",
            {"PATH": "/usr/bin"},
            default=300,
        )
        == 42
    )


def test_timeout_from_env_zero_disables_default(monkeypatch) -> None:
    monkeypatch.setenv("MOLT_CLI_TEST_TIMEOUT_SEC", "0")

    assert (
        harness_memory_guard.timeout_from_env(
            "MOLT_CLI_TEST",
            explicit=None,
            default=300,
        )
        is None
    )


def test_guarded_completed_process_uses_process_tree_guard(monkeypatch) -> None:
    calls: list[dict[str, object]] = []

    def fake_run_guarded(command, **kwargs):
        calls.append({"command": command, **kwargs})
        return harness_memory_guard.memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="ok\n",
            stderr="",
        )

    monkeypatch.setattr(
        harness_memory_guard.memory_guard, "run_guarded", fake_run_guarded
    )
    limits = harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=2,
        max_total_rss_gb=3,
        max_global_rss_gb=4,
        poll_interval=0.1,
    )

    result = harness_memory_guard.guarded_completed_process(
        [sys.executable, "-c", "print('ok')"],
        prefix="MOLT_TEST",
        limits=limits,
    )

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    call = calls[0]
    assert call["max_rss_kb"] == 2 * 1024 * 1024
    assert call["max_total_rss_kb"] == 3 * 1024 * 1024
    assert call["child_rlimit_kb"] == 2 * 1024 * 1024


def test_guarded_completed_process_preserves_signal_diagnostic(monkeypatch) -> None:
    def fake_run_guarded(command, **kwargs):
        return harness_memory_guard.memory_guard.GuardResult(
            returncode=-9,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
        )

    monkeypatch.setattr(
        harness_memory_guard.memory_guard, "run_guarded", fake_run_guarded
    )
    limits = harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=2,
        max_total_rss_gb=3,
        max_global_rss_gb=4,
        poll_interval=0.1,
    )

    result = harness_memory_guard.guarded_completed_process(
        [sys.executable, "-c", "pass"],
        prefix="MOLT_TEST",
        limits=limits,
    )

    assert result.returncode == -9
    assert "memory_guard: command exited with SIGKILL status (-9)" in result.stderr


def test_guarded_completed_process_can_be_disabled(monkeypatch) -> None:
    monkeypatch.setenv("MOLT_TEST_MEMORY_GUARD", "0")

    result = harness_memory_guard.guarded_completed_process(
        [sys.executable, "-c", "print('plain')"],
        prefix="MOLT_TEST",
    )

    assert isinstance(result, subprocess.CompletedProcess)
    assert result.returncode == 0
    assert result.stdout == "plain\n"


def test_batch_process_group_kwargs_applies_child_rlimit(monkeypatch) -> None:
    if harness_memory_guard.os.name != "posix":
        return
    applied: list[int] = []
    monkeypatch.setattr(
        harness_memory_guard.memory_guard,
        "_apply_child_resource_limit",
        lambda limit_kb: applied.append(limit_kb),
    )
    limits = harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=2,
        max_total_rss_gb=3,
        max_global_rss_gb=4,
        poll_interval=0.01,
    )

    kwargs = harness_memory_guard.batch_process_group_kwargs(limits)

    assert kwargs["start_new_session"] is True
    preexec = kwargs["preexec_fn"]
    assert callable(preexec)
    preexec()
    assert applied == [2 * 1024 * 1024]


def test_repo_process_sentinel_records_and_terminates_violation(
    monkeypatch, tmp_path: Path
) -> None:
    violation = harness_memory_guard.process_sentinel.SentinelViolation(
        pgid=12345,
        reason="global_rss",
        total_rss_kb=6 * 1024 * 1024,
        peak_pid=12346,
        peak_rss_kb=3 * 1024 * 1024,
        pids=(12345, 12346),
        command="molt-backend --daemon",
    )
    monkeypatch.setattr(
        harness_memory_guard.process_sentinel,
        "process_groups",
        lambda *args, **kwargs: ["group"],
    )
    monkeypatch.setattr(
        harness_memory_guard.process_sentinel,
        "find_violations",
        lambda *args, **kwargs: [violation],
    )
    terminated: list[int] = []
    monkeypatch.setattr(
        harness_memory_guard.process_sentinel,
        "terminate_group",
        lambda pgid, *, grace: terminated.append(pgid),
    )
    limits = harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=2,
        max_total_rss_gb=3,
        max_global_rss_gb=4,
        poll_interval=0.01,
    )

    sentinel = harness_memory_guard.repo_process_sentinel(
        repo_root=tmp_path,
        artifact_root=tmp_path,
        label="unit",
        limits=limits,
    )
    sentinel.scan_once()

    assert sentinel.tripped is True
    assert terminated == [12345]
    assert "repo_process_guard_tripped" in sentinel.events_path.read_text(
        encoding="utf-8"
    )


def test_repo_process_sentinel_drains_only_groups_started_after_baseline(
    monkeypatch,
    tmp_path: Path,
) -> None:
    baseline_group = harness_memory_guard.process_sentinel.ProcessGroup(
        pgid=111,
        matched=True,
        samples=(
            harness_memory_guard.memory_guard.ProcessSample(
                pid=111,
                ppid=1,
                pgid=111,
                rss_kb=100,
                command="molt-backend --daemon --socket baseline.sock",
            ),
        ),
    )
    new_group = harness_memory_guard.process_sentinel.ProcessGroup(
        pgid=222,
        matched=True,
        samples=(
            harness_memory_guard.memory_guard.ProcessSample(
                pid=222,
                ppid=1,
                pgid=222,
                rss_kb=200,
                command="molt-backend --daemon --socket pytest.sock",
            ),
        ),
    )
    scan_count = 0

    def fake_current_groups(self):  # type: ignore[no-untyped-def]
        nonlocal scan_count
        scan_count += 1
        if scan_count == 1:
            return [baseline_group, new_group]
        return [baseline_group]

    monkeypatch.setattr(
        harness_memory_guard.RepoProcessMemorySentinel,
        "_current_groups",
        fake_current_groups,
    )
    terminated: list[int] = []
    monkeypatch.setattr(
        harness_memory_guard.process_sentinel,
        "terminate_group",
        lambda pgid, *, grace: terminated.append(pgid),
    )
    limits = harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=2,
        max_total_rss_gb=3,
        max_global_rss_gb=4,
        poll_interval=0.001,
    )
    sentinel = harness_memory_guard.repo_process_sentinel(
        repo_root=tmp_path,
        artifact_root=tmp_path,
        label="unit-drain",
        limits=limits,
        drain_until_clean_sec=0.001,
        drain_max_runtime_sec=0.1,
    )
    sentinel._baseline_pgids = {111}

    drained = sentinel.drain_new_processes()

    assert drained == 1
    assert terminated == [222]
    events = sentinel.events_path.read_text(encoding="utf-8")
    assert "repo_process_guard_drained" in events
    assert "drain_on_exit" in events
