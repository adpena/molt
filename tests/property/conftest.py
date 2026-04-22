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

_REPO_ROOT = Path(__file__).resolve().parents[2]
_SRC_DIR = _REPO_ROOT / "src"


def _artifact_root() -> Path:
    configured = os.environ.get("MOLT_EXT_ROOT", "").strip()
    if configured:
        return Path(configured).expanduser()
    return _REPO_ROOT


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

        env = os.environ.copy()
        env["PYTHONPATH"] = str(_SRC_DIR)
        cargo_target = os.environ.get(
            "CARGO_TARGET_DIR",
            str(_artifact_root() / "target"),
        )
        env.setdefault("CARGO_TARGET_DIR", cargo_target)

        result = subprocess.run(
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


def pytest_collection_modifyitems(config: pytest.Config, items: list) -> None:
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

        env = os.environ.copy()
        env["PYTHONPATH"] = str(_SRC_DIR)
        cargo_target = os.environ.get(
            "CARGO_TARGET_DIR",
            str(_artifact_root() / "target"),
        )
        env.setdefault("CARGO_TARGET_DIR", cargo_target)

        # Build
        build_result = subprocess.run(
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
        run_result = subprocess.run(
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
    result = subprocess.run(
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
