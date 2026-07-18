from __future__ import annotations

import pytest

from tools.fuzz_compiler_cli import (
    classified_fuzz_receipt,
    safe_fuzz_exit_code,
    safe_fuzz_receipt,
)
from tools.fuzz_compiler_types import FuzzSummary


def test_safe_fuzz_all_timeouts_fail_as_infrastructure_error() -> None:
    summary = FuzzSummary(total=4, timeouts=4)

    assert safe_fuzz_exit_code(summary) == 2
    assert safe_fuzz_receipt(summary) == {
        "schema": "molt.fuzz-proof.v2",
        "mode": "safe",
        "status": "failure",
        "selected": 4,
        "executed": 0,
        "passed": 0,
        "failed": 0,
        "errors": 4,
        "error_detail": {
            "build": 0,
            "cpython": 0,
            "molt_runtime": 0,
            "timeout": 4,
        },
        "exit_code": 2,
    }


def test_safe_fuzz_all_build_errors_fail_as_infrastructure_error() -> None:
    summary = FuzzSummary(total=3, build_errors=3)

    assert safe_fuzz_exit_code(summary) == 2
    assert safe_fuzz_receipt(summary)["executed"] == 0


def test_safe_fuzz_mixed_pass_and_error_cannot_green() -> None:
    assert safe_fuzz_exit_code(FuzzSummary(total=2, passed=1, cpython_errors=1)) == 2
    assert safe_fuzz_exit_code(FuzzSummary(total=2, passed=1, mismatches=1)) == 1
    assert safe_fuzz_exit_code(FuzzSummary(total=2, passed=2)) == 0


def test_safe_fuzz_rejects_unclassified_results() -> None:
    with pytest.raises(ValueError, match="accounting mismatch"):
        safe_fuzz_exit_code(FuzzSummary(total=1))


def test_reject_and_compile_only_receipts_count_timeouts_and_generation() -> None:
    reject = classified_fuzz_receipt(
        FuzzSummary(total=3, reject_pass=1, reject_fail=1, timeouts=1), "reject"
    )
    compile_only = classified_fuzz_receipt(
        FuzzSummary(
            total=4,
            compile_only_ok=1,
            compile_only_crash=1,
            timeouts=1,
            generation_errors=1,
        ),
        "compile-only",
    )

    assert reject["executed"] == 2
    assert reject["failed"] == 1
    assert reject["errors"] == 1
    assert reject["status"] == "failure"
    assert compile_only["executed"] == 2
    assert compile_only["errors"] == 2
    assert compile_only["generation_errors"] == 1
    assert compile_only["status"] == "failure"


def test_every_fuzz_mode_rejects_zero_work() -> None:
    for mode in ("safe", "reject", "compile-only"):
        receipt = classified_fuzz_receipt(FuzzSummary(), mode)
        assert receipt["status"] == "failure"
        assert receipt["executed"] == 0
        assert receipt["exit_code"] == 2
