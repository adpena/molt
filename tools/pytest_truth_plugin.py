"""Count pytest execution and fail closed when a selected partition does no work."""

from __future__ import annotations

import datetime as dt
import os
from pathlib import Path
from typing import Any

import pytest

from tools.artifact_publish import atomic_write_json

RECEIPT_ENV = "MOLT_PYTEST_TRUTH_RECEIPT"
MINIMUM_ENV = "MOLT_PYTEST_TRUTH_MINIMUM_EXECUTED"
SCHEMA = "molt.pytest-truth.v1"


class ExecutionCounter:
    def __init__(self, *, receipt: Path, minimum_executed: int) -> None:
        self.receipt = receipt
        self.minimum_executed = minimum_executed
        self.started = dt.datetime.now(dt.UTC)
        self.collected = 0
        self.executed = 0
        self.passed = 0
        self.failed = 0
        self.skipped = 0
        self.xfailed = 0
        self.xpassed = 0

    def pytest_collection_finish(self, session: pytest.Session) -> None:
        self.collected = len(session.items)

    def pytest_runtest_logreport(self, report: pytest.TestReport) -> None:
        if report.when == "setup" and report.skipped:
            self.skipped += 1
            return
        if report.when != "call":
            return
        self.executed += 1
        was_xfail = getattr(report, "wasxfail", None)
        if report.skipped and was_xfail:
            self.xfailed += 1
        elif report.passed and was_xfail:
            self.xpassed += 1
        elif report.passed:
            self.passed += 1
        elif report.failed:
            self.failed += 1
        elif report.skipped:
            self.skipped += 1

    def pytest_sessionfinish(
        self, session: pytest.Session, exitstatus: int | pytest.ExitCode
    ) -> None:
        finished = dt.datetime.now(dt.UTC)
        original_exitstatus = int(exitstatus)
        problems: list[str] = []
        if original_exitstatus != 0:
            problems.append(f"pytest exited with status {original_exitstatus}")
        if self.executed < self.minimum_executed:
            problems.append(
                "pytest partition executed "
                f"{self.executed} tests; required at least {self.minimum_executed}"
            )
        final_exitstatus = original_exitstatus or (3 if problems else 0)
        if final_exitstatus != original_exitstatus:
            session.exitstatus = final_exitstatus
        payload: dict[str, Any] = {
            "schema": SCHEMA,
            "status": "success" if not problems else "failure",
            "started_at": self.started.isoformat(),
            "finished_at": finished.isoformat(),
            "duration_seconds": round((finished - self.started).total_seconds(), 6),
            "minimum_executed_test_count": self.minimum_executed,
            "pytest_returncode": original_exitstatus,
            "final_returncode": final_exitstatus,
            "collected_test_count": self.collected,
            "executed_test_count": self.executed,
            "passed_test_count": self.passed,
            "failed_test_count": self.failed,
            "skipped_test_count": self.skipped,
            "xfailed_test_count": self.xfailed,
            "xpassed_test_count": self.xpassed,
            "problems": problems,
        }
        atomic_write_json(self.receipt, payload, sort_keys=True)
        if problems:
            for problem in problems:
                print(f"pytest-truth: ERROR: {problem}")
        else:
            print(
                "pytest-truth: OK "
                f"(executed={self.executed}, passed={self.passed}, "
                f"skipped={self.skipped})"
            )
        print(f"pytest-truth: receipt={self.receipt}")


def pytest_configure(config: pytest.Config) -> None:
    receipt_text = os.environ.get(RECEIPT_ENV)
    if not receipt_text:
        raise pytest.UsageError(f"{RECEIPT_ENV} is required")
    minimum_text = os.environ.get(MINIMUM_ENV, "1")
    try:
        minimum = int(minimum_text)
    except ValueError as exc:
        raise pytest.UsageError(f"{MINIMUM_ENV} must be an integer") from exc
    if minimum < 1:
        raise pytest.UsageError(f"{MINIMUM_ENV} must be at least 1")
    receipt = Path(receipt_text)
    if not receipt.is_absolute():
        receipt = Path.cwd() / receipt
    config.pluginmanager.register(
        ExecutionCounter(receipt=receipt, minimum_executed=minimum),
        "molt-pytest-truth-counter",
    )
