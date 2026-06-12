from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
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


def test_watched_pids_excludes_host_control_plane_group() -> None:
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
        ),
        200: memory_guard.ProcessSample(200, 1, 20, "unrelated", pgid=200),
    }

    assert memory_guard.watched_pids(samples, 100) == set()


def test_watched_pids_excludes_plain_claude_control_plane_group() -> None:
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "claude",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
        ),
        200: memory_guard.ProcessSample(200, 1, 20, "unrelated", pgid=200),
    }

    assert memory_guard.is_host_control_plane_process(samples[100])
    assert memory_guard.watched_pids(samples, 100) == set()


def test_watched_pids_excludes_claude_code_executable_group() -> None:
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "/opt/homebrew/bin/claude-code --continue",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
        ),
    }

    assert memory_guard.is_host_control_plane_process(samples[100])
    assert memory_guard.watched_pids(samples, 100) == set()


def test_watched_pids_excludes_node_launched_claude_code_group() -> None:
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "node /opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
        ),
    }

    assert memory_guard.is_host_control_plane_process(samples[100])
    assert memory_guard.watched_pids(samples, 100) == set()


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


def test_process_tree_tracker_does_not_absorb_root_ambient_process_group() -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    samples = {
        100: memory_guard.ProcessSample(100, 50, 10, "pytest current", pgid=500),
        50: memory_guard.ProcessSample(
            50,
            1,
            20,
            "/Applications/Codex.app/Contents/MacOS/Codex app-server",
            pgid=500,
        ),
        200: memory_guard.ProcessSample(
            200,
            50,
            30,
            "/Users/adpena/Projects/molt/.venv/bin/python3 tests/molt_diff.py",
            pgid=200,
        ),
    }

    assert tracker.update(samples) == {100}
    assert tracker.known_pids == {100}
    assert tracker.known_pgids == {100}


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


def test_terminate_watched_processes_kills_only_root_group_and_tracked_pids(
    monkeypatch,
) -> None:
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
    assert (101, memory_guard.signal.SIGTERM) not in sent_groups
    assert (102, memory_guard.signal.SIGTERM) not in sent_groups
    assert (101, memory_guard.signal.SIGTERM) in sent_pids
    assert (102, memory_guard.signal.SIGTERM) in sent_pids
    assert (101, memory_guard.signal.SIGKILL) in sent_pids
    assert (102, memory_guard.signal.SIGKILL) in sent_pids


def test_terminate_watched_processes_skips_host_control_plane_root_group(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "posix":
        return
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)
    monkeypatch.setattr(
        memory_guard.os,
        "killpg",
        lambda pgid, sig: sent_groups.append((pgid, sig)),
    )
    monkeypatch.setattr(
        memory_guard.os,
        "kill",
        lambda pid, sig: sent_pids.append((pid, sig)),
    )

    memory_guard.terminate_watched_processes(
        100,
        samples=samples,
        watched={100, 101},
        grace=0.001,
    )

    assert sent_groups == []
    assert sent_pids == []


def test_protected_process_groups_include_external_codex_descendant_not_owned_child() -> None:
    if memory_guard.os.name != "posix":
        return
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            10_000,
            "/bin/zsh -l",
            pgid=101,
        ),
        777: memory_guard.ProcessSample(
            777,
            101,
            250_000,
            "/Users/adpena/Projects/molt/target/dev-fast/molt-backend",
            pgid=777,
        ),
        999: memory_guard.ProcessSample(
            999,
            100,
            30_000,
            "python tools/memory_guard.py -- pytest",
            pgid=999,
        ),
        200: memory_guard.ProcessSample(
            200,
            999,
            250_000,
            "/Users/adpena/Projects/molt/target/dev-fast/molt-backend",
            pgid=200,
        ),
    }

    protected = memory_guard.protected_process_group_ids(
        samples,
        self_pid=999,
        self_pgid=999,
    )

    assert 100 in protected
    assert 777 in protected
    assert 999 in protected
    assert 200 not in protected


def test_protected_process_groups_include_external_claude_descendant_not_owned_child() -> None:
    if memory_guard.os.name != "posix":
        return
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "claude --dangerously-skip-permissions",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            10_000,
            "/bin/zsh -c source /Users/adpena/.claude/shell-snapshots/snapshot-zsh",
            pgid=101,
        ),
        777: memory_guard.ProcessSample(
            777,
            101,
            250_000,
            "/Users/adpena/Projects/molt/target/dev-fast/molt-backend",
            pgid=777,
        ),
        999: memory_guard.ProcessSample(
            999,
            1,
            30_000,
            "python tools/memory_guard.py -- pytest",
            pgid=999,
        ),
        200: memory_guard.ProcessSample(
            200,
            999,
            250_000,
            "/Users/adpena/Projects/molt/target/dev-fast/molt-backend",
            pgid=200,
        ),
    }

    protected = memory_guard.protected_process_group_ids(
        samples,
        self_pid=999,
        self_pgid=999,
    )

    assert 100 in protected
    assert 101 in protected
    assert 777 in protected
    assert 999 in protected
    assert 200 not in protected


def test_terminate_single_process_group_refuses_protected_group(monkeypatch) -> None:
    if memory_guard.os.name != "posix":
        return
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)
    monkeypatch.setattr(
        memory_guard.os,
        "killpg",
        lambda pgid, sig: sent_groups.append((pgid, sig)),
    )

    assert memory_guard._terminate_single_process_group(100, grace=0.001) is True

    assert sent_groups == []


def test_terminate_watched_processes_filters_protected_escaped_pid(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "posix":
        return
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(
            101,
            100,
            500_000,
            "/Applications/Codex.app/Contents/Resources/codex app-server",
            pgid=777,
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)

    def fake_killpg(pgid, sig):
        sent_groups.append((pgid, sig))
        if sig == memory_guard.signal.SIGTERM:
            raise ProcessLookupError

    monkeypatch.setattr(memory_guard.os, "killpg", fake_killpg)
    monkeypatch.setattr(
        memory_guard.os,
        "kill",
        lambda pid, sig: sent_pids.append((pid, sig)),
    )

    memory_guard.terminate_watched_processes(
        100,
        samples=samples,
        watched={100, 101},
        grace=0.001,
    )

    assert (100, memory_guard.signal.SIGTERM) in sent_groups
    assert all(pid != 101 for pid, _sig in sent_pids)


def test_terminate_watched_processes_never_killpgs_shared_child_group(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "posix":
        return
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(101, 100, 20, "child", pgid=777),
        200: memory_guard.ProcessSample(200, 1, 999, "unrelated", pgid=777),
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
        watched={100, 101},
        grace=0.001,
    )

    assert (100, memory_guard.signal.SIGTERM) in sent_groups
    assert all(pgid != 777 for pgid, _sig in sent_groups)
    assert (101, memory_guard.signal.SIGTERM) in sent_pids
    assert (101, memory_guard.signal.SIGKILL) in sent_pids
    assert all(pid != 200 for pid, _sig in sent_pids)


def test_terminate_watched_processes_never_kills_host_control_plane_group(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "posix":
        return
    samples = {
        100: memory_guard.ProcessSample(
            100,
            27404,
            20,
            "uv run python tests/molt_diff.py --jobs 1",
            pgid=700,
        ),
        27404: memory_guard.ProcessSample(
            27404,
            27335,
            500_000,
            "/Applications/Codex.app/Contents/Resources/codex app-server",
            pgid=700,
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999)
    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(
        memory_guard.os,
        "killpg",
        lambda pgid, sig: sent_groups.append((pgid, sig)),
    )
    monkeypatch.setattr(
        memory_guard.os,
        "kill",
        lambda pid, sig: sent_pids.append((pid, sig)),
    )

    memory_guard.terminate_watched_processes(
        100,
        samples=samples,
        watched={100},
        grace=0.001,
    )

    assert sent_groups == []
    assert sent_pids == []


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


def test_parse_darwin_vm_stat_available_bytes() -> None:
    text = """
Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                             10.
Pages active:                           99.
Pages inactive:                         20.
Pages speculative:                       3.
Pages purgeable:                         2.
Pages wired down:                       88.
Pages occupied by compressor:            7.
"""

    available = memory_guard._parse_darwin_vm_stat_available_bytes(text)

    assert available == (10 + 20 + 3 + 2) * 16_384


def test_available_memory_bytes_uses_darwin_vm_stat(monkeypatch) -> None:
    class Result:
        returncode = 0
        stdout = (
            "Mach Virtual Memory Statistics: (page size of 4096 bytes)\n"
            "Pages free: 2.\n"
            "Pages inactive: 3.\n"
            "Pages speculative: 5.\n"
            "Pages purgeable: 7.\n"
        )

    monkeypatch.setattr(memory_guard.sys, "platform", "darwin")
    monkeypatch.setattr(
        memory_guard.subprocess,
        "run",
        lambda *args, **kwargs: Result(),
    )

    assert memory_guard.available_memory_bytes(environ={}) == 17 * 4096


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


def test_run_guarded_binary_capture_preserves_bytes() -> None:
    result = memory_guard.run_guarded(
        [
            sys.executable,
            "-c",
            (
                "import sys; "
                "data = sys.stdin.buffer.read(); "
                "sys.stdout.buffer.write(data[::-1]); "
                "sys.stderr.buffer.write(b'err:' + data[:2])"
            ),
        ],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        input=b"\xffabc",
        text=False,
    )

    assert result.returncode == 0
    assert result.stdout == b"cba\xff"
    assert result.stderr == b"err:\xffa"


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
        sampler=lambda: {},
    )

    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    assert result.violation is not None
    assert result.elapsed_s is not None
    assert result.elapsed_s < 1.0


def test_run_command_rusage_catches_short_lived_allocator_spike() -> None:
    if memory_guard.os.name != "posix" or not hasattr(memory_guard.os, "wait4"):
        return
    script = "import os\nbuf = bytearray(192 * 1024 * 1024)\nos._exit(0)"

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


def test_run_command_timeout_teardown_uses_bounded_wait(monkeypatch) -> None:
    waits: list[float | None] = []

    class FakeProc:
        pid = 987654
        returncode: int | None = None
        stdin = None

        def __init__(self, command, **_kwargs):  # type: ignore[no-untyped-def]
            self.command = list(command)

        def wait(self, timeout=None):  # type: ignore[no-untyped-def]
            waits.append(timeout)
            if timeout is None:
                raise AssertionError("memory guard attempted an unbounded wait")
            raise subprocess.TimeoutExpired(self.command, timeout)

        def poll(self):  # type: ignore[no-untyped-def]
            return self.returncode

    monkeypatch.setattr(memory_guard.subprocess, "Popen", FakeProc)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {})

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import time; time.sleep(10)"],
        max_rss_kb=1_000_000,
        poll_interval=0.001,
        timeout=0.001,
        env={"MOLT_MEMORY_GUARD_TERMINATION_WAIT_SEC": "0.001"},
        sampler=lambda: {},
    )

    assert result.returncode == memory_guard.TIMEOUT_RETURN_CODE
    assert result.timed_out is True
    assert "termination wait expired" in result.stderr
    assert waits
    assert None not in waits


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


def test_cargo_incremental_quarantine_moves_only_incremental_dirs(
    tmp_path: Path,
) -> None:
    target = tmp_path / "target"
    debug_file = target / "debug" / "incremental" / "unit-a" / "work.o"
    triple_file = (
        target
        / "aarch64-apple-darwin"
        / "dev-fast"
        / "incremental"
        / "unit-b"
        / "work.o"
    )
    non_incremental = target / "debug" / "deps" / "libmolt.rlib"
    for path in (debug_file, triple_file, non_incremental):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(path.name, encoding="utf-8")

    receipt = memory_guard._quarantine_cargo_incremental_state(
        reason="signal_exit",
        target_dir=target,
        command=["cargo", "test"],
        cwd=tmp_path,
    )

    assert not (target / "debug" / "incremental").exists()
    assert not (target / "aarch64-apple-darwin" / "dev-fast" / "incremental").exists()
    assert non_incremental.exists()
    assert len(receipt.moved_paths) == 2
    assert receipt.errors == ()
    assert receipt.quarantine_dir is not None
    quarantine_dir = Path(receipt.quarantine_dir)
    assert (quarantine_dir / "debug" / "incremental" / "unit-a" / "work.o").exists()
    assert (
        quarantine_dir
        / "aarch64-apple-darwin"
        / "dev-fast"
        / "incremental"
        / "unit-b"
        / "work.o"
    ).exists()
    assert receipt.receipt_path is not None
    payload = json.loads(Path(receipt.receipt_path).read_text(encoding="utf-8"))
    assert payload["reason"] == "signal_exit"
    assert payload["target_dir"] == str(target)
    assert payload["command"] == ["cargo", "test"]
    assert len(payload["moved_paths"]) == 2


def test_cargo_incremental_quarantine_prunes_old_receipts(tmp_path: Path) -> None:
    target = tmp_path / "target"
    parent = target / ".molt_state" / "quarantine" / "cargo_incremental"
    for index in range(3):
        stale = parent / f"stale-{index}"
        stale.mkdir(parents=True)
        os.utime(stale, (index + 1, index + 1))
    live_file = target / "debug" / "incremental" / "unit" / "work.o"
    live_file.parent.mkdir(parents=True, exist_ok=True)
    live_file.write_text("work", encoding="utf-8")

    receipt = memory_guard._quarantine_cargo_incremental_state(
        reason="timeout",
        target_dir=target,
        command=["cargo", "build"],
        cwd=tmp_path,
        retention_keep=2,
    )

    assert receipt.quarantine_dir is not None
    remaining = sorted(path.name for path in parent.iterdir() if path.is_dir())
    assert len(remaining) == 2
    assert Path(receipt.quarantine_dir).name in remaining
    assert receipt.pruned_quarantine_dirs


def test_run_guarded_signal_exit_quarantines_cargo_incremental(
    tmp_path: Path,
) -> None:
    target = tmp_path / "target"
    live_file = target / "debug" / "incremental" / "unit" / "work.o"
    live_file.parent.mkdir(parents=True, exist_ok=True)
    live_file.write_text("work", encoding="utf-8")

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import os, signal; os.kill(os.getpid(), signal.SIGTERM)"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        cwd=tmp_path,
        env={"CARGO_TARGET_DIR": str(target)},
        sampler=lambda: {},
    )

    assert result.returncode == -15
    assert result.cargo_incremental_quarantine is None
    assert live_file.exists()

    fake_cargo = tmp_path / "cargo"
    fake_cargo.write_text(
        f"#!{sys.executable}\n"
        "import os, signal\n"
        "os.kill(os.getpid(), signal.SIGTERM)\n",
        encoding="utf-8",
    )
    fake_cargo.chmod(0o755)
    result = memory_guard.run_guarded(
        [str(fake_cargo)],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        cwd=tmp_path,
        env={"CARGO_TARGET_DIR": str(target)},
        sampler=lambda: {},
    )

    assert result.returncode != 0
    assert result.cargo_incremental_quarantine is not None
    assert "quarantined Cargo incremental state" in result.stderr
    assert not (target / "debug" / "incremental").exists()


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


def test_main_reports_cargo_incremental_quarantine_summary(
    tmp_path, capsys: pytest.CaptureFixture[str], monkeypatch
) -> None:
    summary_path = tmp_path / "signal-summary.json"
    target = tmp_path / "target"
    quarantine = target / ".molt_state" / "quarantine" / "cargo_incremental" / "q"
    receipt = memory_guard.CargoIncrementalQuarantine(
        reason="signal_exit",
        recorded_at="2026-06-12T00:00:00Z",
        target_dir=str(target),
        quarantine_dir=str(quarantine),
        command=("cargo", "test"),
        cwd=str(tmp_path),
        moved_paths=(
            memory_guard.CargoIncrementalQuarantineMove(
                original_path=str(target / "debug" / "incremental"),
                quarantined_path=str(quarantine / "debug" / "incremental"),
            ),
        ),
        receipt_path=str(quarantine / "receipt.json"),
    )

    def fake_run_guarded(_command, **_kwargs):
        return memory_guard.GuardResult(
            returncode=143,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=0.3,
            cargo_incremental_quarantine=receipt,
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
            "cargo",
            "test",
        ]
    )

    assert rc == 143
    stderr = capsys.readouterr().err
    assert "quarantined Cargo incremental state after signal_exit" in stderr
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["cargo_incremental_quarantine"]["reason"] == "signal_exit"
    assert payload["cargo_incremental_quarantine"]["target_dir"] == str(target)
    assert len(payload["cargo_incremental_quarantine"]["moved_paths"]) == 1
    assert payload["incident"]["cleanup"] == "quarantined Cargo incremental state"


def test_main_reports_incident_repro_context(
    tmp_path,
    capsys: pytest.CaptureFixture[str],
    monkeypatch,
) -> None:
    summary_path = tmp_path / "rss-summary.json"
    current_root = tmp_path / "pytest-memory-guard"
    current_test_path = current_root / "pytest-current-test.json"
    monkeypatch.setattr(memory_guard, "PYTEST_OUTER_GUARD_SUMMARY_DIR", current_root)
    current_root.mkdir(parents=True)
    current_test_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "nodeid": "tests/test_memory_guard_tool.py::live_unit",
                "phase": "call",
            },
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    env = {
        "PATH": "/usr/bin",
        "PYTEST_CURRENT_TEST": "tests/test_memory_guard_tool.py::unit (call)",
        "MOLT_PYTEST_CURRENT_TEST_FILE": str(current_test_path),
        "MOLT_SESSION_ID": "unit-session",
        "SECRET_TOKEN": "must-not-leak",
    }

    def fake_run_guarded(_command, **_kwargs):
        return memory_guard.GuardResult(
            returncode=memory_guard.GUARD_RETURN_CODE,
            violation=memory_guard.RssViolation(
                pid=321,
                rss_kb=4 * 1024 * 1024,
                command="python hungry.py",
                scope="process_tree",
            ),
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=1.25,
            limit_at_violation=memory_guard.ResolvedMemoryLimits(
                max_process_rss_kb=2 * 1024 * 1024,
                max_total_rss_kb=3 * 1024 * 1024,
            ),
        )

    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {})

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "2",
            "--max-total-rss-gb",
            "3",
            "--poll-interval",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "pass",
        ],
        environ=env,
    )

    assert rc == memory_guard.GUARD_RETURN_CODE
    stderr = capsys.readouterr().err
    assert "memory_guard: repro context:" in stderr
    assert "tests/test_memory_guard_tool.py::unit" in stderr
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    repro = payload["repro"]
    assert repro["command"] == [sys.executable, "-c", "pass"]
    assert repro["pytest"]["current_test"] == env["PYTEST_CURRENT_TEST"]
    assert (
        repro["pytest"]["current_test_file"]["payload"]["nodeid"]
        == "tests/test_memory_guard_tool.py::live_unit"
    )
    assert repro["env"]["MOLT_SESSION_ID"] == "unit-session"
    assert "SECRET_TOKEN" not in repro["env"]
    assert repro["limits"]["max_total_rss_gb"] == pytest.approx(3.0)


def test_repro_context_reads_xdist_worker_current_test_sidecars(
    tmp_path: Path,
    monkeypatch,
) -> None:
    current_root = tmp_path / "pytest-memory-guard"
    aggregate_path = current_root / "pytest-current-test.json"
    worker_dir = aggregate_path.with_name(f"{aggregate_path.name}.d")
    worker_dir.mkdir(parents=True)
    worker_path = worker_dir / "gw0-4321_current-test.json"
    worker_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "pid": 4321,
                "nodeid": "tests/test_xdist.py::test_memory",
                "phase": "call",
                "xdist_worker": "gw0",
            },
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(memory_guard, "PYTEST_OUTER_GUARD_SUMMARY_DIR", current_root)
    monkeypatch.setattr(
        memory_guard,
        "sample_processes",
        lambda: {
            4321: memory_guard.ProcessSample(
                pid=4321,
                ppid=100,
                rss_kb=1,
                command="pytest worker gw0",
            ),
            9876: memory_guard.ProcessSample(
                pid=9876,
                ppid=4321,
                rss_kb=4 * 1024 * 1024,
                command="python hungry.py",
            ),
        },
    )

    repro = memory_guard.repro_context_payload(
        command=[sys.executable, "-m", "pytest", "-n", "2"],
        cwd=tmp_path,
        environ={
            "MOLT_PYTEST_CURRENT_TEST_FILE": str(aggregate_path),
            "PYTEST_XDIST_WORKER": "",
        },
        incident_pid=9876,
    )

    current_test = repro["pytest"]["current_test_file"]
    assert current_test["missing"] is True
    records = current_test["worker_records"]
    assert records[0]["incident_match"] == "pid_lineage"
    assert records[0]["payload"]["nodeid"] == "tests/test_xdist.py::test_memory"


def test_repro_context_rejects_noncanonical_current_test_file(
    tmp_path: Path,
    monkeypatch,
) -> None:
    current_root = tmp_path / "pytest-memory-guard"
    outside_path = tmp_path / "outside" / "pytest-current-test.json"
    outside_path.parent.mkdir()
    outside_path.write_text("{}", encoding="utf-8")
    monkeypatch.setattr(memory_guard, "PYTEST_OUTER_GUARD_SUMMARY_DIR", current_root)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {})

    repro = memory_guard.repro_context_payload(
        command=[sys.executable, "-m", "pytest"],
        cwd=tmp_path,
        environ={"MOLT_PYTEST_CURRENT_TEST_FILE": str(outside_path)},
    )

    current_test = repro["pytest"]["current_test_file"]
    assert current_test["rejected"] == "noncanonical"
    assert current_test["canonical_root"] == str(current_root)


def test_repro_context_includes_bounded_host_control_plane(monkeypatch, tmp_path: Path) -> None:
    long_command = "/Applications/Codex.app/Contents/MacOS/Codex " + ("x" * 800)
    samples = {
        10: memory_guard.ProcessSample(
            pid=10,
            ppid=1,
            pgid=10,
            rss_kb=500_000,
            command=long_command,
        ),
        11: memory_guard.ProcessSample(
            pid=11,
            ppid=10,
            pgid=10,
            rss_kb=200_000,
            command="/Users/adpena/Projects/molt/target/release-fast/molt-backend",
        ),
        999: memory_guard.ProcessSample(
            pid=999,
            ppid=10,
            pgid=999,
            rss_kb=10,
            command="python tools/memory_guard.py",
        ),
    }
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)
    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(memory_guard.os, "getppid", lambda: 10)
    monkeypatch.setattr(memory_guard, "_safe_getpgrp", lambda: 999)

    repro = memory_guard.repro_context_payload(
        command=[sys.executable, "-m", "pytest"],
        cwd=tmp_path,
        environ={},
    )

    host = repro["host_control_plane"]
    assert host["host_pgids"] == [10]
    assert 10 in host["protected_pgids"]
    assert host["samples"][0]["pid"] == 10
    assert host["samples"][0]["command"].endswith("...<truncated>")
    assert len(host["samples"][0]["command"]) < len(long_command)


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


def test_run_guarded_marks_child_environment_as_guarded() -> None:
    result = memory_guard.run_guarded(
        [
            sys.executable,
            "-c",
            (
                "import os; "
                "print(os.environ.get('MOLT_MEMORY_GUARD_ACTIVE')); "
                "print(bool(os.environ.get('MOLT_MEMORY_GUARD_PID')))"
            ),
        ],
        max_rss_kb=512 * 1024,
        max_total_rss_kb=1024 * 1024,
        poll_interval=0.01,
        child_rlimit_kb=None,
    )

    assert result.returncode == 0
    assert result.stdout.splitlines() == ["1", "True"]


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


def test_resolve_relative_executable_leaves_absolute_and_bare_names() -> None:
    # Absolute paths and bare program names (no separator) are untouched so
    # PATH lookup still works and an explicit absolute command is preserved.
    absolute = [sys.executable, "-c", "print('x')"]
    assert memory_guard._resolve_relative_executable(absolute) == absolute
    bare = ["python3", "-c", "print('x')"]
    assert memory_guard._resolve_relative_executable(bare) == bare
    assert memory_guard._resolve_relative_executable([]) == []


def test_resolve_relative_executable_resolves_against_parent_cwd(
    monkeypatch, tmp_path
) -> None:
    rel_dir = tmp_path / "relbin"
    rel_dir.mkdir()
    rel_interp = rel_dir / "python3"
    rel_interp.symlink_to(Path(sys.executable).resolve())
    monkeypatch.chdir(tmp_path)

    resolved = memory_guard._resolve_relative_executable(
        ["relbin/python3", "-c", "print('x')"]
    )

    assert resolved[0] == str(rel_interp.resolve())
    assert resolved[1:] == ["-c", "print('x')"]


def test_resolve_relative_executable_skips_nonexistent_relative_path(
    monkeypatch, tmp_path
) -> None:
    # A relative path that does not exist under the parent cwd is left as-is so
    # an intentionally child-relative command is never clobbered.
    monkeypatch.chdir(tmp_path)
    command = ["does/not/exist", "arg"]
    assert memory_guard._resolve_relative_executable(command) == command


@pytest.mark.skipif(
    sys.platform.startswith("win"),
    reason="relative venv interpreter symlink chain is a POSIX concern",
)
def test_run_guarded_execs_relative_interpreter_with_other_cwd(
    monkeypatch, tmp_path
) -> None:
    rel_dir = tmp_path / "relbin"
    rel_dir.mkdir()
    rel_interp = rel_dir / "python3"
    rel_interp.symlink_to(Path(sys.executable).resolve())
    other_cwd = tmp_path / "elsewhere"
    other_cwd.mkdir()
    monkeypatch.chdir(tmp_path)

    result = memory_guard.run_guarded(
        ["relbin/python3", "-c", "print('relrun')"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        cwd=str(other_cwd),
        child_rlimit_kb=1_000_000,
    )

    assert result.returncode == 0
    assert result.stdout == "relrun\n"


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
    assert payload["peak"]["scope"] in {"process", "process_rusage"}
    assert payload["peak_total"]["rss_kb"] >= payload["peak"]["rss_kb"]
    assert payload["peak_total"]["scope"] in {"process_tree", "process_tree_rusage"}
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
