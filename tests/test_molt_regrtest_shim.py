from __future__ import annotations

import json
import shlex
import sys
import time
from pathlib import Path

import tools.molt_regrtest_shim as shim
from tests.process_guard_common import HUNG_CHILD_SOURCE


def test_build_env_enables_hermetic_module_roots(monkeypatch, tmp_path: Path) -> None:
    cpython_dir = tmp_path / "cpython"
    cpython_lib = cpython_dir / "Lib"
    cpython_lib.mkdir(parents=True)

    monkeypatch.setenv("PYTHONPATH", str(tmp_path / "ambient"))

    env = shim.build_env(cpython_dir)

    assert env["MOLT_HERMETIC_MODULE_ROOTS"] == "1"
    assert env["PYTHONNOUSERSITE"] == "1"
    assert env["MOLT_REGRTEST_CPYTHON_DIR"] == str(cpython_dir)
    # Only CPython's `test` package is a module root: all of Lib would shadow
    # Molt's own stdlib sources and fail every compiled test at startup.
    assert env["MOLT_MODULE_ROOTS"] == f"test={cpython_lib.resolve() / 'test'}"


def _cpython_with_test(tmp_path: Path, name: str) -> Path:
    cpython_dir = tmp_path / "cpython"
    test_dir = cpython_dir / "Lib" / "test"
    test_dir.mkdir(parents=True)
    (test_dir / "__init__.py").write_text("", encoding="utf-8")
    (test_dir / f"{name}.py").write_text("", encoding="utf-8")
    return cpython_dir


def _worker(
    tmp_path: Path,
    capsys,
    code: str,
    *,
    timeout: float,
    admit_until: float | None = None,
) -> tuple[dict, dict]:
    log = tmp_path / "results.jsonl"
    rc = shim.run_worker(
        json.dumps({"tests": ["test_probe"], "timeout": timeout}),
        shlex.join([sys.executable, "-c", code]),
        _cpython_with_test(tmp_path, "test_probe"),
        [],
        results_jsonl=log,
        admit_until=admit_until,
    )
    assert rc == 0
    result = json.loads(capsys.readouterr().out.strip().splitlines()[-1])
    (row,) = [json.loads(line) for line in log.read_text(encoding="utf-8").splitlines()]
    return result, row


def test_worker_enforces_the_regrtest_test_timeout(tmp_path: Path, capsys) -> None:
    # CPython's own workers stop a test at --timeout through faulthandler;
    # regrtest kills a worker only at min(1.5 x timeout, timeout + 300 s).
    # The test never ends by itself, so only the timeout ends it; an elapsed
    # bound would also fail a healthy run on a stalled host.
    result, row = _worker(tmp_path, capsys, HUNG_CHILD_SOURCE, timeout=1)

    assert result["state"] == "TIMEOUT"
    assert (row["state"], row["reason"]) == ("TIMEOUT", "timeout")
    assert row["duration_s"] >= 1


def test_worker_refuses_a_test_that_cannot_finish_before_the_deadline(
    tmp_path: Path, capsys
) -> None:
    marker = tmp_path / "ran"
    result, row = _worker(
        tmp_path,
        capsys,
        f"open({str(marker)!r}, 'w').close()",
        timeout=600,
        admit_until=time.time() + 60,
    )

    assert not marker.exists()
    assert (result["state"], result["duration"]) == ("DID_NOT_RUN", None)
    assert row == {
        "test": "test_probe",
        "state": "DID_NOT_RUN",
        "duration_s": None,
        "reason": "deadline",
        "detail": "",
    }


def test_worker_runs_and_records_an_admitted_test(tmp_path: Path, capsys) -> None:
    result, row = _worker(
        tmp_path, capsys, "pass", timeout=60, admit_until=time.time() + 600
    )

    assert result["state"] == "PASSED"
    assert row["state"] == "PASSED" and row["reason"] == ""
    assert row["duration_s"] > 0


def test_worker_fails_a_test_that_molt_refuses_to_compile(
    tmp_path: Path, capsys
) -> None:
    # A compatibility refusal is not a CPython skip; reading it as one would
    # hide every test Molt cannot build.
    refusal = (
        "import sys; "
        "print('MOLT_COMPAT_ERROR: unsupported construct', file=sys.stderr); "
        "print('  feature: call to non-allowlisted function', file=sys.stderr); "
        "print('  location: Lib/test/support/__init__.py:1:1', file=sys.stderr); "
        "sys.exit(2)"
    )
    result, row = _worker(tmp_path, capsys, refusal, timeout=60)

    assert result["state"] == "FAILED"
    assert row["state"] == "FAILED"
    assert row["reason"] == (
        "MOLT_COMPAT_ERROR: call to non-allowlisted function "
        "@ Lib/test/support/__init__.py:1:1"
    )


def test_worker_names_the_diagnostic_that_ended_a_failed_build(
    tmp_path: Path, capsys
) -> None:
    failure = (
        "import sys; print('molt: Preparing build', file=sys.stderr); "
        "print('Compiler cache identity: boom', file=sys.stderr); sys.exit(2)"
    )
    result, row = _worker(tmp_path, capsys, failure, timeout=60)

    assert result["state"] == "FAILED"
    assert row["reason"] == "molt rc=2: Compiler cache identity: boom"
