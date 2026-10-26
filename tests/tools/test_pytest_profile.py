"""The pytest profile record measures what bounds a run's wall clock."""

from __future__ import annotations

import json
import os
from pathlib import Path
import sys

from tests.process_guard_common import current_thread_only, run_guarded_test_process
from tools import pytest_profile

ROOT = Path(__file__).resolve().parents[2]


def _phase(
    nodeid: str,
    when: str,
    *,
    worker: str,
    start: float,
    duration: float,
    cpu: float = 0.0,
    child_cpu: float = 0.0,
    spawns: int = 0,
    outcome: str = "passed",
) -> dict:
    return {
        "nodeid": nodeid,
        "when": when,
        "outcome": outcome,
        "duration": duration,
        "start": start,
        "stop": start + duration,
        "worker": worker,
        "cpu": cpu,
        "child_cpu": child_cpu,
        "spawns": spawns,
    }


def _profile() -> dict:
    return {
        "schema": pytest_profile.SCHEMA,
        "worker": None,
        "argv": ["pytest"],
        "numprocesses": 2,
        "session_start": 1000.0,
        "collection_finish": 1002.0,
        "session_finish": 1020.0,
        "exit_at": 1025.0,
        "worker_records": {
            "gw0": {"worker": "gw0", "session_finish": 1019.0, "exit_at": 1023.5},
            "gw1": {"worker": "gw1", "session_finish": 1019.5, "exit_at": 1020.0},
        },
        "phases": [
            _phase(
                "tests/a.py::t1",
                "setup",
                worker="gw0",
                start=1002.0,
                duration=4.0,
                cpu=0.5,
                child_cpu=1.0,
                spawns=2,
            ),
            _phase(
                "tests/a.py::t1",
                "call",
                worker="gw0",
                start=1006.0,
                duration=6.0,
                cpu=1.0,
                spawns=1,
            ),
            _phase(
                "tests/a.py::t1", "teardown", worker="gw0", start=1012.0, duration=1.0
            ),
            _phase("tests/b.py::t2", "setup", worker="gw1", start=1002.0, duration=0.5),
            _phase(
                "tests/b.py::t2",
                "call",
                worker="gw1",
                start=1002.5,
                duration=2.5,
                cpu=2.5,
                outcome="failed",
            ),
            _phase(
                "tests/b.py::t2", "teardown", worker="gw1", start=1005.0, duration=0.0
            ),
            _phase("tests/b.py::t3", "setup", worker="gw1", start=1005.0, duration=0.0),
            _phase(
                "tests/b.py::t3",
                "call",
                worker="gw1",
                start=1005.0,
                duration=3.0,
                cpu=1.0,
            ),
            _phase(
                "tests/b.py::t3", "teardown", worker="gw1", start=1008.0, duration=0.0
            ),
        ],
    }


def test_summary_measures_wall_phases_balance_waits_and_exit_tails() -> None:
    summary = pytest_profile.summarize(_profile())
    assert summary["wall"] == 20.0
    assert summary["collection"] == 2.0
    assert summary["exit_tail"] == 5.0
    assert summary["worker_exit_tail_max"] == 4.5
    assert (summary["tests"], summary["failed"], summary["workers"]) == (3, 1, 2)
    assert summary["sum_total"] == 17.0
    assert (summary["sum_setup"], summary["sum_call"], summary["sum_teardown"]) == (
        4.5,
        11.5,
        1.0,
    )
    # t1 waits 11 - 1.5 own - 1.0 child = 8.5; t2 waits 3 - 2.5 = 0.5; t3 waits 2.
    assert summary["sum_waiting"] == 11.0
    assert summary["spawns"] == 3
    assert summary["busiest_worker"] == 11.0
    assert summary["mean_worker"] == 8.5
    assert abs(summary["imbalance"] - (11.0 / 8.5 - 1.0)) < 1e-9
    assert summary["per_worker"]["gw1"]["span"] == 6.0
    assert summary["per_file"]["tests/b.py"] == {
        "total": 6.0,
        "setup": 0.5,
        "tests": 2,
        "spawns": 0,
    }
    rows = {row["nodeid"]: row for row in summary["test_rows"]}
    assert rows["tests/a.py::t1"]["waiting"] == 8.5
    assert rows["tests/b.py::t2"]["waiting"] == 0.5
    assert rows["tests/b.py::t2"]["outcome"] == "failed"


def test_report_names_the_slowest_waiting_setup_file_and_tail_rows() -> None:
    text = "\n".join(pytest_profile.report_lines(_profile(), top=1))
    assert "3 tests (1 failed) on 2 worker(s) (-n 2)" in text
    assert "wall 20.0s: collection 2.0s, exit tail after the session 5.0s" in text
    assert "slowest worker exit 4.5s" in text
    assert "busiest 11.0s, mean 8.5s (+29.4%)" in text
    slowest = text.split("slowest 1 tests:\n", 1)[1].splitlines()[0]
    assert "tests/a.py::t1" in slowest and "spawns   3" in slowest
    waiting = text.split("most waiting", 1)[1].splitlines()[1]
    assert "tests/a.py::t1" in waiting and "wait    8.5" in waiting
    setups = text.split("slowest setups, top 1:\n", 1)[1].splitlines()[0]
    assert "setup    4.0" in setups
    files = text.split("files by test time, top 1:\n", 1)[1].splitlines()[0]
    assert files.strip().endswith("tests/a.py")
    # The last 10 s of a 20 s session: t1 ended at 1013, t3 at 1008.
    tail = text.split("last stretch:", 1)[1]
    assert "tests/a.py::t1" in tail and "tests/b.py::t3" not in tail


def test_compare_reports_the_deltas_per_file_and_per_test() -> None:
    before = _profile()
    after = _profile()
    after["session_finish"] = 1012.0
    after["exit_at"] = 1012.5
    after["phases"] = [
        phase
        for phase in after["phases"]
        if not (phase["nodeid"] == "tests/a.py::t1" and phase["when"] == "setup")
    ]
    text = "\n".join(pytest_profile.compare_lines(before, after, top=5))
    assert "wall 20.0 -> 12.0 (-8.0)" in text
    assert "exit_tail 5.0 -> 0.5 (-4.5)" in text
    assert "sum_setup 4.5 -> 0.5 (-4.0)" in text
    assert "spawns 3 -> 1" in text
    assert "   11.0s ->     7.0s (   -4.0)  tests/a.py" in text
    assert "   11.0s ->     7.0s (   -4.0)  tests/a.py::t1" in text


def test_plugin_records_every_workers_phases_spawns_waits_and_exit_tails(
    tmp_path: Path,
) -> None:
    project = tmp_path / "profiled-project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[tool.pytest.ini_options]\naddopts = ""\n', encoding="utf-8"
    )
    (project / "test_profiled.py").write_text(
        "import subprocess, sys, time\n"
        "import pytest\n"
        "@pytest.fixture\n"
        "def slow_setup():\n"
        "    time.sleep(0.3)\n"
        "def test_spawns(slow_setup):\n"
        "    subprocess.run([sys.executable, '-c', 'pass'], check=True)\n"
        "def test_sleeps():\n"
        "    time.sleep(0.4)\n"
        "@pytest.mark.parametrize('index', range(6))\n"
        "def test_quick(index):\n"
        "    assert index >= 0\n",
        encoding="utf-8",
    )
    record = tmp_path / "profile.json"
    environment = dict(os.environ)
    environment["PYTHONPATH"] = os.pathsep.join([str(ROOT / "src"), str(ROOT)])
    # The child is its own session, not this run's xdist worker.
    for key in (
        "PYTEST_ADDOPTS",
        "PYTEST_XDIST_WORKER",
        "PYTEST_XDIST_WORKER_COUNT",
        "PYTEST_XDIST_TESTRUNUID",
    ):
        environment.pop(key, None)
    result = run_guarded_test_process(
        [
            sys.executable,
            "-m",
            "pytest",
            "-q",
            "-p",
            "tools.pytest_profile",
            "-p",
            "no:cacheprovider",
            f"--molt-profile={record}",
            "-n",
            "2",
            str(project / "test_profiled.py"),
        ],
        cwd=project,
        env=environment,
        timeout=180.0,
    )
    assert result.returncode == 0, result.stdout + result.stderr

    profile = pytest_profile.load(record)
    assert profile["numprocesses"] == 2
    assert sorted(profile["worker_records"]) == ["gw0", "gw1"]
    for worker in profile["worker_records"].values():
        assert worker["exit_at"] >= worker["session_finish"] > 0
    summary = pytest_profile.summarize(profile)
    assert summary["tests"] == 8 and summary["failed"] == 0
    assert summary["workers"] == 2
    assert summary["exit_tail"] >= 0
    assert summary["collection"] > 0
    rows = {row["nodeid"].split("::", 1)[1]: row for row in summary["test_rows"]}
    assert set(rows) == {
        "test_spawns",
        "test_sleeps",
        *(f"test_quick[{i}]" for i in range(6)),
    }
    assert rows["test_spawns"]["spawns"] == 1
    assert rows["test_spawns"]["setup"] >= 0.3
    assert rows["test_spawns"]["child_cpu"] > 0
    assert rows["test_sleeps"]["spawns"] == 0
    assert rows["test_sleeps"]["call"] >= 0.4
    assert rows["test_sleeps"]["waiting"] >= 0.35
    assert all(row["cpu"] >= 0 for row in rows.values())
    assert {row["worker"] for row in rows.values()} == {"gw0", "gw1"}
    assert all(row["stop"] >= row["start"] > 0 for row in rows.values())

    report = "\n".join(pytest_profile.report_lines(profile, top=3))
    assert "8 tests (0 failed) on 2 worker(s) (-n 2)" in report
    assert "test_sleeps" in report.split("most waiting", 1)[1]

    # The record is also readable through the command line.
    listing = run_guarded_test_process(
        [
            sys.executable,
            str(ROOT / "tools" / "pytest_profile.py"),
            "report",
            str(record),
        ],
        cwd=ROOT,
        env=environment,
        timeout=120.0,
    )
    assert listing.returncode == 0, listing.stdout + listing.stderr
    assert "8 tests (0 failed)" in listing.stdout
    assert (
        json.loads(record.read_text(encoding="utf-8"))["schema"]
        == pytest_profile.SCHEMA
    )


def _reported_phase(nodeid: str, when: str):
    from types import SimpleNamespace

    return SimpleNamespace(
        nodeid=nodeid,
        when=when,
        outcome="passed",
        duration=0.25,
        start=1000.0,
        stop=1000.25,
        molt_profile={"cpu": 0.05, "child_cpu": 0.0, "spawns": 0},
    )


def _checkpoint_profiler(monkeypatch, path: Path):
    from types import SimpleNamespace

    # Replace the profiler's module binding, never time's process-wide methods.
    real_time = pytest_profile.time
    clock = [0.0]
    monkeypatch.setattr(
        pytest_profile,
        "time",
        SimpleNamespace(
            time=real_time.time,
            process_time=real_time.process_time,
            monotonic=lambda: clock[0],
        ),
    )
    monkeypatch.setattr(pytest_profile.SpawnCounter, "install", lambda self: None)
    profiler = pytest_profile.Profiler(path, worker=None)
    session = SimpleNamespace(
        config=SimpleNamespace(option=SimpleNamespace(numprocesses=0))
    )
    return profiler, session, clock


def test_checkpoints_bound_publications_and_preserve_the_report_prefix(
    tmp_path: Path, monkeypatch
) -> None:
    from molt import artifact_publication

    path = tmp_path / "profile.json"
    profiler, session, clock = _checkpoint_profiler(monkeypatch, path)
    real_write = artifact_publication.atomic_write_json
    writes = []

    def observe_write(destination, payload):
        real_write(destination, payload)
        raw = destination.read_bytes()
        writes.append((len(json.loads(raw)["phases"]), len(raw)))

    monkeypatch.setattr(
        artifact_publication,
        "atomic_write_json",
        current_thread_only(observe_write, real_write),
    )
    profiler.pytest_sessionstart(session)
    profiler.pytest_runtest_logreport(_reported_phase("test_first", "setup"))
    clock[0] = 9.0
    for index in range(100):
        profiler.pytest_runtest_logreport(_reported_phase(f"test_fast_{index}", "call"))
    assert [count for count, _ in writes] == [0, 1]
    clock[0] = 10.0
    profiler.pytest_runtest_logreport(_reported_phase("test_first", "teardown"))
    assert [count for count, _ in writes] == [0, 1, 102]
    assert sum(size for _, size in writes) < 100_000
    assert max(size for _, size in writes) > 10_000
    partial = json.loads(path.read_text(encoding="utf-8"))
    assert [(row["nodeid"], row["when"]) for row in partial["phases"]] == [
        ("test_first", "setup"),
        *((f"test_fast_{index}", "call") for index in range(100)),
        ("test_first", "teardown"),
    ]
    assert partial["session_finish"] is None and partial["exit_at"] is None

    from types import SimpleNamespace

    exits = []
    monkeypatch.setattr(
        pytest_profile, "atexit", SimpleNamespace(register=exits.append)
    )
    profiler.pytest_sessionfinish(session, 0)
    assert len(writes) == 4
    finished = pytest_profile.load(path)
    assert finished["session_finish"] is not None
    assert finished["exit_at"] is None
    finished_summary = pytest_profile.summarize(finished)
    assert finished_summary["complete"] is True
    assert finished_summary["exit_tail"] is None
    assert len(exits) == 1
    exits[0]()
    assert len(writes) == 5
    assert json.loads(path.read_text(encoding="utf-8"))["exit_at"] is not None


def test_checkpoint_publication_failure_preserves_prior_valid_snapshot(
    tmp_path: Path, monkeypatch
) -> None:
    import pytest
    from molt import artifact_publication

    path = tmp_path / "profile.json"
    profiler, session, clock = _checkpoint_profiler(monkeypatch, path)
    profiler.pytest_sessionstart(session)
    profiler.pytest_runtest_logreport(_reported_phase("test_one", "setup"))
    before = path.read_bytes()
    failure = OSError("independent publication refusal")

    def refuse_publish(*args, **kwargs):
        raise failure

    # Exercise actual serialization/staging; fail at the owning replace boundary.
    monkeypatch.setattr(
        artifact_publication,
        "_durable_replace",
        current_thread_only(refuse_publish, artifact_publication._durable_replace),
    )
    clock[0] = 12.0
    with pytest.raises(OSError) as caught:
        profiler.pytest_runtest_logreport(_reported_phase("test_one", "call"))
    assert caught.value is failure
    assert path.read_bytes() == before
    retained = pytest_profile.load(path)
    assert [(row["nodeid"], row["when"]) for row in retained["phases"]] == [
        ("test_one", "setup")
    ]
    assert retained["session_finish"] is None


def test_incomplete_snapshot_reports_observations_without_claiming_completion(
    tmp_path: Path,
) -> None:
    import pytest

    profile = _profile()
    profile["session_finish"] = None
    profile["exit_at"] = None
    profile["phases"] = profile["phases"][:2]
    path = tmp_path / "profile.json"
    path.write_text(json.dumps(profile), encoding="utf-8")
    # An unrelated old worker file must not invent an exit tail for this run.
    path.with_name("profile.json.worker-gw9.json").write_text(
        "not JSON", encoding="utf-8"
    )
    loaded = pytest_profile.load(path)
    summary = pytest_profile.summarize(loaded)
    assert summary["complete"] is False
    assert summary["wall"] is None and summary["exit_tail"] is None
    assert summary["worker_exit_tail_max"] is None
    assert summary["incomplete_tests"] == 1
    assert summary["test_rows"][0]["outcome"] == "incomplete"
    assert summary["sum_setup"] == 4.0 and summary["sum_call"] == 6.0
    text = "\n".join(pytest_profile.report_lines(loaded))
    assert text.startswith("INCOMPLETE pytest profile:")
    assert "observed phase-duration totals are lower bounds" in text
    assert "derived waiting and imbalance are provisional" in text
    assert "tests/a.py::t1" in text
    with pytest.raises(ValueError, match="cannot compare incomplete"):
        pytest_profile.compare_lines(loaded, _profile())
    with pytest.raises(ValueError, match="cannot compare incomplete"):
        pytest_profile.compare_lines(_profile(), loaded)
    with pytest.raises(SystemExit) as exit_error:
        pytest_profile.main(["compare", str(path), str(path)])
    assert exit_error.value.code == 2


def test_abrupt_pytest_exit_retains_completed_phase_checkpoint(tmp_path: Path) -> None:
    project = tmp_path / "interrupted-project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[tool.pytest.ini_options]\naddopts = ""\n', encoding="utf-8"
    )
    (project / "conftest.py").write_text(
        "import os, pytest\n"
        "@pytest.hookimpl(tryfirst=True)\n"
        "def pytest_runtest_logreport(report):\n"
        "    if report.when == 'call':\n"
        "        os._exit(23)\n",
        encoding="utf-8",
    )
    marker = project / "must-not-run"
    (project / "test_interrupted.py").write_text(
        "from pathlib import Path\n"
        "def test_first():\n"
        "    assert 7 * 9 == 63\n"
        "def test_second():\n"
        f"    Path({str(marker)!r}).write_text('ran', encoding='utf-8')\n",
        encoding="utf-8",
    )
    record = tmp_path / "interrupted.json"
    environment = dict(os.environ)
    environment["PYTHONPATH"] = os.pathsep.join([str(ROOT / "src"), str(ROOT)])
    for key in (
        "PYTEST_ADDOPTS",
        "PYTEST_XDIST_WORKER",
        "PYTEST_XDIST_WORKER_COUNT",
        "PYTEST_XDIST_TESTRUNUID",
    ):
        environment.pop(key, None)
    result = run_guarded_test_process(
        [
            sys.executable,
            "-m",
            "pytest",
            "-q",
            "-p",
            "tools.pytest_profile",
            "-p",
            "no:cacheprovider",
            f"--molt-profile={record}",
            "-n",
            "0",
            str(project / "test_interrupted.py"),
        ],
        cwd=project,
        env=environment,
        timeout=120.0,
    )
    assert result.returncode == 23, result.stdout + result.stderr
    assert not marker.exists()
    profile = pytest_profile.load(record)
    assert profile["session_finish"] is None and profile["exit_at"] is None
    assert [
        (row["nodeid"], row["when"], row["outcome"]) for row in profile["phases"]
    ] == [("test_interrupted.py::test_first", "setup", "passed")]
    assert pytest_profile.summarize(profile)["complete"] is False
    assert "INCOMPLETE pytest profile:" in "\n".join(
        pytest_profile.report_lines(profile)
    )


def test_xdist_worker_does_not_publish_controller_phase_checkpoints(
    tmp_path: Path, monkeypatch
) -> None:
    from types import SimpleNamespace

    path = tmp_path / "profile.json"
    profiler, session, clock = _checkpoint_profiler(monkeypatch, path)
    profiler.worker = "gw2"
    monkeypatch.setattr(
        pytest_profile, "atexit", SimpleNamespace(register=lambda fn: None)
    )
    profiler.pytest_sessionstart(session)
    clock[0] = 30.0
    profiler.pytest_runtest_logreport(_reported_phase("test_worker", "call"))
    assert not path.exists()
    assert not path.with_name("profile.json.worker-gw2.json").exists()
    profiler.pytest_sessionfinish(session, 0)
    assert not path.exists()
    worker = json.loads(
        path.with_name("profile.json.worker-gw2.json").read_text(encoding="utf-8")
    )
    assert worker["worker"] == "gw2"
    assert worker["session_finish"] is not None
    assert "phases" not in worker
