"""Verdicts of tools/translation_validate.py: exact observables, fail-closed exits.

A translation-validation run may only succeed when at least one file executed
and every executed file produced exactly CPython's observable. Trailing spaces
and the final newline are program output. A run that produced no observable (a
timeout, a missing binary, a launcher failure) is inconclusive: it makes the
file an error, never a semantic mismatch, and it fails the batch even when
other files passed.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

from tools import translation_validate as tv


def _run(
    mode: str,
    *,
    stdout: str = "",
    stderr: str = "",
    returncode: int = 0,
    harness_error: str | None = None,
) -> tv.RunResult:
    return tv.RunResult(
        stdout=stdout,
        stderr=stderr,
        returncode=returncode,
        elapsed_ms=0.0,
        mode=mode,
        harness_error=harness_error,
    )


def _validate(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    cpython: tv.RunResult,
    molt: tv.RunResult,
    *,
    name: str = "case.py",
) -> tv.ValidationResult:
    source = tmp_path / name
    source.write_text("print('ok')\n", encoding="utf-8")
    monkeypatch.setattr(tv, "_run_cpython", lambda *_args, **_kwargs: cpython)
    monkeypatch.setattr(tv, "_run_molt", lambda *_args, **_kwargs: molt)
    return tv.validate_file(str(source))


def _exit(*results: tv.ValidationResult) -> int:
    return tv.summary_exit_code(tv.ValidationSummary(results=list(results)))


def test_identical_observables_pass(monkeypatch, tmp_path) -> None:
    result = _validate(
        monkeypatch,
        tmp_path,
        _run("cpython", stdout="ok\n"),
        _run("molt", stdout="ok\n"),
    )
    assert result.match_molt_vs_cpython is True
    assert _exit(result) == 0


@pytest.mark.parametrize(
    ("cpython_stdout", "molt_stdout"),
    [
        ("ok \n", "ok\n"),  # a trailing space
        ("ok\n", "ok"),  # a missing final newline
        ("ok\n", "ok\n\n"),  # an extra blank line
    ],
)
def test_trailing_whitespace_and_the_final_newline_are_observable(
    monkeypatch, tmp_path, cpython_stdout: str, molt_stdout: str
) -> None:
    result = _validate(
        monkeypatch,
        tmp_path,
        _run("cpython", stdout=cpython_stdout),
        _run("molt", stdout=molt_stdout),
    )
    assert result.match_molt_vs_cpython is False
    assert _exit(result) == 1


def test_trailing_stderr_differences_are_observable(monkeypatch, tmp_path) -> None:
    result = _validate(
        monkeypatch,
        tmp_path,
        _run("cpython", stderr="warning\n"),
        _run("molt", stderr="warning"),
    )
    assert result.match_molt_vs_cpython is False


def test_a_timeout_is_an_error_not_a_mismatch(monkeypatch, tmp_path) -> None:
    result = _validate(
        monkeypatch,
        tmp_path,
        _run("cpython", stdout="ok\n"),
        _run("molt", returncode=-1, harness_error="timed out after 5s: case_molt"),
    )
    assert result.error is not None and "timed out after 5s" in result.error
    summary = tv.ValidationSummary(results=[result])
    assert (summary.errors, summary.mismatches) == (1, 0)
    assert _exit(result) == 2


def test_a_missing_binary_is_an_error_not_a_mismatch(monkeypatch, tmp_path) -> None:
    result = _validate(
        monkeypatch,
        tmp_path,
        _run("cpython", stdout="ok\n"),
        _run("molt", returncode=-1, harness_error="binary not found at out/case"),
    )
    assert result.error is not None and "binary not found" in result.error
    assert _exit(result) == 2


def test_a_mixed_batch_of_passes_and_errors_fails_closed(monkeypatch, tmp_path) -> None:
    passed = _validate(
        monkeypatch,
        tmp_path,
        _run("cpython", stdout="ok\n"),
        _run("molt", stdout="ok\n"),
        name="passes.py",
    )
    errored = _validate(
        monkeypatch,
        tmp_path,
        _run("cpython", stdout="ok\n"),
        _run("molt", returncode=-1, harness_error="timed out after 5s: errs_molt"),
        name="errs.py",
    )
    assert _exit(passed, errored) == 2
    assert _exit(passed, passed) == 0


def test_a_mismatch_outranks_nothing_but_still_fails(monkeypatch, tmp_path) -> None:
    passed = _validate(
        monkeypatch,
        tmp_path,
        _run("cpython", stdout="ok\n"),
        _run("molt", stdout="ok\n"),
        name="passes.py",
    )
    mismatched = _validate(
        monkeypatch,
        tmp_path,
        _run("cpython", stdout="1\n"),
        _run("molt", stdout="2\n"),
        name="differs.py",
    )
    assert _exit(passed, mismatched) == 1


def test_an_empty_selection_fails_closed(tmp_path) -> None:
    summary = tv.validate_directory(str(tmp_path))
    assert summary.total == 0
    assert tv.summary_exit_code(summary) == 2


def test_a_subprocess_timeout_is_typed(monkeypatch) -> None:
    def time_out(cmd, **_kwargs):
        raise subprocess.TimeoutExpired(cmd, 5)

    monkeypatch.setattr(tv.harness_memory_guard, "guarded_completed_process", time_out)
    stdout, stderr, returncode, harness_error = tv._run_subprocess(
        ["/bin/true"], timeout=5
    )
    assert (stdout, stderr, returncode) == ("", "", -1)
    assert harness_error is not None and harness_error.startswith("timed out after 5s")
