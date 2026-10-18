"""The guard measures each pytest worker's subtree and each test's peak."""

from __future__ import annotations

import itertools
import json
import os
from pathlib import Path
import sys

from tools import memory_guard
from tools.memory_guard_core.process_model import ProcessSample
from tools.memory_guard_core.worker_peaks import WorkerPeakTracker

ROOT = Path(__file__).resolve().parents[2]
CURRENT_TEST_ENV = "MOLT_PYTEST_CURRENT_TEST_FILE"


def _custody(tmp_path: Path) -> tuple[dict[str, str], Path]:
    """A child environment whose pytest custody root is under tmp_path."""
    environ = {
        "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path / "state"),
        CURRENT_TEST_ENV: str(
            tmp_path / "pytest-memory-guard" / "run_current-test.json"
        ),
    }
    aggregate = Path(environ[CURRENT_TEST_ENV])
    (aggregate.parent / f"{aggregate.name}.d").mkdir(parents=True)
    return environ, aggregate


# Each publication gets a later modification time, as each real phase does.
_STAMPS = itertools.count(1_700_000_000_000_000_000, 1_000_000)


def _publish(
    path: Path, *, pid: int, phase: str, nodeid: str, controller: bool = False
) -> None:
    record = {
        "schema_version": 1,
        "pid": pid,
        "phase": phase,
        "nodeid": nodeid,
        "xdist_controller": controller,
    }
    path.write_text(json.dumps(record) + "\n", encoding="utf-8")
    stamp = next(_STAMPS)
    os.utime(path, ns=(stamp, stamp))


def _sample(pid: int, ppid: int, rss_kb: int) -> ProcessSample:
    return ProcessSample(pid=pid, ppid=ppid, rss_kb=rss_kb, command=f"p{pid}")


def test_each_worker_subtree_is_its_unit_and_each_test_its_peak(
    tmp_path: Path,
) -> None:
    environ, aggregate = _custody(tmp_path)
    tracker = WorkerPeakTracker.for_child(
        environ, current_test_file_env=CURRENT_TEST_ENV, root=ROOT
    )
    assert tracker is not None
    workers = aggregate.parent / f"{aggregate.name}.d"
    gw0, gw1 = (
        workers / "gw0-11_current-test.json",
        workers / "gw1-12_current-test.json",
    )
    # The controller (10) also publishes the aggregate under xdist and marks
    # it; its subtree is the whole run, so it must never count as a unit.
    _publish(aggregate, pid=10, phase="call", nodeid="t::a", controller=True)
    _publish(gw0, pid=11, phase="call", nodeid="t::a")
    _publish(gw1, pid=12, phase="call", nodeid="t::b")
    samples = {
        1: _sample(1, 0, 5),
        10: _sample(10, 1, 100),
        11: _sample(11, 10, 200),
        12: _sample(12, 10, 300),
        13: _sample(13, 11, 1_000),  # a build test a's worker started
        14: _sample(14, 13, 500),
        15: _sample(15, 12, 50),
    }
    tracker.observe(samples, watched=set(samples))
    # gw0 finishes a and runs c; its build has exited.
    _publish(gw0, pid=11, phase="setup", nodeid="t::c")
    tracker.observe(
        {pid: s for pid, s in samples.items() if pid not in {13, 14}},
        watched={1, 10, 11, 12, 15},
    )
    summary = tracker.summary()
    assert summary is not None
    assert summary.worker_peak_kb == 200 + 1_000 + 500
    assert summary.workers_observed == 2
    assert dict(summary.top_tests) == {"t::a": 1_700, "t::b": 350, "t::c": 200}
    assert summary.top_tests[0] == ("t::a", 1_700)


def test_a_run_without_workers_is_one_unit_and_dead_workers_are_ignored(
    tmp_path: Path,
) -> None:
    environ, aggregate = _custody(tmp_path)
    tracker = WorkerPeakTracker.for_child(
        environ, current_test_file_env=CURRENT_TEST_ENV, root=ROOT
    )
    assert tracker is not None
    _publish(aggregate, pid=20, phase="call", nodeid="t::serial")
    samples = {20: _sample(20, 1, 400), 21: _sample(21, 20, 100)}
    tracker.observe(samples, watched={20, 21})
    summary = tracker.summary()
    assert summary is not None
    assert (summary.worker_peak_kb, summary.workers_observed) == (500, 1)
    # A stale worker record whose pid left the tree contributes nothing.
    workers = aggregate.parent / f"{aggregate.name}.d"
    _publish(workers / "gw0-99_current-test.json", pid=99, phase="call", nodeid="t::x")
    stale = WorkerPeakTracker.for_child(
        environ, current_test_file_env=CURRENT_TEST_ENV, root=ROOT
    )
    assert stale is not None
    stale.observe(samples, watched={20, 21})
    assert stale.summary() == summary


def test_a_child_without_pytest_custody_has_no_tracker(tmp_path: Path) -> None:
    assert (
        WorkerPeakTracker.for_child(
            {}, current_test_file_env=CURRENT_TEST_ENV, root=ROOT
        )
        is None
    )
    # A record path outside the custody root is never read.
    assert (
        WorkerPeakTracker.for_child(
            {CURRENT_TEST_ENV: str(tmp_path / "elsewhere.json")},
            current_test_file_env=CURRENT_TEST_ENV,
            root=ROOT,
        )
        is None
    )


def test_a_guarded_xdist_run_reports_its_workers(tmp_path: Path) -> None:
    """The real current-test writer and the guard agree on the protocol."""
    target = tmp_path / "test_two.py"
    target.write_text(
        # Each test runs long enough for several 50 ms samples to see it.
        "import time\n\n\ndef test_one():\n    time.sleep(0.5)\n\n\n"
        "def test_two():\n    time.sleep(0.5)\n",
        encoding="utf-8",
    )
    # The repository config is the only one pytest custody admits; it loads
    # the current-test writer this test exercises.
    command = [
        sys.executable,
        "-m",
        "pytest",
        "-q",
        "--strict-config",
        "-W",
        "error",
        "-c",
        str(ROOT / "pyproject.toml"),
        "-n",
        "2",
        str(target),
    ]
    env = memory_guard.test_custody_launch_env(command, environ=os.environ, cwd=ROOT)
    result = memory_guard.run_guarded(
        command,
        env=env,
        cwd=ROOT,
        max_rss_kb=8 * 1024 * 1024,
        poll_interval=0.05,
        capture_output=True,
        timeout=300,
    )
    assert result.returncode == 0, (result.stdout, result.stderr)
    assert result.worker_peaks is not None
    aggregate = Path(env[CURRENT_TEST_ENV])
    records = aggregate.with_name(f"{aggregate.name}.d")
    published = sorted(path.name for path in records.iterdir())
    assert result.worker_peaks.workers_observed == 2, (aggregate, published)
    assert 0 < result.worker_peaks.worker_peak_kb <= result.peak_total.rss_kb
    # pytest spells a test outside its rootdir as "::name".
    assert {
        nodeid.rpartition("::")[2] for nodeid, _ in result.worker_peaks.top_tests
    } == {"test_one", "test_two"}
