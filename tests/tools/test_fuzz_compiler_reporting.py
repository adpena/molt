from __future__ import annotations

import pytest

from tools import fuzz_compiler_driver
from tools.fuzz_compiler_reporting import _print_diff_snippet
from tools.fuzz_compiler_types import FuzzResult


def _result(status: str, **fields: str) -> FuzzResult:
    return FuzzResult(
        program_id=0, seed=7, source="print(1)\n", status=status, **fields
    )


def test_quiet_campaign_logs_each_failure_when_it_happens(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    outcomes = iter(
        [
            _result(
                "molt_run_error",
                cpython_stdout="a\nb\nc\n",
                molt_stdout="a\n",
                molt_stderr="warming up\nTypeError: unsupported operand\n",
                error_detail="Molt binary exited with rc=1",
            ),
            _result(
                "build_error",
                error_detail="backend failed:\n  lowering   error",
            ),
        ]
    )
    monkeypatch.setattr(
        fuzz_compiler_driver, "fuzz_one_safe", lambda **_kwargs: next(outcomes)
    )

    summary = fuzz_compiler_driver.run_safe_fuzzer(
        count=2,
        seed=100,
        output_dir=None,
        profile="dev",
        timeout=1.0,
        verbose=False,
    )

    log = capsys.readouterr().err
    assert (summary.molt_run_errors, summary.build_errors) == (1, 1)
    assert "MOLT_RUN_ERROR (seed=100)" in log
    assert "Molt binary exited with rc=1" in log
    assert "molt stderr: TypeError: unsupported operand" in log
    assert "CPython: 'b'" in log
    assert "Molt:    '<missing>'" in log
    assert "BUILD_ERROR (seed=101)" in log
    assert "backend failed: lowering error" in log


def test_diff_snippet_reports_differences_past_the_first_lines(
    capsys: pytest.CaptureFixture[str],
) -> None:
    expected = [f"line-{n}" for n in range(40)]
    actual = list(expected)
    for n in (30, 31, 32, 33, 34, 35, 36):
        actual[n] = f"wrong-{n}"
    result = _result(
        "mismatch",
        cpython_stdout="\n".join(expected),
        molt_stdout="\n".join(actual),
    )

    _print_diff_snippet(result)

    log = capsys.readouterr().err
    assert "line 31:" in log
    assert "Molt:    'wrong-30'" in log
    assert "line 35:" in log
    assert "line 36:" not in log
    assert "... and 2 more differing lines" in log
