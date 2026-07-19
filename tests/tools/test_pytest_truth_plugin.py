from __future__ import annotations

import json
from pathlib import Path
from types import SimpleNamespace

from tools.pytest_truth_plugin import ExecutionCounter


def _report(*, when: str, passed: bool = False, skipped: bool = False):
    return SimpleNamespace(
        when=when,
        passed=passed,
        failed=not passed and not skipped,
        skipped=skipped,
    )


def test_pytest_truth_records_nonzero_execution(tmp_path: Path) -> None:
    receipt = tmp_path / "receipt.json"
    counter = ExecutionCounter(receipt=receipt, minimum_executed=1)
    counter.pytest_collection_finish(SimpleNamespace(items=[object()]))
    counter.pytest_runtest_logreport(_report(when="call", passed=True))
    session = SimpleNamespace(exitstatus=0)

    counter.pytest_sessionfinish(session, 0)

    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert session.exitstatus == 0
    assert payload["status"] == "success"
    assert payload["collected_test_count"] == 1
    assert payload["executed_test_count"] == 1
    assert payload["passed_test_count"] == 1


def test_pytest_truth_rejects_all_skipped_partition(tmp_path: Path) -> None:
    receipt = tmp_path / "receipt.json"
    counter = ExecutionCounter(receipt=receipt, minimum_executed=1)
    counter.pytest_collection_finish(SimpleNamespace(items=[object()]))
    counter.pytest_runtest_logreport(_report(when="setup", skipped=True))
    session = SimpleNamespace(exitstatus=0)

    counter.pytest_sessionfinish(session, 0)

    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert session.exitstatus == 3
    assert payload["status"] == "failure"
    assert payload["collected_test_count"] == 1
    assert payload["executed_test_count"] == 0
    assert payload["skipped_test_count"] == 1
    assert payload["final_returncode"] == 3
    assert payload["problems"] == [
        "pytest partition executed 0 tests; required at least 1"
    ]
