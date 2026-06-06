from __future__ import annotations

import contextlib
import json
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
    assert limits.child_rlimit_gb == 6
    assert limits.poll_interval == 0.05
    assert limits.dynamic_process_rss is False
    assert limits.dynamic_total_rss is False
    assert limits.dynamic_global_rss is False
    assert limits.max_process_rss_kb == 3 * 1024 * 1024
    assert limits.max_total_rss_kb == 4 * 1024 * 1024
    assert limits.max_global_rss_kb == 7 * 1024 * 1024
    assert limits.child_rlimit_kb == 6 * 1024 * 1024


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

    assert limits.max_process_rss_gb == pytest.approx(85.6704)
    assert limits.max_total_rss_gb == pytest.approx(85.6704)
    assert limits.max_global_rss_gb == pytest.approx(85.6704)
    assert limits.child_rlimit_gb == pytest.approx(
        harness_memory_guard.HARD_CHILD_RLIMIT_GB
    )


def test_limits_from_env_honors_explicit_child_rlimit_above_rss_budget(
    monkeypatch,
) -> None:
    monkeypatch.setenv("MOLT_WASM_TEST_MAX_PROCESS_RSS_GB", "3")
    monkeypatch.setenv("MOLT_WASM_TEST_MAX_TOTAL_RSS_GB", "4")
    monkeypatch.setenv("MOLT_WASM_TEST_MAX_GLOBAL_RSS_GB", "5")
    monkeypatch.setenv("MOLT_WASM_TEST_CHILD_RLIMIT_GB", "16")

    limits = harness_memory_guard.limits_from_env("MOLT_WASM_TEST")

    assert limits.max_process_rss_gb == 3
    assert limits.max_total_rss_gb == 4
    assert limits.max_global_rss_gb == 5
    assert limits.child_rlimit_gb == 16
    assert limits.current_child_rlimit_kb({}) == 16 * 1024 * 1024
    assert limits.dynamic_child_rlimit is False


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
    assert env["MOLT_SESSION_ID"].startswith("guard-")


def test_canonical_harness_env_preserves_caller_session(tmp_path: Path) -> None:
    env = harness_memory_guard.canonical_harness_env(
        {"MOLT_SESSION_ID": "caller-session"},
        repo_root=tmp_path,
    )

    assert env["MOLT_SESSION_ID"] == "caller-session"


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
    assert call["progress_label"] is None
    assert call["keepalive_interval"] is None


def test_guarded_completed_process_writes_command_profile(
    monkeypatch,
    tmp_path: Path,
) -> None:
    profile_log = tmp_path / "commands.jsonl"

    def fake_run_guarded(command, **kwargs):
        del command, kwargs
        return harness_memory_guard.memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=harness_memory_guard.memory_guard.RssViolation(
                pid=123,
                rss_kb=64 * 1024,
                command="python3 unit.py",
            ),
            peak_total=harness_memory_guard.memory_guard.RssViolation(
                pid=123,
                rss_kb=96 * 1024,
                command="python3 unit.py",
                scope="process_tree",
            ),
            stdout="ok\n",
            stderr="",
            elapsed_s=0.25,
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
        env={"MOLT_GUARD_PROFILE_LOG": str(profile_log), "MOLT_SESSION_ID": "unit"},
        limits=limits,
    )

    assert result.returncode == 0
    payload = [
        json.loads(line)
        for line in profile_log.read_text(encoding="utf-8").splitlines()
    ]
    assert len(payload) == 1
    event = payload[0]
    assert event["event"] == "guarded_command_profile"
    assert event["prefix"] == "MOLT_TEST"
    assert event["session_id"] == "unit"
    assert event["status"] == "pass"
    assert event["elapsed_s"] == 0.25
    assert event["memory_guard_enabled"] is True
    assert event["peak"]["rss_kb"] == 64 * 1024
    assert event["peak_total"]["scope"] == "process_tree"


def test_guarded_completed_process_rotates_command_profile(
    monkeypatch,
    tmp_path: Path,
) -> None:
    profile_log = tmp_path / "commands.jsonl"
    profile_log.write_text("x" * 2048, encoding="utf-8")

    def fake_run_guarded(command, **kwargs):
        del command, kwargs
        return harness_memory_guard.memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="ok\n",
            stderr="",
            elapsed_s=0.1,
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
        env={
            "MOLT_GUARD_PROFILE_LOG": str(profile_log),
            "MOLT_GUARD_PROFILE_MAX_MB": "0.001",
        },
        limits=limits,
    )

    assert result.returncode == 0
    assert (tmp_path / "commands.jsonl.1").exists()
    event = json.loads(profile_log.read_text(encoding="utf-8"))
    assert event["event"] == "guarded_command_profile"


def test_guarded_completed_process_streamed_commands_emit_keepalive(
    monkeypatch,
) -> None:
    calls: list[dict[str, object]] = []

    def fake_run_guarded(command, **kwargs):
        calls.append({"command": command, **kwargs})
        return harness_memory_guard.memory_guard.GuardResult(
            returncode=0,
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
        [sys.executable, "-c", "print('ok')"],
        prefix="MOLT_WASM_TEST",
        env={"MOLT_WASM_TEST_KEEPALIVE_SEC": "3"},
        limits=limits,
        capture_output=False,
    )

    assert result.returncode == 0
    assert calls[0]["progress_label"] == (
        "memory_guard: MOLT_WASM_TEST guarded command"
    )
    assert calls[0]["keepalive_interval"] == 3


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
        harness_memory_guard,
        "_prune_stale_repo_processes",
        lambda **kwargs: (),
    )
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


def test_guarded_completed_process_honors_external_repo_sentinel_env(
    monkeypatch,
) -> None:
    captured: dict[str, object] = {}

    def fake_run_guarded(command, **kwargs):
        captured.update(kwargs)
        return harness_memory_guard.memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="ok\n",
            stderr="",
        )

    def fail_sentinel(**kwargs):  # type: ignore[no-untyped-def]
        raise AssertionError("external suite sentinel should suppress auto sentinel")

    monkeypatch.setattr(
        harness_memory_guard,
        "repo_process_sentinel",
        fail_sentinel,
    )
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
    env = {harness_memory_guard.repo_sentinel_active_env_key("MOLT_TEST"): "1"}

    result = harness_memory_guard.guarded_completed_process(
        [sys.executable, "-c", "print('ok')"],
        prefix="MOLT_TEST",
        env=env,
        limits=limits,
    )

    assert result.returncode == 0
    assert captured["cleanup_orphans"] is False


def test_execution_context_start_repo_sentinel_honors_external_marker(
    monkeypatch,
    tmp_path: Path,
) -> None:
    def fail_sentinel(**kwargs):  # type: ignore[no-untyped-def]
        raise AssertionError("external suite sentinel should suppress nested sentinel")

    monkeypatch.setattr(
        harness_memory_guard,
        "repo_process_sentinel",
        fail_sentinel,
    )
    limits = harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=2,
        max_total_rss_gb=3,
        max_global_rss_gb=4,
        poll_interval=0.1,
    )
    context = harness_memory_guard.HarnessExecutionContext.from_env(
        "MOLT_TEST",
        {harness_memory_guard.repo_sentinel_active_env_key("MOLT_TEST"): "1"},
        repo_root=tmp_path,
        limits=limits,
    )

    assert context.start_repo_sentinel(label="unit") is None


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
            elapsed_s=0.25,
        )

    monkeypatch.setattr(
        harness_memory_guard.memory_guard, "run_guarded", fake_run_guarded
    )
    monkeypatch.setattr(harness_memory_guard, "_sentinel_active", lambda: False)

    @contextlib.contextmanager
    def fake_auto_repo_sentinel(**kwargs):
        yield None

    monkeypatch.setattr(
        harness_memory_guard,
        "_auto_repo_sentinel",
        fake_auto_repo_sentinel,
    )
    monkeypatch.setattr(
        harness_memory_guard, "_utc_timestamp", lambda: "2026-05-21T12:00:00Z"
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
    assert "observed_at=2026-05-21T12:00:00Z" in result.stderr
    assert "elapsed=0.25s" in result.stderr
    assert (
        "next action: inspect child stderr/logs or host signal source" in result.stderr
    )


def test_guarded_completed_process_reports_actionable_violation(
    monkeypatch,
) -> None:
    def fake_run_guarded(command, **kwargs):
        return harness_memory_guard.memory_guard.GuardResult(
            returncode=harness_memory_guard.memory_guard.GUARD_RETURN_CODE,
            violation=harness_memory_guard.memory_guard.RssViolation(
                pid=123,
                rss_kb=4 * 1024 * 1024,
                command="python boom.py",
                scope="process_tree",
            ),
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=2.5,
            limit_at_violation=harness_memory_guard.memory_guard.ResolvedMemoryLimits(
                max_process_rss_kb=2 * 1024 * 1024,
                max_total_rss_kb=3 * 1024 * 1024,
            ),
        )

    monkeypatch.setattr(
        harness_memory_guard.memory_guard, "run_guarded", fake_run_guarded
    )
    monkeypatch.setattr(harness_memory_guard, "_sentinel_active", lambda: False)

    @contextlib.contextmanager
    def fake_auto_repo_sentinel(**kwargs):
        yield None

    monkeypatch.setattr(
        harness_memory_guard,
        "_auto_repo_sentinel",
        fake_auto_repo_sentinel,
    )
    monkeypatch.setattr(
        harness_memory_guard, "_utc_timestamp", lambda: "2026-05-21T12:00:00Z"
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

    assert result.returncode == harness_memory_guard.memory_guard.GUARD_RETURN_CODE
    assert "RSS limit exceeded; terminated the tracked process tree" in result.stderr
    assert "killed_at=2026-05-21T12:00:00Z" in result.stderr
    assert "elapsed=2.50s" in result.stderr
    assert "scope=process_tree" in result.stderr
    assert "limit=3.00GB" in result.stderr
    assert "MOLT_TEST_MAX_PROCESS_RSS_GB/MOLT_TEST_MAX_TOTAL_RSS_GB" in result.stderr


def test_guarded_completed_process_reports_actionable_timeout(
    monkeypatch,
) -> None:
    def fake_run_guarded(command, **kwargs):
        return harness_memory_guard.memory_guard.GuardResult(
            returncode=harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="memory_guard: timeout after 7.00s\n",
            timed_out=True,
            elapsed_s=7.01,
        )

    monkeypatch.setattr(
        harness_memory_guard.memory_guard, "run_guarded", fake_run_guarded
    )
    monkeypatch.setattr(
        harness_memory_guard, "_utc_timestamp", lambda: "2026-05-21T12:00:00Z"
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
        timeout=7,
    )

    assert result.returncode == harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE
    assert "timeout; terminated the tracked process tree" in result.stderr
    assert "killed_at=2026-05-21T12:00:00Z" in result.stderr
    assert "elapsed=7.01s" in result.stderr
    assert "timeout=7.00s" in result.stderr
    assert "MOLT_TEST_TIMEOUT_SEC or MOLT_TEST_PROCESS_TIMEOUT_SEC" in result.stderr


def test_guarded_completed_process_reports_orphan_cleanup(
    monkeypatch,
) -> None:
    def fake_run_guarded(command, **kwargs):
        assert kwargs["cleanup_orphans"] is True
        return harness_memory_guard.memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="ok\n",
            stderr="",
            elapsed_s=1.25,
            orphaned_process_groups=(101, 202),
        )

    monkeypatch.setattr(
        harness_memory_guard.memory_guard, "run_guarded", fake_run_guarded
    )
    monkeypatch.setattr(harness_memory_guard, "_sentinel_active", lambda: False)

    @contextlib.contextmanager
    def fake_auto_repo_sentinel(**kwargs):
        yield None

    monkeypatch.setattr(
        harness_memory_guard,
        "_auto_repo_sentinel",
        fake_auto_repo_sentinel,
    )
    monkeypatch.setattr(
        harness_memory_guard, "_utc_timestamp", lambda: "2026-05-21T12:00:00Z"
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

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    assert "orphaned child processes detected after command exit" in result.stderr
    assert "killed_at=2026-05-21T12:00:00Z" in result.stderr
    assert "elapsed=1.25s" in result.stderr
    assert "pgids=101,202" in result.stderr
    assert "next action: inspect child process lifecycle and logs" in result.stderr


def test_guarded_completed_process_defers_orphan_cleanup_to_active_sentinel(
    monkeypatch,
) -> None:
    captured: dict[str, object] = {}

    def fake_run_guarded(command, **kwargs):
        captured.update(kwargs)
        return harness_memory_guard.memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=0.1,
        )

    monkeypatch.setattr(
        harness_memory_guard.memory_guard, "run_guarded", fake_run_guarded
    )
    monkeypatch.setattr(harness_memory_guard, "_sentinel_active", lambda: True)
    limits = harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=2,
        max_total_rss_gb=3,
        max_global_rss_gb=4,
        poll_interval=0.1,
    )

    harness_memory_guard.guarded_completed_process(
        [sys.executable, "-c", "pass"],
        prefix="MOLT_TEST",
        limits=limits,
    )

    assert captured["cleanup_orphans"] is False


def test_guarded_completed_process_to_tempfiles_refreshes_dynamic_limits(
    monkeypatch,
) -> None:
    budget_calls: list[int] = []
    terminated: list[int] = []

    class FakeProc:
        pid = 4242
        returncode: int | None = None
        stdin = None

        def __init__(self, command, **kwargs):  # type: ignore[no-untyped-def]
            self.command = list(command)
            self.stdout_file = kwargs["stdout"]
            self.stderr_file = kwargs["stderr"]
            self.wait_count = 0
            self.stdout_file.write(b"partial\n")
            self.stdout_file.flush()

        def wait(self, timeout=None):  # type: ignore[no-untyped-def]
            self.wait_count += 1
            if self.wait_count == 1:
                if timeout is not None:
                    harness_memory_guard.time.sleep(timeout)
                raise subprocess.TimeoutExpired(self.command, timeout)
            self.returncode = 0
            return 0

        def kill(self) -> None:
            self.returncode = -9

    def fake_budget(prefix, environ=None, *, accounted_rss_kb=0):
        del environ
        assert prefix == "MOLT_CLI"
        budget_calls.append(accounted_rss_kb)
        return harness_memory_guard.memory_guard.AdaptiveMemoryBudget(
            max_process_rss_gb=4,
            max_total_rss_gb=5,
            max_global_rss_gb=6,
            reserve_gb=1,
            physical_gb=16,
            available_gb=12,
            source="test",
            accounted_rss_gb=accounted_rss_kb / (1024 * 1024),
        )

    def fake_samples():
        return {
            4242: harness_memory_guard.memory_guard.ProcessSample(
                pid=4242,
                ppid=1,
                pgid=4242,
                rss_kb=6 * 1024 * 1024,
                command="molt-backend fake",
            )
        }

    monkeypatch.setattr(harness_memory_guard.subprocess, "Popen", FakeProc)
    monkeypatch.setattr(
        harness_memory_guard.memory_guard,
        "adaptive_memory_budget",
        fake_budget,
    )
    monkeypatch.setattr(
        harness_memory_guard.memory_guard,
        "sample_processes",
        fake_samples,
    )
    monkeypatch.setattr(
        harness_memory_guard.memory_guard,
        "terminate_watched_processes",
        lambda root_pid, **kwargs: terminated.append(root_pid),
    )
    limits = harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=2,
        max_total_rss_gb=3,
        max_global_rss_gb=4,
        poll_interval=0.001,
        adaptive_prefix="MOLT_CLI",
        dynamic_process_rss=True,
        dynamic_total_rss=True,
        dynamic_global_rss=True,
        dynamic_child_rlimit=True,
    )

    result = harness_memory_guard.guarded_completed_process_to_tempfiles(
        ["molt-backend", "fake"],
        prefix="MOLT_CLI",
        limits=limits,
    )

    assert result.returncode == harness_memory_guard.memory_guard.GUARD_RETURN_CODE
    assert result.stdout == b"partial\n"
    assert b"molt memory guard: RSS limit exceeded" in result.stderr
    assert b"terminated the tracked process tree" in result.stderr
    assert b"next action: inspect child logs" in result.stderr
    assert 6 * 1024 * 1024 in budget_calls
    assert terminated == [4242]


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
    harness_memory_guard._TERMINATED_PGIDS.clear()
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
    monkeypatch.setattr(
        harness_memory_guard, "_claim_terminated_pgid", lambda pgid: True
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
    scans: list[tuple[int, float]] = []
    violations: list[dict[str, object]] = []

    sentinel = harness_memory_guard.repo_process_sentinel(
        repo_root=tmp_path,
        artifact_root=tmp_path,
        label="unit",
        limits=limits,
        on_scan=lambda groups, resolved, elapsed: scans.append(
            (len(groups), resolved.max_global_rss_gb or 0)
        ),
        on_violation=lambda _violation, _resolved, payload: violations.append(
            dict(payload)
        ),
    )
    sentinel.scan_once()

    assert sentinel.tripped is True
    assert terminated == [12345]
    assert scans == [(1, 4)]
    assert violations
    assert violations[0]["global_total_kb"] == 3 * 1024 * 1024
    assert violations[0]["active_pgids"] == [12345]
    events = sentinel.events_path.read_text(encoding="utf-8")
    assert "repo_process_guard_tripped" in events
    assert "limits" in events
    assert "killed_at" in events
    assert "guard_started_at" in events
    assert "elapsed_s" in events
    assert "global_rss" in events
    assert "terminated process group to prevent orphaned Molt subprocesses" in events


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
    assert "killed_at" in events
    assert "guard_started_at" in events
    assert "elapsed_s" in events
    assert "terminated process group left behind by the guarded scope" in events


def test_auto_repo_sentinel_does_not_exit_drain(monkeypatch, tmp_path: Path) -> None:
    captured: dict[str, object] = {}

    @contextlib.contextmanager
    def fake_repo_process_sentinel(**kwargs):  # type: ignore[no-untyped-def]
        captured.update(kwargs)
        yield object()

    monkeypatch.setattr(
        harness_memory_guard,
        "repo_process_sentinel",
        fake_repo_process_sentinel,
    )
    monkeypatch.setattr(
        harness_memory_guard,
        "_artifact_root_from_env",
        lambda env: tmp_path,
    )
    monkeypatch.setattr(harness_memory_guard, "_sentinel_active", lambda: False)
    monkeypatch.setattr(
        harness_memory_guard,
        "_prune_stale_repo_processes",
        lambda **kwargs: (),
    )
    limits = harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=2,
        max_total_rss_gb=3,
        max_global_rss_gb=4,
        poll_interval=0.001,
    )

    with harness_memory_guard._auto_repo_sentinel(
        prefix="MOLT_BUILD",
        env={},
        limits=limits,
    ):
        pass

    assert captured["drain_on_exit"] is False
    assert captured["suppress_auto_guard"] is False


def test_auto_repo_sentinel_prunes_stale_orphaned_groups(
    monkeypatch,
    tmp_path: Path,
    capsys,
) -> None:
    group = harness_memory_guard.process_sentinel.ProcessGroup(
        pgid=555,
        matched=True,
        samples=(
            harness_memory_guard.memory_guard.ProcessSample(
                pid=555,
                ppid=1,
                pgid=555,
                rss_kb=100,
                command="molt-backend --daemon",
                elapsed_sec=4000,
            ),
        ),
    )
    terminated: list[int] = []
    sentinel_calls: list[dict[str, object]] = []

    @contextlib.contextmanager
    def fake_repo_process_sentinel(**kwargs):  # type: ignore[no-untyped-def]
        sentinel_calls.append(kwargs)
        yield object()

    monkeypatch.setattr(
        harness_memory_guard.memory_guard,
        "sample_processes",
        lambda: {},
    )
    monkeypatch.setattr(
        harness_memory_guard.process_sentinel,
        "process_groups",
        lambda *args, **kwargs: [group],
    )
    monkeypatch.setattr(
        harness_memory_guard.process_sentinel,
        "terminate_group",
        lambda pgid, *, grace: terminated.append(pgid),
    )
    monkeypatch.setattr(
        harness_memory_guard,
        "repo_process_sentinel",
        fake_repo_process_sentinel,
    )
    monkeypatch.setattr(
        harness_memory_guard,
        "_artifact_root_from_env",
        lambda env: tmp_path,
    )
    monkeypatch.setattr(harness_memory_guard, "_sentinel_active", lambda: False)
    monkeypatch.setattr(harness_memory_guard, "_utc_timestamp", lambda: "now")
    limits = harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=2,
        max_total_rss_gb=3,
        max_global_rss_gb=4,
        poll_interval=0.001,
    )

    with harness_memory_guard._auto_repo_sentinel(
        prefix="MOLT_BUILD",
        env={
            "MOLT_BUILD_STALE_ORPHAN_CLEANUP": "1",
            "MOLT_BUILD_STALE_ORPHAN_SEC": "3600",
        },
        limits=limits,
    ):
        pass

    assert terminated == [555]
    assert sentinel_calls
    err = capsys.readouterr().err
    assert "stale orphaned Molt process group" in err
    assert "age=4000s" in err
    assert "threshold=3600s" in err
    events = (tmp_path / "memory_guard" / "molt_build_stale_preflight.jsonl").read_text(
        encoding="utf-8"
    )
    assert "repo_process_guard_stale_preflight" in events
    assert "stale_orphan" in events


def test_repo_process_sentinel_remembers_observed_child_groups(
    monkeypatch,
    tmp_path: Path,
) -> None:
    seen_known: list[set[int]] = []

    def fake_process_groups(*args, **kwargs):
        known = set(kwargs.get("known_pgids") or set())
        seen_known.append(known)
        pgid = 456 if 123 in known else 123
        return [
            harness_memory_guard.process_sentinel.ProcessGroup(
                pgid=pgid,
                matched=True,
                samples=(
                    harness_memory_guard.memory_guard.ProcessSample(
                        pid=pgid,
                        ppid=1,
                        pgid=pgid,
                        rss_kb=100,
                        command="molt worker",
                    ),
                ),
            )
        ]

    monkeypatch.setattr(
        harness_memory_guard.process_sentinel,
        "process_groups",
        fake_process_groups,
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
        label="unit-observed",
        limits=limits,
    )

    first = sentinel._current_groups()
    second = sentinel._current_groups()

    assert [group.pgid for group in first] == [123]
    assert [group.pgid for group in second] == [456]
    assert set() in seen_known
    assert {123} in seen_known
    assert sentinel._observed_pgids == {123, 456}


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


def _make_relative_interpreter(tmp_path: Path) -> Path:
    """Create `<tmp_path>/relbin/python3` pointing at the real interpreter.

    Mirrors the real-world `.venv/bin/python3` symlink chain a caller can hand
    the guard as a relative path. A symlink preserves the interpreter's behavior
    while letting the test reference it as a relative path that only resolves
    against the parent (test) cwd, not an arbitrary child `cwd=`.
    """
    rel_dir = tmp_path / "relbin"
    rel_dir.mkdir()
    rel_interp = rel_dir / "python3"
    rel_interp.symlink_to(Path(sys.executable).resolve())
    return rel_interp


@pytest.mark.skipif(
    sys.platform.startswith("win"),
    reason="relative venv interpreter symlink chain is a POSIX concern",
)
def test_relative_executable_resolved_against_parent_cwd_under_guard(
    monkeypatch, tmp_path
) -> None:
    # Regression: a relative path-bearing interpreter (e.g. `.venv/bin/python3`)
    # must resolve against the PARENT cwd, not the child `cwd=`. Before the fix
    # this raised FileNotFoundError when `cwd` differed from where the relative
    # interpreter lives.
    _make_relative_interpreter(tmp_path)
    other_cwd = tmp_path / "elsewhere"
    other_cwd.mkdir()
    monkeypatch.chdir(tmp_path)
    limits = harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=2,
        max_total_rss_gb=3,
        max_global_rss_gb=4,
        poll_interval=0.01,
    )

    result = harness_memory_guard.guarded_completed_process(
        ["relbin/python3", "-c", "print('relok')"],
        prefix="MOLT_TEST",
        cwd=str(other_cwd),
        limits=limits,
    )

    assert result.returncode == 0
    assert result.stdout == "relok\n"


@pytest.mark.skipif(
    sys.platform.startswith("win"),
    reason="relative venv interpreter symlink chain is a POSIX concern",
)
def test_relative_executable_resolved_when_guard_disabled(
    monkeypatch, tmp_path
) -> None:
    # The disabled-guard fast path goes straight through `subprocess.run(cwd=...)`
    # and must apply the same relative-executable resolution as the guarded path.
    _make_relative_interpreter(tmp_path)
    other_cwd = tmp_path / "elsewhere"
    other_cwd.mkdir()
    monkeypatch.chdir(tmp_path)
    limits = harness_memory_guard.HarnessMemoryLimits(
        enabled=False,
        max_process_rss_gb=2,
        max_total_rss_gb=3,
        max_global_rss_gb=4,
        poll_interval=0.01,
    )

    result = harness_memory_guard.guarded_completed_process(
        ["relbin/python3", "-c", "print('relok-disabled')"],
        prefix="MOLT_TEST",
        cwd=str(other_cwd),
        limits=limits,
    )

    assert result.returncode == 0
    assert result.stdout == "relok-disabled\n"
