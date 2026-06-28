# MOLT_META: area=property-testing
"""Shared fixtures for Molt property-based tests.

Provides helpers to compile+run Python snippets through both Molt and CPython,
then compare outputs for equivalence.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest
from hypothesis import settings as hypothesis_settings

from molt.dx import development_artifact_env
from tools import harness_memory_guard

_REPO_ROOT = Path(__file__).resolve().parents[2]
_SRC_DIR = _REPO_ROOT / "src"


def _property_env(session_id: str) -> dict[str, str]:
    env = development_artifact_env(
        _REPO_ROOT,
        os.environ,
        session_prefix="property",
        session_id=os.environ.get("MOLT_SESSION_ID") or session_id,
        create_dirs=True,
    )
    env["PYTHONPATH"] = str(_SRC_DIR)
    return env


def _run_property_process(
    args: list[str],
    *,
    timeout: float,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    result = harness_memory_guard.guarded_completed_process(
        args,
        prefix="MOLT_PROPERTY",
        env=os.environ if env is None else env,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    if (
        result.returncode == harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE
        and "memory_guard: timeout after" in (result.stderr or "")
    ):
        raise subprocess.TimeoutExpired(
            args,
            timeout,
            output=result.stdout,
            stderr=result.stderr,
        )
    return result


# ---------------------------------------------------------------------------
# Molt availability detection
# ---------------------------------------------------------------------------


def _molt_cli_available() -> bool:
    """Return True if Molt can compile a trivial program."""
    tmp_path = None
    try:
        with tempfile.NamedTemporaryFile(suffix=".py", mode="w", delete=False) as f:
            f.write("print('ok')\n")
            f.flush()
            tmp_path = f.name

        env = _property_env("property-availability")

        result = _run_property_process(
            [
                sys.executable,
                "-m",
                "molt.cli",
                "build",
                "--profile",
                "dev",
                tmp_path,
            ],
            capture_output=True,
            text=True,
            timeout=120,
            env=env,
        )
        return result.returncode == 0
    except Exception:
        return False
    finally:
        if tmp_path is not None:
            try:
                os.unlink(tmp_path)
            except OSError:
                pass


_MOLT_AVAILABLE: bool | None = None


def molt_is_available() -> bool:
    global _MOLT_AVAILABLE
    if _MOLT_AVAILABLE is None:
        _MOLT_AVAILABLE = _molt_cli_available()
    return _MOLT_AVAILABLE


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


def run_via_molt(code: str, *, timeout: float = 60.0) -> str:
    """Compile *code* through Molt and return its stdout.

    Writes code to a temp file, invokes ``molt.cli build --profile dev``,
    then executes the resulting binary.  Returns stripped stdout.
    Raises ``subprocess.CalledProcessError`` on non-zero exit.
    """
    tmpdir = tempfile.mkdtemp(prefix="molt_prop_")
    try:
        src_file = os.path.join(tmpdir, "prop_test.py")
        with open(src_file, "w") as f:
            f.write(code)

        env = _property_env(f"property-run-{os.getpid()}")

        # Build
        build_result = _run_property_process(
            [
                sys.executable,
                "-m",
                "molt.cli",
                "build",
                "--profile",
                "dev",
                src_file,
            ],
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
        )
        if build_result.returncode != 0:
            raise subprocess.CalledProcessError(
                build_result.returncode,
                build_result.args,
                build_result.stdout,
                build_result.stderr,
            )

        # Find binary — Molt places it next to the source or in a known location
        binary_name = "prop_test"
        binary_candidates = [
            os.path.join(tmpdir, binary_name),
            os.path.join(tmpdir, binary_name + "_molt"),
        ]
        binary_path = None
        for cand in binary_candidates:
            if os.path.isfile(cand) and os.access(cand, os.X_OK):
                binary_path = cand
                break

        if binary_path is None:
            # Search tmpdir for any executable
            for entry in os.listdir(tmpdir):
                full = os.path.join(tmpdir, entry)
                if (
                    os.path.isfile(full)
                    and os.access(full, os.X_OK)
                    and not entry.endswith(".py")
                ):
                    binary_path = full
                    break

        if binary_path is None:
            raise FileNotFoundError(
                f"Molt binary not found in {tmpdir}. "
                f"Build stderr: {build_result.stderr}"
            )

        # Run
        run_result = _run_property_process(
            [binary_path],
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
        )
        if run_result.returncode != 0:
            raise subprocess.CalledProcessError(
                run_result.returncode,
                run_result.args,
                run_result.stdout,
                run_result.stderr,
            )
        return run_result.stdout.strip()
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


def run_via_cpython(code: str, *, timeout: float = 30.0) -> str:
    """Run *code* through CPython and return its stdout."""
    result = _run_property_process(
        [sys.executable, "-c", code],
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    if result.returncode != 0:
        raise subprocess.CalledProcessError(
            result.returncode,
            result.args,
            result.stdout,
            result.stderr,
        )
    return result.stdout.strip()


def assert_molt_matches_cpython(code: str) -> None:
    """Assert that Molt and CPython produce identical stdout for *code*."""
    molt_out = run_via_molt(code)
    cpython_out = run_via_cpython(code)
    assert molt_out == cpython_out, (
        f"Molt/CPython mismatch:\n"
        f"  Code: {code!r}\n"
        f"  Molt:    {molt_out!r}\n"
        f"  CPython: {cpython_out!r}"
    )
