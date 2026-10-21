"""Peak memory of each pytest worker's process subtree, and of each test.

A pytest-xdist worker publishes its current test in
``<aggregate>.d/<worker>-<pid>_current-test.json``. Each guard sample sums the
RSS of every watched process whose parent chain reaches a live worker; the
largest sum over the run is the memory one worker needs, which is the unit a
worker count is sized from. The record names the test the worker runs, so the
same sum is that test's peak. A run without xdist has one unit: the process
that publishes the aggregate record. An xdist controller marks its aggregate
record, which never counts as a unit.

Records are read only when their modification time changes, with the
nonblocking reader the repro context uses, so a sample costs one directory
scan and one stat per worker.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
import heapq
import os
from pathlib import Path
import stat

from molt.memory_guard_paths import pytest_custody_path_is_canonical
from molt.process_model import ProcessSample
from tools.memory_guard_core.repro_context import _read_pytest_current_test_json

# The phases in which a worker's record names the test it is running.
_TEST_PHASES = frozenset({"start", "setup", "call", "teardown"})
TOP_TESTS = 10


@dataclass(frozen=True, slots=True)
class WorkerPeakSummary:
    """The largest worker subtree and the tests with the largest peaks."""

    worker_peak_kb: int
    workers_observed: int
    top_tests: tuple[tuple[str, int], ...]

    def json_payload(self) -> dict[str, object]:
        return {
            "worker_peak_rss_bytes": self.worker_peak_kb * 1024,
            "workers_observed": self.workers_observed,
            "top_tests": [
                {"nodeid": nodeid, "peak_rss_bytes": kb * 1024}
                for nodeid, kb in self.top_tests
            ],
        }


@dataclass(slots=True)
class _Record:
    mtime_ns: int
    pid: int | None
    nodeid: str | None
    controller: bool


@dataclass(slots=True)
class WorkerPeakTracker:
    aggregate_path: Path
    root: Path
    environ: Mapping[str, str]
    _records: dict[str, _Record] = field(default_factory=dict)
    _worker_peak_kb: int = 0
    _workers: set[int] = field(default_factory=set)
    _test_peaks: dict[str, int] = field(default_factory=dict)

    @classmethod
    def for_child(
        cls,
        child_env: Mapping[str, str] | None,
        *,
        current_test_file_env: str,
        root: Path,
    ) -> "WorkerPeakTracker | None":
        """A tracker for a child that publishes current-test records, else None."""
        environ = os.environ if child_env is None else child_env
        raw = environ.get(current_test_file_env, "").strip()
        if not raw:
            return None
        path = Path(raw)
        if not path.is_absolute() or not pytest_custody_path_is_canonical(
            root, path, environ=environ
        ):
            return None
        return cls(aggregate_path=path, root=root, environ=environ)

    def _read(self, path: Path) -> _Record | None:
        try:
            info = path.lstat()
        except OSError:
            return None
        if not stat.S_ISREG(info.st_mode):
            return None
        key = str(path)
        cached = self._records.get(key)
        if cached is not None and cached.mtime_ns == info.st_mtime_ns:
            return cached
        payload = _read_pytest_current_test_json(path).get("payload")
        if not isinstance(payload, dict):
            # A record mid-publication keeps its previous reading.
            return cached
        pid = payload.get("pid")
        nodeid = payload.get("nodeid")
        record = _Record(
            mtime_ns=info.st_mtime_ns,
            pid=pid if type(pid) is int else None,
            nodeid=nodeid if payload.get("phase") in _TEST_PHASES else None,
            controller=payload.get("xdist_controller") is True,
        )
        self._records[key] = record
        return record

    def _units(self) -> list[_Record]:
        worker_dir = self.aggregate_path.with_name(f"{self.aggregate_path.name}.d")
        units: list[_Record] = []
        if pytest_custody_path_is_canonical(
            self.root, worker_dir, environ=self.environ
        ):
            try:
                with os.scandir(worker_dir) as entries:
                    paths = [
                        Path(entry.path)
                        for entry in entries
                        if entry.name.endswith("_current-test.json")
                    ]
            except OSError:
                paths = []
            units = [
                record for path in paths if (record := self._read(path)) is not None
            ]
        # Without xdist the aggregate's publisher runs the tests itself. An xdist
        # controller also writes it and runs no test, so it is never a unit.
        record = self._read(self.aggregate_path)
        if record is not None and not record.controller:
            units.append(record)
        return units

    def observe(self, samples: Mapping[int, ProcessSample], watched: set[int]) -> None:
        units = [
            (record.pid, record.nodeid)
            for record in self._units()
            if record.pid is not None and record.pid in watched
        ]
        if not units:
            return
        children: dict[int, list[int]] = {}
        for pid in watched:
            sample = samples.get(pid)
            if sample is not None and sample.ppid != pid:
                children.setdefault(sample.ppid, []).append(pid)
        for unit_pid, nodeid in units:
            total = 0
            seen: set[int] = set()
            pending = [unit_pid]
            while pending:
                pid = pending.pop()
                if pid in seen:
                    continue
                seen.add(pid)
                sample = samples.get(pid)
                if sample is not None:
                    total += sample.rss_kb
                pending.extend(children.get(pid, ()))
            self._workers.add(unit_pid)
            self._worker_peak_kb = max(self._worker_peak_kb, total)
            if nodeid:
                self._test_peaks[nodeid] = max(self._test_peaks.get(nodeid, 0), total)

    def summary(self) -> WorkerPeakSummary | None:
        if not self._workers:
            return None
        top = heapq.nlargest(
            TOP_TESTS, self._test_peaks.items(), key=lambda item: (item[1], item[0])
        )
        return WorkerPeakSummary(
            worker_peak_kb=self._worker_peak_kb,
            workers_observed=len(self._workers),
            top_tests=tuple(top),
        )
