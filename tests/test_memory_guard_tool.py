from __future__ import annotations

import json
from pathlib import Path
import sys

import pytest

import tools.memory_guard as memory_guard


def test_parse_process_table_keeps_commands_with_spaces() -> None:
    samples = memory_guard.parse_process_table(
        """
          10     1  2048 python worker.py --flag value
          11    10  4096 /bin/sh -c echo hi
        """
    )

    assert samples[10] == memory_guard.ProcessSample(
        pid=10,
        ppid=1,
        rss_kb=2048,
        command="python worker.py --flag value",
    )
    assert samples[11].command == "/bin/sh -c echo hi"


def test_parse_process_table_reads_process_group_ids() -> None:
    samples = memory_guard.parse_process_table(
        """
          10     1    10  2048 python worker.py --flag value
          11    10    10  4096 /bin/sh -c echo hi
        """
    )

    assert samples[10] == memory_guard.ProcessSample(
        pid=10,
        ppid=1,
        rss_kb=2048,
        command="python worker.py --flag value",
        pgid=10,
    )
    assert samples[11].pgid == 10


def test_parse_process_table_reads_process_elapsed_age() -> None:
    samples = memory_guard.parse_process_table(
        """
          10     1    10  2048  901 python worker.py --flag value
          11    10    10  4096  01:02:03 /bin/sh -c echo hi
          12    10    10  4096  2-03:04:05 python slow.py
        """
    )

    assert samples[10] == memory_guard.ProcessSample(
        pid=10,
        ppid=1,
        rss_kb=2048,
        command="python worker.py --flag value",
        pgid=10,
        elapsed_sec=901,
    )
    assert samples[11].elapsed_sec == 3723
    assert samples[12].elapsed_sec == 183845


def test_descendant_pids_includes_grandchildren() -> None:
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root"),
        101: memory_guard.ProcessSample(101, 100, 20, "child"),
        102: memory_guard.ProcessSample(102, 101, 30, "grandchild"),
        200: memory_guard.ProcessSample(200, 1, 999_999, "unrelated"),
    }

    assert memory_guard.descendant_pids(samples, 100) == {100, 101, 102}


def test_watched_pids_includes_reparented_process_group_members() -> None:
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(101, 100, 20, "child", pgid=100),
        102: memory_guard.ProcessSample(102, 1, 30, "reparented", pgid=100),
        200: memory_guard.ProcessSample(200, 1, 999_999, "unrelated", pgid=200),
    }

    assert memory_guard.watched_pids(samples, 100) == {100, 101, 102}


def test_process_tree_tracker_keeps_reparented_new_session_child_after_seen() -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    first = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(101, 100, 20, "child", pgid=101),
        102: memory_guard.ProcessSample(102, 101, 30, "grandchild", pgid=102),
    }

    assert tracker.update(first) == {100, 101, 102}

    reparented = {
        101: memory_guard.ProcessSample(101, 1, 20, "child", pgid=101),
        102: memory_guard.ProcessSample(102, 1, 30, "grandchild", pgid=102),
    }

    assert tracker.update(reparented) == {101, 102}
    violation = memory_guard.find_rss_violation(
        reparented,
        root_pid=100,
        max_rss_kb=25,
        tracker=tracker,
    )
    assert violation == memory_guard.RssViolation(
        pid=102,
        rss_kb=30,
        command="grandchild",
    )


def test_find_rss_violation_catches_reparented_process_group_member() -> None:
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(101, 1, 26_000_000, "reparented", pgid=100),
    }

    violation = memory_guard.find_rss_violation(
        samples, root_pid=100, max_rss_kb=25_000_000
    )

    assert violation == memory_guard.RssViolation(
        pid=101,
        rss_kb=26_000_000,
        command="reparented",
    )


def test_terminate_watched_processes_fans_out_to_tracked_groups(monkeypatch) -> None:
    if memory_guard.os.name != "posix":
        return
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(101, 1, 20, "child", pgid=101),
        102: memory_guard.ProcessSample(102, 1, 30, "grandchild", pgid=102),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)

    def fake_killpg(pgid, sig):
        sent_groups.append((pgid, sig))
        if sig == memory_guard.signal.SIGTERM:
            raise ProcessLookupError

    def fake_kill(pid, sig):
        sent_pids.append((pid, sig))

    monkeypatch.setattr(memory_guard.os, "killpg", fake_killpg)
    monkeypatch.setattr(memory_guard.os, "kill", fake_kill)

    memory_guard.terminate_watched_processes(
        100,
        samples=samples,
        watched={100, 101, 102},
        grace=0.001,
    )

    assert (100, memory_guard.signal.SIGTERM) in sent_groups
    assert (101, memory_guard.signal.SIGTERM) in sent_groups
    assert (102, memory_guard.signal.SIGTERM) in sent_groups
    assert (101, memory_guard.signal.SIGKILL) in sent_pids
    assert (102, memory_guard.signal.SIGKILL) in sent_pids


def test_find_rss_violation_ignores_unrelated_processes() -> None:
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root"),
        101: memory_guard.ProcessSample(101, 100, 26_000_000, "child"),
        200: memory_guard.ProcessSample(200, 1, 40_000_000, "unrelated"),
    }

    violation = memory_guard.find_rss_violation(
        samples, root_pid=100, max_rss_kb=25_000_000
    )

    assert violation == memory_guard.RssViolation(
        pid=101,
        rss_kb=26_000_000,
        command="child",
    )


def test_find_rss_violation_returns_highest_descendant() -> None:
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root"),
        101: memory_guard.ProcessSample(101, 100, 28_000_000, "smaller"),
        102: memory_guard.ProcessSample(102, 100, 29_000_000, "larger"),
    }

    violation = memory_guard.find_rss_violation(
        samples, root_pid=100, max_rss_kb=25_000_000
    )

    assert violation is not None
    assert violation.pid == 102
    assert violation.rss_gb == pytest.approx(29_000_000 / (1024 * 1024))


def test_find_rss_violation_catches_aggregate_process_tree_rss() -> None:
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(101, 100, 15_000_000, "child-a", pgid=100),
        102: memory_guard.ProcessSample(102, 100, 15_000_000, "child-b", pgid=100),
        200: memory_guard.ProcessSample(200, 1, 40_000_000, "unrelated", pgid=200),
    }

    violation = memory_guard.find_rss_violation(
        samples,
        root_pid=100,
        max_rss_kb=25_000_000,
        max_total_rss_kb=25_000_000,
    )

    assert violation == memory_guard.RssViolation(
        pid=100,
        rss_kb=30_000_010,
        command="process tree aggregate",
        scope="process_tree",
    )


def test_max_rss_gb_accepts_high_workstation_limits() -> None:
    assert memory_guard.max_rss_kb_from_gb(96) == 96 * 1024 * 1024


def test_max_rss_gb_must_leave_margin_below_hard_cap() -> None:
    with pytest.raises(ValueError, match="below 112"):
        memory_guard.max_rss_kb_from_gb(112)


def test_max_global_rss_gb_must_leave_workstation_margin() -> None:
    assert memory_guard.max_global_rss_kb_from_gb(128) == 128 * 1024 * 1024
    with pytest.raises(ValueError, match="below 4096"):
        memory_guard.max_global_rss_kb_from_gb(4096)


def test_memory_guard_defaults_adapt_to_live_memory_budget() -> None:
    budget = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_TOTAL_MEMORY_GB": "128",
            "MOLT_BENCH_MEM_AVAILABLE_GB": "96",
        },
    )

    assert budget.reserve_gb == pytest.approx(7.68)
    assert budget.max_process_rss_gb == pytest.approx(46.262016)
    assert budget.max_total_rss_gb == pytest.approx(51.40224)
    assert budget.max_global_rss_gb == pytest.approx(85.6704)
    assert memory_guard.DEFAULT_POLL_INTERVAL_SEC == 0.10


def test_adaptive_budget_scales_up_and_down_with_live_available_memory() -> None:
    high = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_TOTAL_MEMORY_GB": "128",
            "MOLT_BENCH_MEM_AVAILABLE_GB": "120",
        },
    )
    pressured = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_TOTAL_MEMORY_GB": "128",
            "MOLT_BENCH_MEM_AVAILABLE_GB": "32",
        },
    )

    assert high.reserve_gb == pytest.approx(7.68)
    assert high.max_global_rss_gb == pytest.approx(108.9504)
    assert high.max_total_rss_gb == pytest.approx(65.37024)
    assert high.max_process_rss_gb == pytest.approx(58.833216)
    assert pressured.reserve_gb == pytest.approx(high.reserve_gb)
    assert pressured.max_global_rss_gb == pytest.approx(23.5904)
    assert pressured.max_total_rss_gb == pytest.approx(14.15424)
    assert pressured.max_process_rss_gb == pytest.approx(12.738816)
    assert high.max_global_rss_gb > pressured.max_global_rss_gb
    assert high.available_gb - high.max_global_rss_gb > high.reserve_gb
    assert pressured.available_gb - pressured.max_global_rss_gb > pressured.reserve_gb


def test_adaptive_budget_accounts_guarded_tree_rss_without_self_tightening() -> None:
    budget = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_TOTAL_MEMORY_GB": "128",
            "MOLT_BENCH_MEM_AVAILABLE_GB": "46",
        },
        accounted_rss_kb=50 * 1024 * 1024,
    )

    assert budget.accounted_rss_gb == pytest.approx(50.0)
    assert budget.available_gb == pytest.approx(96.0)
    assert budget.max_process_rss_gb == pytest.approx(46.262016)
    assert budget.max_total_rss_gb == pytest.approx(51.40224)
    assert budget.max_global_rss_gb == pytest.approx(85.6704)


def test_adaptive_budget_clamps_large_hosts_below_rss_conversion_cap() -> None:
    budget = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_TOTAL_MEMORY_GB": "512",
            "MOLT_BENCH_MEM_AVAILABLE_GB": "500",
        },
    )

    assert budget.reserve_gb == pytest.approx(12.0)
    assert budget.max_global_rss_gb == pytest.approx(473.36)
    assert budget.max_total_rss_gb == pytest.approx(
        memory_guard.DEFAULT_HARD_MAX_RSS_GB - 0.001
    )
    assert budget.max_process_rss_gb == pytest.approx(100.7991)
    assert memory_guard.max_rss_kb_from_gb(budget.max_total_rss_gb) > 0
    assert memory_guard.max_rss_kb_from_gb(budget.max_process_rss_gb) > 0


def test_resolve_memory_limits_refreshes_dynamic_caps() -> None:
    seen_accounted: list[int] = []

    def provider(accounted_rss_kb: int) -> memory_guard.AdaptiveMemoryBudget:
        seen_accounted.append(accounted_rss_kb)
        return memory_guard.AdaptiveMemoryBudget(
            max_process_rss_gb=4.0,
            max_total_rss_gb=6.0,
            max_global_rss_gb=8.0,
            reserve_gb=1.0,
            physical_gb=16.0,
            available_gb=12.0,
            source="test",
            accounted_rss_gb=accounted_rss_kb / (1024 * 1024),
        )

    limits = memory_guard.resolve_memory_limits(
        max_process_rss_kb=2 * 1024 * 1024,
        max_total_rss_kb=3 * 1024 * 1024,
        max_global_rss_kb=5 * 1024 * 1024,
        adaptive_budget_provider=provider,
        dynamic_process_rss=True,
        dynamic_total_rss=True,
        dynamic_global_rss=False,
        accounted_rss_kb=12345,
    )

    assert seen_accounted == [12345]
    assert limits.max_process_rss_kb == 4 * 1024 * 1024
    assert limits.max_total_rss_kb == 6 * 1024 * 1024
    assert limits.max_global_rss_kb == 5 * 1024 * 1024


def test_memory_guard_adaptive_defaults_do_not_starve_small_hosts() -> None:
    budget = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_TOTAL_MEMORY_GB": "7",
            "MOLT_BENCH_MEM_AVAILABLE_GB": "5",
        },
    )

    assert budget.reserve_gb == pytest.approx(1.0)
    assert budget.max_process_rss_gb == pytest.approx(2.0952)
    assert budget.max_total_rss_gb == pytest.approx(2.328)
    assert budget.max_global_rss_gb == pytest.approx(3.88)


def test_default_child_rlimit_tracks_process_rss_budget() -> None:
    assert memory_guard.default_child_rlimit_gb(
        max_process_rss_gb=2.0,
        max_total_rss_gb=3.0,
    ) == pytest.approx(2.0)
    assert memory_guard.default_child_rlimit_gb(
        max_process_rss_gb=2.0,
        max_total_rss_gb=3.0,
        max_global_rss_gb=4.0,
    ) == pytest.approx(2.0)
    assert memory_guard.default_child_rlimit_gb(
        max_process_rss_gb=46.0,
        max_total_rss_gb=51.0,
        max_global_rss_gb=85.0,
    ) == pytest.approx(46.0)
    assert memory_guard.default_child_rlimit_gb(
        max_process_rss_gb=46.0,
        max_total_rss_gb=51.0,
    ) == pytest.approx(46.0)


def test_run_command_passes_through_success() -> None:
    result = memory_guard.run_guarded(
        [sys.executable, "-c", "print('ok')"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
    )

    assert result.returncode == 0
    assert result.violation is None
    assert result.peak is not None
    assert result.peak.rss_kb > 0
    assert result.stdout == "ok\n"
    assert result.elapsed_s is not None
    assert result.elapsed_s > 0


def test_cleanup_tracked_orphans_terminates_live_tracked_groups(monkeypatch) -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    assert tracker.known_pgids is not None
    tracker.known_pgids.add(300)
    samples = {
        200: memory_guard.ProcessSample(
            pid=200,
            ppid=1,
            pgid=100,
            rss_kb=64,
            command="worker same group",
        ),
        300: memory_guard.ProcessSample(
            pid=300,
            ppid=1,
            pgid=300,
            rss_kb=64,
            command="worker new group",
        ),
    }
    calls: list[dict[str, object]] = []

    def fake_terminate(root_pid, **kwargs):
        calls.append({"root_pid": root_pid, **kwargs})

    monkeypatch.setattr(memory_guard, "terminate_watched_processes", fake_terminate)

    orphaned = memory_guard.cleanup_tracked_orphans(
        100,
        tracker=tracker,
        sampler=lambda: samples,
        grace=0.125,
    )

    assert orphaned == (100, 300)
    assert calls[0]["root_pid"] == 100
    assert calls[0]["watched"] == {200, 300}
    assert calls[0]["grace"] == 0.125


def test_run_command_cleans_tracked_orphans_by_default(monkeypatch) -> None:
    calls: list[dict[str, object]] = []

    def fake_cleanup(root_pid, **kwargs):
        calls.append({"root_pid": root_pid, **kwargs})
        return (777,)

    monkeypatch.setattr(memory_guard, "cleanup_tracked_orphans", fake_cleanup)

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "print('ok')"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
    )

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    assert result.orphaned_process_groups == (777,)
    assert len(calls) == 1


def test_run_command_captures_large_stdout_without_pipe_deadlock() -> None:
    script = (
        "import sys; "
        "sys.stdout.write('x' * (2 * 1024 * 1024)); "
        "sys.stdout.flush(); "
        "sys.stderr.write('done\\n')"
    )

    result = memory_guard.run_guarded(
        [sys.executable, "-c", script],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        timeout=5.0,
    )

    assert result.returncode == 0
    assert len(result.stdout) == 2 * 1024 * 1024
    assert result.stderr == "done\n"


def test_run_command_feeds_stdin_under_guard() -> None:
    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import sys; print(sys.stdin.read().upper())"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        input="guarded stdin",
    )

    assert result.returncode == 0
    assert result.stdout == "GUARDED STDIN\n"


def test_run_command_elapsed_excludes_guard_child_runner_startup() -> None:
    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import time; time.sleep(0.03); print('ok')"],
        max_rss_kb=1_000_000,
        poll_interval=1.0,
        child_rlimit_kb=1_000_000,
    )

    assert result.returncode == 0
    assert result.elapsed_s is not None
    assert result.elapsed_s >= 0.02
    assert result.elapsed_s < 0.5


def test_run_command_ignores_samples_without_root_pid() -> None:
    def sampler() -> dict[int, memory_guard.ProcessSample]:
        return {
            999_999: memory_guard.ProcessSample(999_999, 1, 1, "missing-root"),
        }

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "print('ok')"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        sampler=sampler,
    )

    assert result.returncode == 0
    assert result.violation is None


def test_run_command_returns_guard_code_on_real_low_limit() -> None:
    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import time; time.sleep(10)"],
        max_rss_kb=1,
        poll_interval=0.01,
    )

    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    assert result.violation is not None
    assert result.violation.rss_kb > 1


def test_run_command_fast_start_poll_catches_allocator_before_slow_poll() -> None:
    script = (
        "import time; "
        "buf = bytearray(192 * 1024 * 1024); "
        "time.sleep(0.20); "
        "print(len(buf))"
    )

    result = memory_guard.run_guarded(
        [sys.executable, "-c", script],
        max_rss_kb=96 * 1024,
        max_total_rss_kb=160 * 1024,
        poll_interval=1.0,
        child_rlimit_kb=None,
    )

    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    assert result.violation is not None
    assert result.elapsed_s is not None
    assert result.elapsed_s < 1.0


def test_run_command_rusage_catches_short_lived_allocator_spike() -> None:
    if memory_guard.os.name != "posix" or not hasattr(memory_guard.os, "wait4"):
        return
    script = "buf = bytearray(192 * 1024 * 1024); print(len(buf))"

    result = memory_guard.run_guarded(
        [sys.executable, "-c", script],
        max_rss_kb=96 * 1024,
        max_total_rss_kb=160 * 1024,
        poll_interval=1.0,
        child_rlimit_kb=None,
    )

    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    assert result.violation is not None
    assert result.violation.scope == "process_rusage"


def test_run_command_returns_timeout_code_when_wall_clock_expires() -> None:
    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import time; time.sleep(10)"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        timeout=0.01,
    )

    assert result.returncode == memory_guard.TIMEOUT_RETURN_CODE
    assert result.timed_out is True
    assert "timeout after" in result.stderr


def test_exit_signal_payload_classifies_direct_signal_status() -> None:
    assert memory_guard._exit_signal_payload(-15) == {
        "signal": 15,
        "name": "SIGTERM",
        "conventional_shell_status": False,
    }


def test_exit_signal_payload_classifies_shell_signal_status() -> None:
    assert memory_guard._exit_signal_payload(143) == {
        "signal": 15,
        "name": "SIGTERM",
        "conventional_shell_status": True,
    }


def test_main_enforces_timeout_and_writes_summary(
    tmp_path, capsys: pytest.CaptureFixture[str]
) -> None:
    summary_path = tmp_path / "timeout-summary.json"

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--child-rlimit-gb",
            "0",
            "--timeout",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "import time; time.sleep(10)",
        ]
    )

    assert rc == memory_guard.TIMEOUT_RETURN_CODE
    assert "timeout after" in capsys.readouterr().err
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["returncode"] == memory_guard.TIMEOUT_RETURN_CODE
    assert payload["timed_out"] is True
    assert payload["violation"] is None
    assert payload["exit_signal"] is None
    assert payload["incident"]["reason"] == "timeout"
    assert payload["incident"]["cleanup"] == "terminated tracked process tree"


def test_main_reports_signal_status_without_guard_violation(
    tmp_path, capsys: pytest.CaptureFixture[str], monkeypatch
) -> None:
    summary_path = tmp_path / "signal-summary.json"

    def fake_run_guarded(_command, **_kwargs):
        return memory_guard.GuardResult(
            returncode=143,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=0.3,
        )

    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "raise SystemExit(143)",
        ]
    )

    assert rc == 143
    assert "SIGTERM status" in capsys.readouterr().err
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["returncode"] == 143
    assert payload["child_rlimit_gb"] == pytest.approx(1.0)
    assert payload["timed_out"] is False
    assert payload["violation"] is None
    assert payload["exit_signal"] == {
        "signal": 15,
        "name": "SIGTERM",
        "conventional_shell_status": True,
    }
    assert payload["incident"]["reason"] == "signal_exit"
    assert payload["incident"]["elapsed_s"] == pytest.approx(0.3)


def test_main_rejects_unsafe_threshold(capsys: pytest.CaptureFixture[str]) -> None:
    rc = memory_guard.main(["--max-rss-gb", "112", "--", sys.executable, "-c", "pass"])

    assert rc == 2
    assert "below 112" in capsys.readouterr().err


def test_main_rejects_unsafe_total_threshold(
    capsys: pytest.CaptureFixture[str],
) -> None:
    rc = memory_guard.main(
        ["--max-total-rss-gb", "112", "--", sys.executable, "-c", "pass"]
    )

    assert rc == 2
    assert "below 112" in capsys.readouterr().err


def test_parser_accepts_process_and_tree_rss_aliases() -> None:
    args = memory_guard._parser().parse_args(
        [
            "--max-process-rss-gb",
            "1.5",
            "--max-tree-rss-gb",
            "2.5",
            "--",
            sys.executable,
            "-c",
            "pass",
        ]
    )
    group_args = memory_guard._parser().parse_args(
        [
            "--max-group-rss-gb",
            "3.5",
            "--",
            sys.executable,
            "-c",
            "pass",
        ]
    )

    assert args.max_rss_gb == 1.5
    assert args.max_total_rss_gb == 2.5
    assert group_args.max_total_rss_gb == 3.5


def test_main_reexec_hides_guarded_command_from_guard_argv() -> None:
    marker = "molt-backend-marker"
    captured: dict[str, object] = {}

    def fake_execve(path, argv, env):
        captured["path"] = path
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        raise SystemExit(73)

    with pytest.raises(SystemExit) as exc:
        memory_guard.main(
            [
                "--max-rss-gb",
                "1",
                "--poll-interval",
                "0.01",
                "--",
                sys.executable,
                "-c",
                f"print({marker!r})",
            ],
            hide_command_argv=True,
            execve=fake_execve,
        )

    assert exc.value.code == 73
    worker_argv = captured["argv"]
    assert isinstance(worker_argv, list)
    assert all(marker not in arg for arg in worker_argv)
    env = captured["env"]
    assert isinstance(env, dict)
    encoded = env[memory_guard.INTERNAL_COMMAND_ENV]
    assert json.loads(encoded) == [sys.executable, "-c", f"print({marker!r})"]
    assert env[memory_guard.INTERNAL_WORKER_ENV] == "1"


def test_main_reexec_preserves_stream_and_sample_rotation_options(tmp_path) -> None:
    captured: dict[str, object] = {}
    samples_path = tmp_path / "samples.jsonl"

    def fake_execve(path, argv, env):
        captured["path"] = path
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        raise SystemExit(74)

    with pytest.raises(SystemExit) as exc:
        memory_guard.main(
            [
                "--max-rss-gb",
                "1",
                "--poll-interval",
                "0.01",
                "--samples-jsonl",
                str(samples_path),
                "--samples-max-mb",
                "0.5",
                "--stream",
                "json-stderr",
                "--child-rlimit-gb",
                "0.75",
                "--",
                sys.executable,
                "-c",
                "print('ok')",
            ],
            hide_command_argv=True,
            execve=fake_execve,
        )

    assert exc.value.code == 74
    worker_argv = captured["argv"]
    assert isinstance(worker_argv, list)
    assert "--samples-jsonl" in worker_argv
    assert str(samples_path) in worker_argv
    assert "--samples-max-mb" in worker_argv
    assert "0.5" in worker_argv
    assert "--stream" in worker_argv
    assert "json-stderr" in worker_argv
    assert "--child-rlimit-gb" in worker_argv
    assert "0.75" in worker_argv


def test_internal_worker_loads_command_and_strips_internal_env(monkeypatch) -> None:
    command = [sys.executable, "-c", "print('worker')"]
    observed: dict[str, object] = {}

    def fake_run_guarded(seen_command, **kwargs):
        observed["command"] = list(seen_command)
        observed["env"] = dict(kwargs["env"])
        return memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
        )

    monkeypatch.setenv(memory_guard.INTERNAL_WORKER_ENV, "1")
    monkeypatch.setenv(memory_guard.INTERNAL_COMMAND_ENV, json.dumps(command))
    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--poll-interval",
            "0.01",
        ],
        hide_command_argv=True,
    )

    assert rc == 0
    assert observed["command"] == command
    child_env = observed["env"]
    assert isinstance(child_env, dict)
    assert memory_guard.INTERNAL_COMMAND_ENV not in child_env
    assert memory_guard.INTERNAL_WORKER_ENV not in child_env
    assert memory_guard.INTERNAL_CHILD_RUNNER_ENV not in child_env
    assert memory_guard.INTERNAL_CHILD_COMMAND_ENV not in child_env
    assert memory_guard.INTERNAL_CHILD_RLIMIT_KB_ENV not in child_env


def test_child_runner_env_wraps_command_without_leaking_guard_keys() -> None:
    command = [sys.executable, "-c", "print('child')"]
    env = memory_guard._child_runner_env(
        {
            "KEEP": "1",
            memory_guard.INTERNAL_WORKER_ENV: "1",
            memory_guard.INTERNAL_COMMAND_ENV: json.dumps(["hidden"]),
        },
        command,
        child_rlimit_kb=12345,
    )

    assert env[memory_guard.INTERNAL_CHILD_RUNNER_ENV] == "1"
    assert json.loads(env[memory_guard.INTERNAL_CHILD_COMMAND_ENV]) == command
    assert env[memory_guard.INTERNAL_CHILD_RLIMIT_KB_ENV] == "12345"
    assert memory_guard.INTERNAL_CHILD_STARTED_FD_ENV not in env
    child_env = memory_guard._child_env_without_internal_keys(env)
    assert child_env == {"KEEP": "1"}


def test_guarded_launch_applies_resource_limit_before_exec_on_posix() -> None:
    command = [sys.executable, "-c", "print('child')"]
    launch = memory_guard._guarded_launch(
        command,
        {"KEEP": "1"},
        child_rlimit_kb=12345,
    )

    if memory_guard.os.name == "posix":
        assert launch.command == command
        assert launch.env == {"KEEP": "1"}
        assert launch.preexec_fn is not None
        assert launch.started_read_fd is not None
        assert launch.pass_fds == launch.close_fds
    else:
        assert launch.command == [
            sys.executable,
            str(Path(memory_guard.__file__).resolve()),
        ]
        launch_env = launch.env
        assert launch_env is not None
        assert (
            json.loads(launch_env[memory_guard.INTERNAL_CHILD_COMMAND_ENV]) == command
        )
        assert launch_env[memory_guard.INTERNAL_CHILD_RLIMIT_KB_ENV] == "12345"
        assert memory_guard.INTERNAL_CHILD_STARTED_FD_ENV in launch_env
    memory_guard._close_fds((*launch.close_fds, launch.started_read_fd))


def test_main_writes_summary_json(tmp_path) -> None:
    summary_path = tmp_path / "summary.json"
    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--child-rlimit-gb",
            "0",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "print('ok')",
        ]
    )

    assert rc == 0
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["returncode"] == 0
    assert payload["violation"] is None
    assert payload["peak"]["rss_kb"] > 0
    assert payload["peak"]["scope"] == "process"
    assert payload["peak_total"]["rss_kb"] >= payload["peak"]["rss_kb"]
    assert payload["peak_total"]["scope"] == "process_tree"
    assert payload["max_total_rss_gb"] == pytest.approx(18.0)
    assert payload["child_rlimit_gb"] is None
    assert payload["orphaned_process_groups"] == []
    assert payload["incident"] is None


def test_main_reports_orphan_cleanup_with_operator_signal(
    tmp_path,
    capsys: pytest.CaptureFixture[str],
    monkeypatch,
) -> None:
    summary_path = tmp_path / "orphan-summary.json"

    def fake_run_guarded(_command, **_kwargs):
        return memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=0.4,
            orphaned_process_groups=(44,),
        )

    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "print('ok')",
        ]
    )

    assert rc == 0
    stderr = capsys.readouterr().err
    assert "orphaned child processes detected after command exit" in stderr
    assert "elapsed=0.40s" in stderr
    assert "pgids=44" in stderr
    assert "next action: inspect child process lifecycle and logs" in stderr
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["orphaned_process_groups"] == [44]
    assert payload["incident"]["reason"] == "orphaned_processes_cleaned"
    assert payload["incident"]["elapsed_s"] == pytest.approx(0.4)
    assert payload["incident"]["process_groups"] == [44]


def test_main_writes_samples_jsonl(tmp_path) -> None:
    samples_path = tmp_path / "samples.jsonl"
    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--poll-interval",
            "0.01",
            "--child-rlimit-gb",
            "0",
            "--samples-jsonl",
            str(samples_path),
            "--",
            sys.executable,
            "-c",
            "print('ok')",
        ]
    )

    assert rc == 0
    lines = samples_path.read_text(encoding="utf-8").splitlines()
    assert lines
    payload = json.loads(lines[-1])
    assert payload["root_pid"] > 0
    assert "peak" in payload
    assert "total" in payload


def test_sample_jsonl_rotation_bounds_artifacts(tmp_path) -> None:
    samples_path = tmp_path / "samples.jsonl"
    peak = memory_guard.RssViolation(pid=100, rss_kb=10, command="root")

    for _ in range(8):
        memory_guard._append_sample_jsonl(
            str(samples_path),
            root_pid=100,
            peak=peak,
            total=peak,
            violation=None,
            max_bytes=1024,
        )

    assert samples_path.exists()
    assert samples_path.with_name("samples.jsonl.1").exists()
    assert samples_path.stat().st_size <= 1024
    assert samples_path.with_name("samples.jsonl.1").stat().st_size <= 1024


def test_main_streams_samples_without_sample_artifact(
    tmp_path, capsys: pytest.CaptureFixture[str]
) -> None:
    samples_path = tmp_path / "samples.jsonl"

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--poll-interval",
            "0.01",
            "--child-rlimit-gb",
            "0",
            "--stream",
            "stderr",
            "--",
            sys.executable,
            "-c",
            "import time; time.sleep(0.05)",
        ]
    )

    captured = capsys.readouterr()
    assert rc == 0
    assert "memory_guard sample:" in captured.err
    assert not samples_path.exists()
