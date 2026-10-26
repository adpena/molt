"""Shared fixtures for Molt property-based tests.

Provides helpers to compile+run Python snippets through both Molt and CPython,
then compare outputs for equivalence.
"""

from __future__ import annotations

from collections.abc import Mapping

from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder

import os
import json
import subprocess
import sys
from pathlib import Path

import pytest
from hypothesis import settings as hypothesis_settings

from molt.dx import development_artifact_env
from molt.temporary_artifacts import OwnedTemporaryDirectory
from tests.process_guard_common import run_guarded_test_process
from tools.compat.backends import BackendResult
from tools.compat.comparison import Outputs, compare_outputs

_REPO_ROOT = Path(__file__).resolve().parents[2]
_SRC_DIR = _REPO_ROOT / "src"


def _property_env(session_id: str) -> ProcessEnvironment:
    env = ProcessEnvironmentBuilder(
        development_artifact_env(
            _REPO_ROOT,
            os.environ,
            session_prefix="property",
            session_id=os.environ.get("MOLT_SESSION_ID") or session_id,
            create_dirs=True,
        )
    )
    env["PYTHONPATH"] = str(_SRC_DIR)
    return ProcessEnvironment(env)


def _run_property_process(
    args: list[str],
    *,
    timeout: float | None = None,
    env: Mapping[str, str] | None = None,
) -> BackendResult:
    process = run_guarded_test_process(
        args,
        prefix="MOLT_PROPERTY",
        env=os.environ if env is None else env,
        capture_output=True,
        text=False,
        timeout=timeout,
    )
    result = BackendResult.from_process(process)
    if (
        process.cancelled
        or not process.descendants_closed
        or result.timed_out
        or result.infrastructure_failure is not None
        or result.rss_limit_exceeded
        or result.guard_signal is not None
        or result.stdout is None
    ):
        error = RuntimeError(
            f"Property process did not complete: {args!r}\n{result.stderr}"
        )
        setattr(error, "guarded_result", process)
        raise error
    return result


def _observable_outputs(result: BackendResult) -> Outputs:
    return Outputs(result.stdout, result.child_stderr or "", result.returncode)


def pytest_addoption(parser: pytest.Parser) -> None:
    parser.addoption(
        "--run-molt",
        action="store_true",
        default=False,
        help="Run tests that require a working Molt compiler",
    )
    parser.addoption(
        "--molt-max-examples",
        type=int,
        default=None,
        help="Override Hypothesis max_examples for Molt property test gates.",
    )


def pytest_collection_modifyitems(config: pytest.Config, items: list) -> None:
    max_examples = config.getoption("--molt-max-examples", default=None)
    if max_examples is not None:
        if max_examples <= 0:
            raise pytest.UsageError("--molt-max-examples must be greater than 0")
        for item in items:
            target = getattr(item.obj, "__func__", item.obj)
            current = getattr(target, "_hypothesis_internal_use_settings", None)
            if current is None:
                continue
            target._hypothesis_internal_use_settings = hypothesis_settings(
                parent=current,
                max_examples=max_examples,
            )
    if config.getoption("--run-molt", default=False):
        return
    skip_molt = pytest.mark.skip(reason="Need --run-molt to run Molt compilation tests")
    for item in items:
        if "molt_compile" in item.keywords:
            item.add_marker(skip_molt)


# ---------------------------------------------------------------------------
# Execution helpers
# ---------------------------------------------------------------------------


def run_via_molt(code: str, *, timeout: float = 60.0) -> Outputs:
    """Compile *code* and retain its exact stdout, stderr and guest exit status.

    Build and harness failures raise; they are never guest-equivalence results.
    The build's selected consumer path is the only executable admitted here.
    """
    # Keep the compiler dependency closure off the CPython-only property path.
    from molt.cli.wrapper_build import _parse_wrapper_build_contract_payload

    owner = OwnedTemporaryDirectory(prefix="molt_prop_")
    closed = True
    primary: BaseException | None = None
    try:
        tmpdir = owner.name
        src_file = Path(tmpdir) / "prop_test.py"
        src_file.write_text(code, encoding="utf-8")
        env = _property_env(f"property-run-{os.getpid()}")
        command = [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--profile",
            "dev",
            "--out-dir",
            str(Path(tmpdir) / "build"),
            "--json",
            str(src_file),
        ]
        closed = False
        build = _run_property_process(command, env=env)
        closed = True
        if build.returncode != 0:
            raise subprocess.CalledProcessError(
                build.returncode, command, build.stdout, build.stderr
            )
        payload = json.loads(build.stdout or "")
        contract, error = _parse_wrapper_build_contract_payload(
            payload, json_output=False, command="property"
        )
        if error is not None or contract is None:
            raise RuntimeError(
                "Molt property build did not publish an executable contract"
            )
        binary = contract.consumer_output
        if not binary.is_file():
            raise FileNotFoundError(
                f"Molt selected executable does not exist: {binary}"
            )
        closed = False
        result = _run_property_process([str(binary)], timeout=timeout, env=env)
        closed = True
        return _observable_outputs(result)
    except BaseException as error:
        primary = error
        guarded = getattr(error, "guarded_result", None)
        closed = closed or (guarded is not None and guarded.descendants_closed is True)
        raise
    finally:
        if closed:
            try:
                owner.cleanup()
            except BaseException as error:
                if primary is None:
                    raise
                primary.add_note(
                    f"Property scratch cleanup failed at {owner.name}: {error!r}"
                )
        else:
            owner.retain()
            if primary is not None:
                primary.add_note(
                    f"Property scratch retained pending process-tree closure: {owner.name}"
                )


def run_via_cpython(code: str, *, timeout: float = 30.0) -> Outputs:
    """Run *code* through CPython with complete, lossless observable output."""
    return _observable_outputs(
        _run_property_process([sys.executable, "-c", code], timeout=timeout)
    )


def assert_molt_matches_cpython(code: str) -> None:
    """Require exact stdout, stderr and exit status under the shared parity law."""
    molt_out = run_via_molt(code)
    cpython_out = run_via_cpython(code)
    verdict = compare_outputs(cpython_out, molt_out, stderr_mode="exact")
    assert verdict.equal, (
        f"Molt/CPython mismatch ({verdict.detail}):\n"
        f"  Code: {code!r}\n"
        f"  Molt:    {molt_out!r}\n"
        f"  CPython: {cpython_out!r}"
    )
