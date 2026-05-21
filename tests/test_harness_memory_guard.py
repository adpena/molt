from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

from tools import harness_memory_guard


def test_limits_from_env_prefers_harness_prefix(monkeypatch) -> None:
    monkeypatch.setenv("MOLT_MEMORY_GUARD", "0")
    monkeypatch.setenv("MOLT_BENCH_MEMORY_GUARD", "1")
    monkeypatch.setenv("MOLT_BENCH_MAX_PROCESS_RSS_GB", "3")
    monkeypatch.setenv("MOLT_BENCH_MAX_TOTAL_RSS_GB", "4")
    monkeypatch.setenv("MOLT_BENCH_GLOBAL_RSS_LIMIT_GB", "7")
    monkeypatch.setenv("MOLT_BENCH_CHILD_RLIMIT_GB", "6")
    monkeypatch.setenv("MOLT_BENCH_MEMORY_GUARD_POLL_SEC", "0.05")

    limits = harness_memory_guard.limits_from_env("MOLT_BENCH")

    assert limits.enabled is True
    assert limits.max_process_rss_gb == 3
    assert limits.max_total_rss_gb == 4
    assert limits.max_global_rss_gb == 7
    assert limits.child_rlimit_gb == 3
    assert limits.poll_interval == 0.05
    assert limits.dynamic_process_rss is False
    assert limits.dynamic_total_rss is False
    assert limits.dynamic_global_rss is False
    assert limits.max_process_rss_kb == 3 * 1024 * 1024
    assert limits.max_total_rss_kb == 4 * 1024 * 1024
    assert limits.max_global_rss_kb == 7 * 1024 * 1024
    assert limits.child_rlimit_kb == 3 * 1024 * 1024


def test_enabled_from_env_matches_family_override_semantics(monkeypatch) -> None:
    monkeypatch.setenv("MOLT_MEMORY_GUARD", "0")
    monkeypatch.delenv("MOLT_BENCH_MEMORY_GUARD", raising=False)

    assert harness_memory_guard.enabled_from_env("MOLT_BENCH") is False

    monkeypatch.setenv("MOLT_BENCH_MEMORY_GUARD", "1")
    assert harness_memory_guard.enabled_from_env("MOLT_BENCH") is True


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


def test_limits_from_env_uses_adaptive_defaults(monkeypatch) -> None:
    monkeypatch.delenv("MOLT_MEMORY_GUARD", raising=False)
    monkeypatch.delenv("MOLT_BENCH_MEMORY_GUARD", raising=False)
    monkeypatch.delenv("MOLT_BENCH_MAX_PROCESS_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_BENCH_MAX_TOTAL_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_BENCH_MAX_GLOBAL_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_BENCH_GLOBAL_RSS_LIMIT_GB", raising=False)
    monkeypatch.delenv("MOLT_BENCH_MEMORY_GUARD_POLL_SEC", raising=False)
    monkeypatch.delenv("MOLT_MAX_PROCESS_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_MAX_TOTAL_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_MAX_GLOBAL_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_GLOBAL_RSS_LIMIT_GB", raising=False)
    monkeypatch.delenv("MOLT_MEMORY_GUARD_POLL_SEC", raising=False)

    limits = harness_memory_guard.limits_from_env(
        "MOLT_BENCH",
        {
            "PATH": "/usr/bin",
            "MOLT_BENCH_TOTAL_MEMORY_GB": "128",
            "MOLT_BENCH_MEM_AVAILABLE_GB": "96",
        },
    )

    assert limits.enabled is True
    assert limits.max_process_rss_gb == pytest.approx(46.262016)
    assert limits.max_total_rss_gb == pytest.approx(51.40224)
    assert limits.max_global_rss_gb == pytest.approx(85.6704)
    assert limits.child_rlimit_gb == pytest.approx(46.262016)
    assert limits.poll_interval == harness_memory_guard.DEFAULT_POLL_INTERVAL_SEC
    assert limits.dynamic_process_rss is True
    assert limits.dynamic_total_rss is True
    assert limits.dynamic_global_rss is True
    assert limits.dynamic_child_rlimit is True


def test_limits_from_env_merges_parent_guard_controls(monkeypatch) -> None:
    monkeypatch.setenv("MOLT_MEMORY_GUARD", "0")
    monkeypatch.setenv("MOLT_MAX_PROCESS_RSS_GB", "6")

    limits = harness_memory_guard.limits_from_env(
        "MOLT_BENCH",
        {"PATH": "/usr/bin", "MOLT_BENCH_MEMORY_GUARD": "1"},
    )

    assert limits.enabled is True
    assert limits.max_process_rss_gb == 6
    assert limits.dynamic_process_rss is False
    assert limits.dynamic_total_rss is True


def test_limits_from_env_canonicalizes_implausible_overrides(monkeypatch) -> None:
    monkeypatch.setenv("MOLT_CONFORMANCE_MAX_PROCESS_RSS_GB", "4200")
    monkeypatch.setenv("MOLT_CONFORMANCE_MAX_TREE_RSS_GB", "4500")
    monkeypatch.setenv("MOLT_CONFORMANCE_GLOBAL_RSS_LIMIT_GB", "5000")
    monkeypatch.setenv("MOLT_CONFORMANCE_CHILD_RLIMIT_GB", "5000")
    env = {
        "PATH": "/usr/bin",
        "MOLT_CONFORMANCE_TOTAL_MEMORY_GB": "128",
        "MOLT_CONFORMANCE_MEM_AVAILABLE_GB": "96",
    }

    limits = harness_memory_guard.limits_from_env("MOLT_CONFORMANCE", env)

    assert limits.max_process_rss_gb == pytest.approx(
        85.6704
    )
    assert limits.max_total_rss_gb == pytest.approx(
        85.6704
    )
    assert limits.max_global_rss_gb == pytest.approx(85.6704)
    assert limits.child_rlimit_gb == pytest.approx(85.6704)


def test_current_memory_limits_refreshes_unset_adaptive_caps(monkeypatch) -> None:
    calls: list[tuple[str, int]] = []

    def fake_budget(prefix, environ=None, *, accounted_rss_kb=0):
        calls.append((prefix, accounted_rss_kb))
        return harness_memory_guard.memory_guard.AdaptiveMemoryBudget(
            max_process_rss_gb=7,
            max_total_rss_gb=8,
            max_global_rss_gb=9,
            reserve_gb=1,
            physical_gb=16,
            available_gb=12,
            source="test",
            accounted_rss_gb=accounted_rss_kb / (1024 * 1024),
        )

    monkeypatch.setattr(
        harness_memory_guard.memory_guard,
        "adaptive_memory_budget",
        fake_budget,
    )
    limits = harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=2,
        max_total_rss_gb=3,
        max_global_rss_gb=4,
        poll_interval=0.1,
        adaptive_prefix="MOLT_BENCH",
        dynamic_process_rss=True,
        dynamic_total_rss=False,
        dynamic_global_rss=True,
    )

    current = limits.current_memory_limits(accounted_rss_kb=42)

    assert calls == [("MOLT_BENCH", 42)]
    assert current.max_process_rss_gb == pytest.approx(7)
    assert current.max_total_rss_gb == pytest.approx(3)
    assert current.max_global_rss_gb == pytest.approx(9)


def test_current_child_rlimit_refreshes_dynamic_adaptive_budget(monkeypatch) -> None:
    calls: list[tuple[str, int]] = []

    def fake_budget(prefix, environ=None, *, accounted_rss_kb=0):
        calls.append((prefix, accounted_rss_kb))
        return harness_memory_guard.memory_guard.AdaptiveMemoryBudget(
            max_process_rss_gb=11,
            max_total_rss_gb=13,
            max_global_rss_gb=17,
            reserve_gb=1,
            physical_gb=32,
            available_gb=24,
            source="test",
            accounted_rss_gb=accounted_rss_kb / (1024 * 1024),
        )

    monkeypatch.setattr(
        harness_memory_guard.memory_guard,
        "adaptive_memory_budget",
        fake_budget,
    )
    limits = harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=2,
        max_total_rss_gb=3,
        max_global_rss_gb=4,
        poll_interval=0.1,
        adaptive_prefix="MOLT_CONFORMANCE",
        dynamic_process_rss=True,
        dynamic_total_rss=True,
        dynamic_global_rss=True,
        dynamic_child_rlimit=True,
    )

    child_rlimit = limits.current_child_rlimit_kb(
        {"PATH": "/usr/bin"},
        accounted_rss_kb=99,
    )

    assert calls == [("MOLT_CONFORMANCE", 99)]
    assert child_rlimit == 11 * 1024 * 1024


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


def test_canonical_harness_env_installs_repo_local_defaults(tmp_path: Path) -> None:
    env = harness_memory_guard.canonical_harness_env(
        {"PATH": "/usr/bin"},
        repo_root=tmp_path,
    )

    assert env["MOLT_EXT_ROOT"] == str(tmp_path.resolve())
    assert env["CARGO_TARGET_DIR"] == str(tmp_path / "target")
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == env["CARGO_TARGET_DIR"]
    assert env["MOLT_CACHE"] == str(tmp_path / ".molt_cache")
    assert env["MOLT_DIFF_ROOT"] == str(tmp_path / "tmp" / "diff")
    assert env["MOLT_DIFF_TMPDIR"] == str(tmp_path / "tmp")
    assert env["UV_CACHE_DIR"] == str(tmp_path / ".uv-cache")
    assert env["TMPDIR"] == str(tmp_path / "tmp")


def test_execution_context_owns_env_limits_and_batch_kwargs(
    tmp_path: Path,
    monkeypatch,
) -> None:
    if harness_memory_guard.os.name != "posix":
        return
    monkeypatch.setenv("MOLT_BENCH_MAX_PROCESS_RSS_GB", "2")
    monkeypatch.setenv("MOLT_BENCH_MAX_TOTAL_RSS_GB", "3")
    monkeypatch.setenv("MOLT_BENCH_MAX_GLOBAL_RSS_GB", "4")
    context = harness_memory_guard.HarnessExecutionContext.from_env(
        "MOLT_BENCH",
        {"PATH": "/usr/bin"},
        repo_root=tmp_path,
    )

    assert context.prefix == "MOLT_BENCH"
    assert context.env["TMPDIR"] == str(tmp_path / "tmp")
    assert context.limits.enabled is True
    kwargs = context.process_group_kwargs()
    assert kwargs["start_new_session"] is True
    assert callable(kwargs["preexec_fn"])


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
    assert call["dynamic_process_rss"] is False
    assert call["dynamic_total_rss"] is False


def test_guarded_completed_process_starts_default_repo_sentinel(monkeypatch) -> None:
    run_calls: list[dict[str, object]] = []
    sentinel_calls: list[dict[str, object]] = []

    class FakeSentinel:
        def __enter__(self):
            sentinel_calls.append({"event": "enter"})
            return self

        def __exit__(self, exc_type, exc, tb) -> None:
            sentinel_calls.append({"event": "exit"})

    def fake_sentinel(**kwargs):
        sentinel_calls.append(kwargs)
        return FakeSentinel()

    def fake_run_guarded(command, **kwargs):
        run_calls.append({"command": command, **kwargs})
        return harness_memory_guard.memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="ok\n",
            stderr="",
        )

    monkeypatch.setattr(
        harness_memory_guard,
        "repo_process_sentinel",
        fake_sentinel,
    )
    monkeypatch.setattr(harness_memory_guard, "_sentinel_active", lambda: False)
    monkeypatch.setattr(
        harness_memory_guard.memory_guard,
        "run_guarded",
        fake_run_guarded,
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
        prefix="MOLT_CONFORMANCE",
        limits=limits,
    )

    assert result.returncode == 0
    assert run_calls
    assert sentinel_calls[0]["label"] == "molt_conformance_command"
    assert sentinel_calls[0]["limits"] is limits
    assert sentinel_calls[1] == {"event": "enter"}
    assert sentinel_calls[2] == {"event": "exit"}


def test_guarded_completed_process_reuses_active_repo_sentinel(monkeypatch) -> None:
    sentinel_calls: list[dict[str, object]] = []

    def fake_run_guarded(command, **kwargs):
        return harness_memory_guard.memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="ok\n",
            stderr="",
        )

    monkeypatch.setattr(
        harness_memory_guard,
        "repo_process_sentinel",
        lambda **kwargs: sentinel_calls.append(kwargs),
    )
    monkeypatch.setattr(harness_memory_guard, "_sentinel_active", lambda: True)
    monkeypatch.setattr(
        harness_memory_guard.memory_guard,
        "run_guarded",
        fake_run_guarded,
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
        prefix="MOLT_CONFORMANCE",
        limits=limits,
    )

    assert result.returncode == 0
    assert sentinel_calls == []


def test_guarded_completed_process_refreshes_dynamic_child_rlimit(monkeypatch) -> None:
    calls: list[dict[str, object]] = []

    def fake_budget(prefix, environ=None, *, accounted_rss_kb=0):
        return harness_memory_guard.memory_guard.AdaptiveMemoryBudget(
            max_process_rss_gb=5,
            max_total_rss_gb=7,
            max_global_rss_gb=9,
            reserve_gb=1,
            physical_gb=16,
            available_gb=12,
            source="test",
            accounted_rss_gb=accounted_rss_kb / (1024 * 1024),
        )

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
        harness_memory_guard.memory_guard,
        "adaptive_memory_budget",
        fake_budget,
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
        adaptive_prefix="MOLT_TEST",
        dynamic_process_rss=True,
        dynamic_total_rss=True,
        dynamic_global_rss=True,
        dynamic_child_rlimit=True,
    )

    result = harness_memory_guard.guarded_completed_process(
        [sys.executable, "-c", "print('ok')"],
        prefix="MOLT_TEST",
        limits=limits,
    )

    assert result.returncode == 0
    assert calls[0]["child_rlimit_kb"] == 5 * 1024 * 1024


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


def test_batch_process_group_kwargs_can_disable_child_rlimit() -> None:
    limits = harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=2,
        max_total_rss_gb=3,
        max_global_rss_gb=4,
        poll_interval=0.01,
        child_rlimit_gb=0,
    )

    kwargs = harness_memory_guard.batch_process_group_kwargs(limits)

    assert kwargs == {"start_new_session": True}


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
        lambda *args, **kwargs: [
            harness_memory_guard.process_sentinel.ProcessGroup(
                pgid=12345,
                matched=True,
                samples=(
                    harness_memory_guard.memory_guard.ProcessSample(
                        pid=12346,
                        ppid=1,
                        pgid=12345,
                        rss_kb=3 * 1024 * 1024,
                        command="molt-backend --daemon",
                    ),
                ),
            )
        ],
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
    events = sentinel.events_path.read_text(encoding="utf-8")
    assert "repo_process_guard_tripped" in events
    assert "limits" in events


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


def test_guarded_harness_scope_standardizes_repo_sentinel(monkeypatch, tmp_path: Path):
    calls: list[dict[str, object]] = []

    class FakeSentinel:
        def __enter__(self):
            calls.append({"event": "enter"})
            return self

        def __exit__(self, exc_type, exc, tb) -> None:
            calls.append({"event": "exit"})

    def fake_repo_process_sentinel(**kwargs):
        calls.append(kwargs)
        return FakeSentinel()

    monkeypatch.setattr(
        harness_memory_guard,
        "repo_process_sentinel",
        fake_repo_process_sentinel,
    )
    limits = harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=2,
        max_total_rss_gb=3,
        max_global_rss_gb=4,
        poll_interval=0.01,
    )

    with harness_memory_guard.guarded_harness_scope(
        prefix="MOLT_CONFORMANCE",
        repo_root=tmp_path,
        artifact_root=tmp_path / "artifacts",
        label="unit-scope",
        env={"PATH": "/usr/bin"},
        limits=limits,
    ) as scope:
        assert scope.limits is limits
        assert scope.memory_guard["enabled"] is True

    assert calls[0]["repo_root"] == tmp_path
    assert calls[0]["artifact_root"] == tmp_path / "artifacts"
    assert calls[0]["label"] == "unit-scope"
    assert calls[0]["limits"] is limits
    assert calls[1] == {"event": "enter"}
    assert calls[2] == {"event": "exit"}
