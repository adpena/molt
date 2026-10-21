"""Post-custody callback and allocation telemetry contract tests."""

from __future__ import annotations

import os
import sys
import time

import pytest

from tools import memory_guard
from tests.process_guard_common import HUNG_CHILD_SOURCE, run_in_fresh_guard_observer


def _run(command: list[str], *, on_spawn=None):
    # Default orphan cleanup: on POSIX it is the scratch-closure authority, so
    # disabling it makes every successful child an infrastructure failure.
    # A Windows Job supersedes it inside the guard.
    return memory_guard.run_guarded(
        command,
        max_rss_kb=512 * 1024,
        max_total_rss_kb=768 * 1024,
        poll_interval=0.02,
        capture_output=True,
        on_spawn=on_spawn,
    )


def test_on_spawn_observes_custodied_child_and_result_owns_tree_peak(request) -> None:
    if run_in_fresh_guard_observer(request):
        return
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
            [sys.executable, "-c", HUNG_CHILD_SOURCE],
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
        f"gc=subprocess.Popen([sys.executable,'-c',{HUNG_CHILD_SOURCE!r}]);"
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
    duty_cycle = result.sampling_telemetry.observer_cpu_duty_cycle
    if result.sampling_telemetry.observer_cpu_time_s > 0.0:
        assert duty_cycle is not None and 0.0 < duty_cycle <= 1.0
    else:
        # A coarse Windows CPU clock can read zero; the share is unmeasurable.
        assert duty_cycle is None


@pytest.mark.skipif(
    not memory_guard._process_model.posix_custody_reads_partial_rows(),
    reason="this host reads the process table for custody",
)
@pytest.mark.parametrize(
    "sample_delay_s", [0.0, 0.5], ids=["host-speed", "throttled-sampler"]
)
def test_guard_polls_read_the_tree_not_the_host_table(
    request, monkeypatch, tmp_path, sample_delay_s
) -> None:
    if run_in_fresh_guard_observer(request):
        return
    # Each poll once read every process of the host (about 1,300 kern.proc rows
    # on a developer Mac, or every /proc entry) to keep the few of this tree.
    held = tmp_path / "held"
    sampled = tmp_path / "sampled"
    # The child and the grandchild each hold 20 MB until a poll has sampled
    # them together. A fixed 0.3 s overlap missed the pair on a loaded host
    # (HF-425); the throttled sampler is slower than that overlap.
    grandchild = (
        "import pathlib, sys, time\n"
        "x = b'x' * 20_000_000\n"
        "pathlib.Path(sys.argv[1]).write_text('held')\n"
        "while not pathlib.Path(sys.argv[2]).exists():\n"
        "    time.sleep(0.01)\n"
    )
    child = (
        "import subprocess, sys\n"
        "x = b'x' * 20_000_000\n"
        "subprocess.run([sys.executable, '-c', *sys.argv[1:]], check=True)\n"
    )
    real_total_rss = memory_guard.total_rss
    held_seen = False

    def total_rss(samples, *args, **kwargs):
        # A poll that starts after one saw the pair hold samples them both.
        nonlocal held_seen
        if held_seen:
            sampled.touch()
        held_seen = held_seen or held.exists()
        return real_total_rss(samples, *args, **kwargs)

    monkeypatch.setattr(memory_guard, "total_rss", total_rss)
    if sample_delay_s:
        real_custody = memory_guard.sample_processes_custody

        def slow_custody(*args, **kwargs):
            time.sleep(sample_delay_s)  # A host that samples slowly.
            return real_custody(*args, **kwargs)

        monkeypatch.setattr(memory_guard, "sample_processes_custody", slow_custody)

    result = memory_guard.run_guarded(
        [sys.executable, "-c", child, grandchild, str(held), str(sampled)],
        max_rss_kb=256 * 1024,
        max_total_rss_kb=512 * 1024,
        poll_interval=0.01,
        capture_output=True,
    )

    assert result.returncode == 0
    assert result.sampling_telemetry is not None
    assert (
        result.sampling_telemetry.source
        == memory_guard._process_model.posix_custody_source()
    )
    # The tree's rows plus the guard's own lineage, never the host table.
    lineage = memory_guard._process_model.posix_custody_lineage(os.getpid())
    assert 2 <= result.sampling_telemetry.max_process_rows <= len(lineage) + 4
    assert result.peak_total is not None and result.peak is not None
    assert result.peak_total.rss_kb > result.peak.rss_kb


@pytest.mark.skipif(
    not memory_guard._process_model.posix_custody_reads_partial_rows(),
    reason="this host reads the process table for custody",
)
def test_guard_kill_decides_protection_on_the_host_table(monkeypatch) -> None:
    # Polls read custody rows; a kill re-reads the host table (samples=None)
    # before it judges which groups it may signal.
    kills: list[tuple[str, object]] = []
    original = memory_guard.terminate_watched_processes

    def recording(root_pid, **kwargs):  # noqa: ANN001, ANN003, ANN202
        kills.append((kwargs["reason"], kwargs.get("samples")))
        return original(root_pid, **kwargs)

    monkeypatch.setattr(memory_guard, "terminate_watched_processes", recording)

    result = memory_guard.run_guarded(
        [
            sys.executable,
            "-c",
            "x=bytearray(200_000_000)\n" + HUNG_CHILD_SOURCE,
        ],
        max_rss_kb=64 * 1024,
        poll_interval=0.01,
        capture_output=True,
    )

    assert result.returncode != 0
    assert result.sampling_telemetry is not None
    assert (
        result.sampling_telemetry.source
        == memory_guard._process_model.posix_custody_source()
    )
    assert ("rss_limit", None) in kills


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


def test_owner_cancellation_uses_guard_custody_and_proves_child_closure(tmp_path):
    cancellation = tmp_path / "cancel"
    result = memory_guard.run_guarded(
        [sys.executable, "-c", HUNG_CHILD_SOURCE],
        max_rss_kb=512 * 1024,
        max_total_rss_kb=768 * 1024,
        poll_interval=0.02,
        capture_output=True,
        on_spawn=lambda _pid: cancellation.touch(),
        cancellation_requested=cancellation.exists,
    )
    assert result.cancelled
    assert result.guard_signal is None
    assert not result.timed_out
    assert result.descendants_closed
    assert result.child_returncode is not None
    assert memory_guard._incident_payload(result)["reason"] == "owner_cancellation"
    if result.windows_job_cleanup is not None:
        # The Job is the exact owner on Windows; no duplicate per-PID
        # termination reports are manufactured for a successful Job cleanup.
        assert result.windows_job_cleanup.completed
        assert result.windows_job_cleanup.after.active_processes == 0
    else:
        assert any(
            report.reason == "owner_cancellation"
            for report in result.termination_reports
        )
    assert result.child_process.pid not in memory_guard.sample_processes()
