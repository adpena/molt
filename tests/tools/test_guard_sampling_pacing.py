from __future__ import annotations

from pathlib import Path
import sys

import pytest

import tools.harness_memory_guard as harness_memory_guard
import tools.memory_guard as memory_guard


def test_paced_poll_interval_keeps_configured_floor_for_cheap_sampling() -> None:
    assert memory_guard.paced_poll_interval(0.1, 0.0) == 0.1
    assert memory_guard.paced_poll_interval(0.1, -1.0) == 0.1
    # Sampling cheaper than interval / factor keeps the configured cadence,
    # so POSIX ps sampling and unit-test fake samplers see no behavior change.
    assert memory_guard.paced_poll_interval(0.1, 0.05) == 0.1
    assert memory_guard.paced_poll_interval(2.0, 0.6) == 2.0


def test_paced_poll_interval_bounds_expensive_sampling_duty_cycle() -> None:
    assert memory_guard.paced_poll_interval(0.1, 0.6) == pytest.approx(
        memory_guard.SAMPLE_COST_PACING_FACTOR * 0.6
    )
    cost = 0.75
    wait = memory_guard.paced_poll_interval(0.1, cost)
    assert cost / (cost + wait) <= 1.0 / (1.0 + memory_guard.SAMPLE_COST_PACING_FACTOR)


def test_repo_sentinel_loop_paces_waits_by_scan_cost(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    limits = harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=10,
        max_total_rss_gb=10,
        max_global_rss_gb=10,
        poll_interval=0.01,
    )
    sentinel = harness_memory_guard.repo_process_sentinel(
        repo_root=tmp_path,
        artifact_root=tmp_path,
        label="pacing-unit",
        limits=limits,
    )
    paced_calls: list[tuple[float, float]] = []
    real_paced = memory_guard.paced_poll_interval

    def recording_paced(poll_interval: float, cost: float) -> float:
        paced_calls.append((poll_interval, cost))
        return real_paced(poll_interval, cost)

    monkeypatch.setattr(
        harness_memory_guard.memory_guard,
        "paced_poll_interval",
        recording_paced,
    )

    def stop_after_scan() -> None:
        sentinel._stop.set()

    monkeypatch.setattr(sentinel, "scan_once", stop_after_scan)
    sentinel._run()

    assert len(paced_calls) == 1
    poll_interval, cost = paced_calls[0]
    assert poll_interval == limits.poll_interval
    assert cost >= 0.0


def test_run_guarded_paces_waits_by_sampler_cost(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    paced_calls: list[tuple[float, float]] = []
    real_paced = memory_guard.paced_poll_interval

    def recording_paced(poll_interval: float, cost: float) -> float:
        paced_calls.append((poll_interval, cost))
        return real_paced(poll_interval, cost)

    monkeypatch.setattr(memory_guard, "paced_poll_interval", recording_paced)

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import time; time.sleep(0.3)"],
        max_rss_kb=10 * 1024 * 1024,
        poll_interval=0.05,
        sampler=lambda: {},
        cleanup_orphans=False,
    )

    assert result.returncode == 0
    # A conftest-level repo sentinel thread may also call the shared pacing
    # authority with its own poll interval; assert on the run_guarded loop's
    # calls, identified by the distinctive poll interval passed above.
    loop_calls = [call for call in paced_calls if call[0] == 0.05]
    assert loop_calls
    assert all(cost >= 0.0 for _interval, cost in loop_calls)
