#!/usr/bin/env python3
"""Record where a pytest run spends its wall clock, and report it.

Record a run (phase measurements plus opt-in atomic checkpoints)::

    python -m pytest -p tools.pytest_profile --molt-profile=run.json -n 4 tests/x

Read the record::

    python tools/pytest_profile.py report run.json [--top 20]
    python tools/pytest_profile.py compare before.json after.json [--top 20]

The controller publishes an initial snapshot, its first completed phase, then
at most one checkpoint per ten seconds on report callbacks. A long phase can
leave the snapshot older than ten seconds; no timer or worker writes its phase
history. Normal session/exit publication remains final. Interrupted snapshots
retain only observed phases and cannot be compared as completed runs. A
session_finish records completed pytest phases; process exit can remain unknown.
Neither is proof acceptance. Use a fresh output path per run, including its
worker metadata siblings, so final reports cannot mix metadata from older runs.

A test's wall time splits three ways: its own CPU (``cpu``), the CPU of the
children it started and reaped (``child_cpu``), and the rest (``waiting``):
sleeps, polls, I/O, lock waits, children left unreaped, and time the scheduler
gave to other processes. A test whose waiting dominates is the lead for an
event wait or a smaller fixture; one whose child CPU dominates is a build or a
process-heavy fixture. Under xdist the controller receives every worker's
reports, so one record describes the whole run: wall, collection, each
worker's busy time and span, the busiest worker against the mean, the exit
tail after the session, and each test's phases, spawns and waiting.
"""

from __future__ import annotations

import argparse
import atexit
import json
from pathlib import Path
import subprocess
import sys
import time
from typing import Any

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

import pytest  # noqa: E402

try:
    import resource
except ImportError:  # Windows has no rusage; child CPU stays unknown there.
    resource = None  # type: ignore[assignment]

SCHEMA = "molt.pytest-profile.v1"
OPTION = "--molt-profile"
PLUGIN_NAME = "molt-pytest-profile"
_CHECKPOINT_INTERVAL_SECONDS = 10.0
_PHASES = ("setup", "call", "teardown")


def _now() -> float:
    return time.time()


def _child_cpu_seconds() -> float | None:
    if resource is None:
        return None
    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    return usage.ru_utime + usage.ru_stime


class SpawnCounter:
    """Count ``subprocess.Popen`` constructions in this process."""

    def __init__(self) -> None:
        self.count = 0

    def install(self) -> None:
        original = subprocess.Popen.__init__
        counter = self

        def counting(popen: subprocess.Popen[Any], *args: Any, **kwargs: Any) -> None:
            counter.count += 1
            original(popen, *args, **kwargs)

        subprocess.Popen.__init__ = counting  # type: ignore[method-assign]


class Profiler:
    """Measure phases where they run; collect reports where they arrive.

    Under xdist each worker holds one instance that measures and attaches its
    measurements to the reports it sends; the controller holds one that
    records every report. A serial run's one instance does both.
    """

    def __init__(self, path: Path, *, worker: str | None) -> None:
        self.path = path
        self.worker = worker
        self.spawns = SpawnCounter()
        self.spawns.install()
        self._started: dict[tuple[str, str], tuple[float, float | None, int]] = {}
        self._measured: dict[tuple[str, str], dict[str, Any]] = {}
        self.records: list[dict[str, Any]] = []
        self.session_start: float | None = None
        self.collection_finish: float | None = None
        self.session_finish: float | None = None
        self.numprocesses: int | None = None
        self.argv = list(sys.argv)
        self._last_checkpoint: float | None = None
        self._first_phase_checkpointed = False

    # -- measuring, where the test runs --

    def _phase(self, item: Any, when: str):  # type: ignore[no-untyped-def]
        key = (item.nodeid, when)
        self._started[key] = (
            time.process_time(),
            _child_cpu_seconds(),
            self.spawns.count,
        )
        try:
            return (yield)
        finally:
            cpu, child_cpu, spawns = self._started.pop(key)
            child_now = _child_cpu_seconds()
            self._measured[key] = {
                "cpu": time.process_time() - cpu,
                "child_cpu": (
                    None
                    if child_cpu is None or child_now is None
                    else child_now - child_cpu
                ),
                "spawns": self.spawns.count - spawns,
            }

    @pytest.hookimpl(wrapper=True)
    def pytest_runtest_setup(self, item: Any):  # type: ignore[no-untyped-def]
        return (yield from self._phase(item, "setup"))

    @pytest.hookimpl(wrapper=True)
    def pytest_runtest_call(self, item: Any):  # type: ignore[no-untyped-def]
        return (yield from self._phase(item, "call"))

    @pytest.hookimpl(wrapper=True)
    def pytest_runtest_teardown(self, item: Any):  # type: ignore[no-untyped-def]
        return (yield from self._phase(item, "teardown"))

    @pytest.hookimpl(wrapper=True)
    def pytest_runtest_makereport(self, item: Any, call: Any):  # type: ignore[no-untyped-def]
        report = yield
        measured = self._measured.pop((item.nodeid, call.when), None)
        if measured is not None:
            # A plain attribute survives xdist's report serialization.
            report.molt_profile = measured
        return report

    # -- recording, where the reports arrive --

    def pytest_runtest_logreport(self, report: Any) -> None:
        if self.worker is not None:
            return
        gateway = getattr(getattr(report, "node", None), "gateway", None)
        measured = getattr(report, "molt_profile", None) or {}
        self.records.append(
            {
                "nodeid": report.nodeid,
                "when": report.when,
                "outcome": report.outcome,
                "duration": report.duration,
                "start": report.start,
                "stop": report.stop,
                "worker": gateway.id if gateway is not None else "main",
                "cpu": measured.get("cpu"),
                "child_cpu": measured.get("child_cpu"),
                "spawns": measured.get("spawns"),
            }
        )
        now = time.monotonic()
        if (
            not self._first_phase_checkpointed
            or self._last_checkpoint is None
            or now - self._last_checkpoint >= _CHECKPOINT_INTERVAL_SECONDS
        ):
            # Callback-driven: a long phase can leave this snapshot older than
            # ten seconds. Only completed reports are measured, never guessed.
            self.write(exit_at=None)
            self._last_checkpoint = time.monotonic()
            self._first_phase_checkpointed = True

    @pytest.hookimpl(tryfirst=True)
    def pytest_sessionstart(self, session: Any) -> None:
        self.session_start = _now()
        self.numprocesses = getattr(session.config.option, "numprocesses", None)
        if self.worker is None:
            self.write(exit_at=None)
            self._last_checkpoint = time.monotonic()

    def pytest_runtest_logstart(self, nodeid: str, location: Any) -> None:
        # The first test starts once every worker has collected: xdist's
        # controller never collects itself, so this is the one collection end
        # both a serial run and a distributed run report.
        if self.collection_finish is None:
            self.collection_finish = _now()

    @pytest.hookimpl(trylast=True)
    def pytest_sessionfinish(self, session: Any, exitstatus: Any) -> None:
        self.session_finish = _now()
        self.write(exit_at=None)
        atexit.register(lambda: self.write(exit_at=_now()))

    def file(self) -> Path:
        if self.worker is None:
            return self.path
        return self.path.with_name(f"{self.path.name}.worker-{self.worker}.json")

    def write(self, *, exit_at: float | None) -> None:
        from molt.artifact_publication import atomic_write_json

        payload: dict[str, Any] = {
            "schema": SCHEMA,
            "worker": self.worker,
            "session_start": self.session_start,
            "collection_finish": self.collection_finish,
            "session_finish": self.session_finish,
            "exit_at": exit_at,
        }
        if self.worker is None:
            payload["argv"] = self.argv
            payload["numprocesses"] = self.numprocesses
            payload["phases"] = self.records
        atomic_write_json(self.file(), payload)


def pytest_addoption(parser: Any) -> None:
    parser.getgroup("molt-profile").addoption(
        OPTION,
        dest="molt_profile",
        default=None,
        help="write a JSON record of every test phase's wall, CPU, child CPU and spawns",
    )


def pytest_configure(config: Any) -> None:
    raw = config.getoption("molt_profile")
    if not raw:
        return
    path = Path(raw)
    if not path.is_absolute():
        path = Path.cwd() / path
    workerinput = getattr(config, "workerinput", None)
    worker = workerinput.get("workerid") if workerinput is not None else None
    config.pluginmanager.register(Profiler(path, worker=worker), PLUGIN_NAME)


# --------------------------------------------------------------------------
# Reading a record
# --------------------------------------------------------------------------


def load(path: Path) -> dict[str, Any]:
    """The controller's record with its workers' exit records joined."""
    profile = json.loads(path.read_text(encoding="utf-8"))
    if profile.get("schema") != SCHEMA:
        raise ValueError(f"{path} is not a {SCHEMA} record")
    workers = {}
    # An interrupted snapshot has no exit-tail evidence; old siblings from a
    # prior use of the output path must not supply it.
    siblings = (
        sorted(path.parent.glob(f"{path.name}.worker-*.json"))
        if profile.get("session_finish") is not None
        else ()
    )
    for sibling in siblings:
        record = json.loads(sibling.read_text(encoding="utf-8"))
        workers[record["worker"]] = record
    profile["worker_records"] = workers
    return profile


def tests_of(profile: dict[str, Any]) -> list[dict[str, Any]]:
    """One row per test with its phases joined, in the order the tests ended."""
    rows: dict[str, dict[str, Any]] = {}
    for phase in profile["phases"]:
        row = rows.setdefault(
            phase["nodeid"],
            {
                "nodeid": phase["nodeid"],
                "file": phase["nodeid"].split("::", 1)[0],
                "worker": phase["worker"],
                "outcome": "passed",
                "complete": False,
                "setup": 0.0,
                "call": 0.0,
                "teardown": 0.0,
                "start": phase["start"],
                "stop": phase["stop"],
                "cpu": 0.0,
                "child_cpu": 0.0,
                "spawns": 0,
            },
        )
        row[phase["when"]] += phase["duration"]
        if phase["when"] == "teardown":
            row["complete"] = True
        row["start"] = min(row["start"], phase["start"])
        row["stop"] = max(row["stop"], phase["stop"])
        if phase["outcome"] != "passed":
            row["outcome"] = phase["outcome"]
        row["cpu"] += phase.get("cpu") or 0.0
        row["child_cpu"] += phase.get("child_cpu") or 0.0
        row["spawns"] += phase.get("spawns") or 0
    for row in rows.values():
        if not row["complete"] and row["outcome"] == "passed":
            row["outcome"] = "incomplete"
        row["total"] = row["setup"] + row["call"] + row["teardown"]
        row["waiting"] = max(0.0, row["total"] - row["cpu"] - row["child_cpu"])
    return sorted(rows.values(), key=lambda row: row["stop"])


def summarize(profile: dict[str, Any]) -> dict[str, Any]:
    tests = tests_of(profile)
    start = profile.get("session_start")
    finish = profile.get("session_finish")
    workers: dict[str, dict[str, Any]] = {}
    for row in tests:
        worker = workers.setdefault(
            row["worker"],
            {"busy": 0.0, "first": row["start"], "last": row["stop"], "tests": 0},
        )
        worker["busy"] += row["total"]
        worker["first"] = min(worker["first"], row["start"])
        worker["last"] = max(worker["last"], row["stop"])
        worker["tests"] += 1
    for worker in workers.values():
        worker["span"] = worker["last"] - worker["first"]
    total = sum(row["total"] for row in tests)
    busiest = max((worker["busy"] for worker in workers.values()), default=0.0)
    mean = total / len(workers) if workers else 0.0
    exits = [
        record["exit_at"] - record["session_finish"]
        for record in profile.get("worker_records", {}).values()
        if finish is not None and record.get("exit_at") and record.get("session_finish")
    ]
    files: dict[str, dict[str, Any]] = {}
    for row in tests:
        entry = files.setdefault(
            row["file"], {"total": 0.0, "setup": 0.0, "tests": 0, "spawns": 0}
        )
        entry["total"] += row["total"]
        entry["setup"] += row["setup"]
        entry["tests"] += 1
        entry["spawns"] += row["spawns"]
    collection = profile.get("collection_finish")
    exit_at = profile.get("exit_at")
    return {
        "complete": finish is not None,
        "incomplete_tests": sum(not row["complete"] for row in tests),
        "wall": finish - start if start and finish else None,
        "collection": collection - start if start and collection else None,
        "exit_tail": exit_at - finish if finish and exit_at else None,
        "worker_exit_tail_max": max(exits) if exits else None,
        "tests": len(tests),
        "failed": sum(row["outcome"] == "failed" for row in tests),
        "workers": len(workers),
        "numprocesses": profile.get("numprocesses"),
        "sum_total": total,
        "sum_setup": sum(row["setup"] for row in tests),
        "sum_call": sum(row["call"] for row in tests),
        "sum_teardown": sum(row["teardown"] for row in tests),
        "sum_cpu": sum(row["cpu"] for row in tests),
        "sum_child_cpu": sum(row["child_cpu"] for row in tests),
        "sum_waiting": sum(row["waiting"] for row in tests),
        "spawns": sum(row["spawns"] for row in tests),
        "busiest_worker": busiest,
        "mean_worker": mean,
        "imbalance": (busiest / mean - 1.0) if mean else 0.0,
        "parallel_floor": total / len(workers) if workers else 0.0,
        "per_worker": workers,
        "per_file": files,
        "test_rows": tests,
    }


def _seconds(value: float | None) -> str:
    return "n/a" if value is None else f"{value:.1f}s"


def _test_line(row: dict[str, Any]) -> str:
    return (
        f"{row['total']:7.1f}s  setup {row['setup']:6.1f}  call {row['call']:6.1f}"
        f"  teardown {row['teardown']:5.1f}  cpu {row['cpu']:5.1f}"
        f"  child {row['child_cpu']:6.1f}  wait {row['waiting']:6.1f}"
        f"  spawns {row['spawns']:3d}  [{row['worker']}] {row['nodeid']}"
    )


def report_lines(profile: dict[str, Any], *, top: int = 20) -> list[str]:
    summary = summarize(profile)
    rows = summary["test_rows"]
    wall, floor = summary["wall"], summary["parallel_floor"]
    lines = [
        f"pytest profile: {summary['tests']} tests ({summary['failed']} failed) on "
        f"{summary['workers']} worker(s) (-n {summary['numprocesses']})",
        f"  wall {_seconds(wall)}: collection {_seconds(summary['collection'])},"
        f" exit tail after the session {_seconds(summary['exit_tail'])}"
        f" (slowest worker exit {_seconds(summary['worker_exit_tail_max'])})",
        f"  test time {summary['sum_total']:.1f}s = setup {summary['sum_setup']:.1f}"
        f" + call {summary['sum_call']:.1f} + teardown {summary['sum_teardown']:.1f};"
        f" cpu {summary['sum_cpu']:.1f}, child cpu {summary['sum_child_cpu']:.1f},"
        f" waiting {summary['sum_waiting']:.1f}; {summary['spawns']} spawns",
        f"  workers: busiest {summary['busiest_worker']:.1f}s, mean"
        f" {summary['mean_worker']:.1f}s (+{summary['imbalance'] * 100:.1f}%);"
        f" wall / (test time / workers) = {(wall / floor) if wall and floor else 0:.2f}",
    ]
    if not summary["complete"]:
        lines.insert(
            0,
            "INCOMPLETE pytest profile: last published completed phases only; "
            "observed phase-duration totals are lower bounds; "
            "derived waiting and imbalance are provisional; "
            "not a completed run or proof result "
            f"({summary['incomplete_tests']} tests have unfinished phase coverage).",
        )
    for worker, entry in sorted(summary["per_worker"].items()):
        lines.append(
            f"    {worker}: busy {entry['busy']:.1f}s over a span of"
            f" {entry['span']:.1f}s, {entry['tests']} tests"
        )
    lines.append(f"  slowest {top} tests:")
    for row in sorted(rows, key=lambda row: row["total"], reverse=True)[:top]:
        lines.append("    " + _test_line(row))
    lines.append(f"  most waiting (wall beyond own and child CPU), top {top}:")
    for row in sorted(rows, key=lambda row: row["waiting"], reverse=True)[:top]:
        if row["waiting"] <= 0:
            break
        lines.append("    " + _test_line(row))
    lines.append(f"  slowest setups, top {top}:")
    for row in sorted(rows, key=lambda row: row["setup"], reverse=True)[:top]:
        if row["setup"] <= 0:
            break
        lines.append("    " + _test_line(row))
    lines.append(f"  files by test time, top {top}:")
    for name, entry in sorted(
        summary["per_file"].items(), key=lambda item: item[1]["total"], reverse=True
    )[:top]:
        lines.append(
            f"    {entry['total']:7.1f}s  setup {entry['setup']:6.1f}  tests"
            f" {entry['tests']:4d}  spawns {entry['spawns']:4d}  {name}"
        )
    if wall:
        # The run's critical tail: 5% of the wall, between 10 s and 60 s.
        tail_from = profile["session_finish"] - min(60.0, max(10.0, wall * 0.05))
        lines.append("  tests still running in the session's last stretch:")
        for row in rows:
            if row["stop"] >= tail_from:
                lines.append("    " + _test_line(row))
    return lines


def compare_lines(
    before: dict[str, Any], after: dict[str, Any], *, top: int = 20
) -> list[str]:
    first, second = summarize(before), summarize(after)
    if not first["complete"] or not second["complete"]:
        raise ValueError("cannot compare incomplete pytest profiles as completed runs")

    def delta(key: str) -> str:
        a, b = first.get(key), second.get(key)
        if a is None or b is None:
            return f"{key} {_seconds(a)} -> {_seconds(b)}"
        return f"{key} {a:.1f} -> {b:.1f} ({b - a:+.1f})"

    keys = (
        "wall",
        "exit_tail",
        "sum_total",
        "sum_setup",
        "sum_waiting",
        "sum_child_cpu",
        "busiest_worker",
    )
    lines = [
        "pytest profile comparison (before -> after)",
        "  " + "; ".join(delta(key) for key in keys),
        f"  tests {first['tests']} -> {second['tests']}; workers"
        f" {first['workers']} -> {second['workers']}; spawns"
        f" {first['spawns']} -> {second['spawns']}",
        "  files:",
    ]
    changes = []
    for name in set(first["per_file"]) | set(second["per_file"]):
        a = first["per_file"].get(name, {}).get("total", 0.0)
        b = second["per_file"].get(name, {}).get("total", 0.0)
        changes.append((b - a, a, b, name))
    for change, a, b, name in sorted(changes)[:top]:
        lines.append(f"    {a:7.1f}s -> {b:7.1f}s ({change:+7.1f})  {name}")
    before_tests = {row["nodeid"]: row["total"] for row in first["test_rows"]}
    after_tests = {row["nodeid"]: row["total"] for row in second["test_rows"]}
    moved = [
        (after_tests.get(nodeid, 0.0) - before_tests.get(nodeid, 0.0), nodeid)
        for nodeid in set(before_tests) | set(after_tests)
    ]
    lines.append("  tests that changed most:")
    for change, nodeid in sorted(moved, key=lambda item: abs(item[0]), reverse=True)[
        :top
    ]:
        a, b = before_tests.get(nodeid, 0.0), after_tests.get(nodeid, 0.0)
        lines.append(f"    {a:7.1f}s -> {b:7.1f}s ({change:+7.1f})  {nodeid}")
    return lines


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    commands = parser.add_subparsers(dest="command", required=True)
    report = commands.add_parser("report", help="summarize one record")
    report.add_argument("profile", type=Path)
    report.add_argument("--top", type=int, default=20)
    compare = commands.add_parser("compare", help="compare two records")
    compare.add_argument("before", type=Path)
    compare.add_argument("after", type=Path)
    compare.add_argument("--top", type=int, default=20)
    args = parser.parse_args(argv)
    if args.command == "report":
        lines = report_lines(load(args.profile), top=args.top)
    else:
        try:
            lines = compare_lines(load(args.before), load(args.after), top=args.top)
        except ValueError as exc:
            parser.error(str(exc))
    print("\n".join(lines))
    return 0


if __name__ == "__main__":
    sys.exit(main())
