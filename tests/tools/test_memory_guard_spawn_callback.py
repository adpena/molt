"""Post-custody callback and allocation telemetry contract tests."""

from __future__ import annotations

import sys
import time

import pytest

from tools import memory_guard


def _run(command: list[str], *, on_spawn=None):
    return memory_guard.run_guarded(
        command,
        max_rss_kb=512 * 1024,
        max_total_rss_kb=768 * 1024,
        poll_interval=0.02,
        capture_output=True,
        cleanup_orphans=False,
        on_spawn=on_spawn,
    )


def test_on_spawn_observes_custodied_child_and_result_owns_tree_peak() -> None:
    spawned: list[int] = []

    result = _run(
        [sys.executable, "-c", "print('ok')"],
        on_spawn=spawned.append,
    )

    assert result.returncode == 0
    assert len(spawned) == 1
    assert result.child_process is not None
    assert spawned == [result.child_process.pid]
    assert result.peak_total is not None
    if sys.platform.startswith("win"):
        assert result.peak_job_commit_bytes is not None
        assert result.peak_job_commit_bytes > 0
    else:
        assert result.peak_job_commit_bytes is None


def test_on_spawn_failure_reaps_the_owned_child_tree() -> None:
    spawned: list[int] = []

    def fail_after_custody(pid: int) -> None:
        spawned.append(pid)
        raise RuntimeError("forced callback failure")

    with pytest.raises(RuntimeError, match="forced callback failure"):
        _run(
            [sys.executable, "-c", "import time; time.sleep(60)"],
            on_spawn=fail_after_custody,
        )

    assert len(spawned) == 1
    for _ in range(100):
        if spawned[0] not in memory_guard.sample_processes():
            break
        time.sleep(0.01)
    assert spawned[0] not in memory_guard.sample_processes()


@pytest.mark.skipif(
    not sys.platform.startswith("win"),
    reason="exact Job Object completion is Windows-only",
)
def test_run_guarded_does_not_return_with_live_job_descendants(tmp_path) -> None:
    pidfile = tmp_path / "grandchild.pid"
    child = (
        "import pathlib,subprocess,sys;"
        "gc=subprocess.Popen([sys.executable,'-c','import time;time.sleep(120)']);"
        "pathlib.Path(sys.argv[1]).write_text(str(gc.pid),encoding='utf-8')"
    )

    result = _run([sys.executable, "-c", child, str(pidfile)])

    assert result.returncode == 0
    assert pidfile.read_text(encoding="utf-8").strip()
    assert result.windows_job_cleanup is not None
    assert result.windows_job_cleanup.completed
    assert result.windows_job_cleanup.terminated_remaining_processes
    assert result.windows_job_cleanup.before.active_processes >= 1
    assert result.windows_job_cleanup.after.active_processes == 0


@pytest.mark.skipif(not sys.platform.startswith("win"), reason="Windows Job telemetry")
def test_windows_job_sampling_automatically_avoids_global_process_table() -> None:
    global_samples = 0

    def forbidden_global_sampler():
        nonlocal global_samples
        global_samples += 1
        raise AssertionError("owned-tree measurement must not scan the whole host")

    result = memory_guard.run_guarded(
        [
            sys.executable,
            "-c",
            "import time; x=bytearray(8_000_000); time.sleep(0.1)",
        ],
        max_rss_kb=512 * 1024,
        max_total_rss_kb=768 * 1024,
        poll_interval=0.003,
        sampler=forbidden_global_sampler,
        capture_output=True,
        cleanup_orphans=True,
    )

    assert result.returncode == 0
    assert global_samples == 0
    assert result.peak_total is not None
    assert result.peak_total.rss_kb > 4_000
    assert result.peak_job_commit_bytes is not None
    assert result.peak_job_commit_bytes > 0
    assert result.sampling_telemetry is not None
    assert result.sampling_telemetry.source == "windows_job_members"
    assert result.sampling_telemetry.process_rows > 0
    assert result.sampling_telemetry.max_process_rows < 16
    assert result.sampling_telemetry.observer_wall_time_s > 0.0
    assert 0.0 <= result.sampling_telemetry.observer_cpu_duty_cycle <= 1.0


@pytest.mark.skipif(not sys.platform.startswith("win"), reason="Windows Job telemetry")
def test_windows_job_sampling_preserves_process_and_tree_rss_semantics() -> None:
    grandchild = "import time; x=bytearray(20_000_000); time.sleep(0.3)"
    child = (
        "import subprocess,sys,time;"
        "x=bytearray(20_000_000);"
        "p=subprocess.Popen([sys.executable,'-c',sys.argv[1]]);"
        "time.sleep(0.3);p.wait()"
    )

    result = memory_guard.run_guarded(
        [sys.executable, "-c", child, grandchild],
        max_rss_kb=256 * 1024,
        max_total_rss_kb=512 * 1024,
        poll_interval=0.01,
        capture_output=True,
    )

    assert result.returncode == 0
    assert result.peak is not None
    assert result.peak_total is not None
    assert result.peak.scope == "process"
    assert result.peak_total.scope == "process_tree"
    assert result.peak_total.rss_kb > result.peak.rss_kb
    assert result.sampling_telemetry is not None
    assert result.sampling_telemetry.max_process_rows >= 2


@pytest.mark.skipif(not sys.platform.startswith("win"), reason="Windows Job telemetry")
def test_windows_job_sampling_tolerates_proven_member_exit_churn() -> None:
    churn = (
        "import subprocess,sys;"
        "[subprocess.run([sys.executable,'-c','pass'],check=True) for _ in range(40)]"
    )

    result = memory_guard.run_guarded(
        [sys.executable, "-c", churn],
        max_rss_kb=256 * 1024,
        max_total_rss_kb=512 * 1024,
        poll_interval=0.002,
        capture_output=True,
    )

    assert result.returncode == 0
    assert result.violation is None
    assert result.sampling_telemetry is not None
    assert result.sampling_telemetry.source == "windows_job_members"
    assert result.sampling_telemetry.enforcement_complete
