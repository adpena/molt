from __future__ import annotations

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


def test_descendant_pids_includes_grandchildren() -> None:
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root"),
        101: memory_guard.ProcessSample(101, 100, 20, "child"),
        102: memory_guard.ProcessSample(102, 101, 30, "grandchild"),
        200: memory_guard.ProcessSample(200, 1, 999_999, "unrelated"),
    }

    assert memory_guard.descendant_pids(samples, 100) == {100, 101, 102}


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


def test_max_rss_gb_must_leave_margin_below_thirty() -> None:
    with pytest.raises(ValueError, match="below 30"):
        memory_guard.max_rss_kb_from_gb(30)


def test_run_command_passes_through_success() -> None:
    result = memory_guard.run_guarded(
        [sys.executable, "-c", "print('ok')"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
    )

    assert result.returncode == 0
    assert result.violation is None
    assert result.stdout == "ok\n"


def test_run_command_ignores_samples_without_root_pid() -> None:
    def sampler() -> dict[int, memory_guard.ProcessSample]:
        return {
            999_999: memory_guard.ProcessSample(999_999, 1, 1, "missing-root"),
        }

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "print('ok')"],
        max_rss_kb=1,
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


def test_main_rejects_unsafe_threshold(capsys: pytest.CaptureFixture[str]) -> None:
    rc = memory_guard.main(["--max-rss-gb", "30", "--", sys.executable, "-c", "pass"])

    assert rc == 2
    assert "below 30" in capsys.readouterr().err
