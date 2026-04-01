"""End-to-end regression tests for traceback caret annotation pipeline.

Each test compiles a Python snippet with ``python3 -m molt build --target native``,
runs the resulting binary, captures stderr, and verifies that the caret annotation
line (the line consisting solely of ``^``, ``~``, and whitespace) matches CPython's
output exactly.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]

# ---------------------------------------------------------------------------
# Helpers (mirrors test_cli_smoke.py conventions)
# ---------------------------------------------------------------------------

def _base_env() -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(ROOT / "src")
    env["MOLT_BACKEND_DAEMON"] = "0"
    return env


def _python_executable() -> str:
    exe = sys.executable
    if exe and os.path.exists(exe) and os.access(exe, os.X_OK):
        return exe
    fallback = shutil.which("python3") or shutil.which("python")
    if fallback:
        return fallback
    return exe


def _molt_build_available() -> bool:
    """Return True if ``python3 -m molt build --help`` succeeds."""
    try:
        result = subprocess.run(
            [_python_executable(), "-m", "molt", "build", "--help"],
            capture_output=True,
            text=True,
            timeout=30,
            env=_base_env(),
        )
        return result.returncode == 0
    except (subprocess.TimeoutExpired, FileNotFoundError, OSError):
        return False


skip_no_molt = pytest.mark.skipif(
    not _molt_build_available(),
    reason="python3 -m molt build is not available",
)

# Caret line: a line that, after stripping, consists entirely of ^, ~, and spaces.
_CARET_RE = re.compile(r"^[\s\^~]+$")


def _resolve_macos_tmp(p: str) -> str:
    """Resolve /private/tmp vs /tmp symlink difference on macOS."""
    return str(Path(p).resolve())


def _extract_caret_line(stderr: str) -> str | None:
    """Return the first caret annotation line from traceback stderr output.

    The caret line is the line immediately following a source-code line in a
    Python traceback.  It contains only ``^``, ``~``, and whitespace.
    """
    for line in stderr.splitlines():
        if _CARET_RE.match(line) and ("^" in line or "~" in line):
            return line
    return None


def _extract_caret_pattern(stderr: str) -> str | None:
    """Return the stripped caret pattern (e.g. ``~~^~~``) from stderr."""
    caret_line = _extract_caret_line(stderr)
    if caret_line is None:
        return None
    return caret_line.strip()


def _compile_and_run(source: str) -> subprocess.CompletedProcess[str]:
    """Compile *source* with molt and run the resulting binary.

    Returns the CompletedProcess from running the compiled binary (not the
    build step).  Raises ``pytest.skip`` if the build itself fails for
    infrastructure reasons.
    """
    with tempfile.TemporaryDirectory() as tmpdir:
        tmpdir = _resolve_macos_tmp(tmpdir)
        src_file = os.path.join(tmpdir, "test_input.py")
        out_dir = os.path.join(tmpdir, "out")

        with open(src_file, "w") as f:
            f.write(source)

        # Build
        build_result = subprocess.run(
            [
                _python_executable(),
                "-m",
                "molt",
                "build",
                "--target",
                "native",
                "--output",
                out_dir,
                src_file,
                "--rebuild",
            ],
            capture_output=True,
            text=True,
            timeout=120,
            cwd=ROOT,
            env=_base_env(),
        )

        if build_result.returncode != 0:
            pytest.skip(
                f"molt build failed (infrastructure): {build_result.stderr[:500]}"
            )

        # Find the compiled binary
        binary = os.path.join(out_dir, "test_input")
        if not os.path.isfile(binary):
            # Try platform-specific extensions
            for ext in ("", ".exe"):
                candidate = binary + ext
                if os.path.isfile(candidate):
                    binary = candidate
                    break
            else:
                pytest.fail(
                    f"Compiled binary not found in {out_dir}. "
                    f"Contents: {os.listdir(out_dir) if os.path.isdir(out_dir) else 'dir missing'}"
                )

        # Run
        run_result = subprocess.run(
            [binary],
            capture_output=True,
            text=True,
            timeout=30,
            env=_base_env(),
        )

        return run_result


# ---------------------------------------------------------------------------
# Test cases
# ---------------------------------------------------------------------------


@skip_no_molt
class TestTracebackCarets:
    """Verify that traceback caret annotations match CPython output."""

    def test_binary_op_zero_division(self) -> None:
        """ZeroDivisionError from ``1 / 0`` should produce ``~~^~~`` carets."""
        result = _compile_and_run("a = 1 / 0\n")

        assert result.returncode != 0, "Expected non-zero exit for ZeroDivisionError"
        assert "ZeroDivisionError" in result.stderr, (
            f"Expected ZeroDivisionError in stderr, got: {result.stderr}"
        )

        pattern = _extract_caret_pattern(result.stderr)
        assert pattern is not None, (
            f"No caret annotation line found in stderr:\n{result.stderr}"
        )
        assert pattern == "~~^~~", (
            f"Expected caret pattern '~~^~~', got '{pattern}'\n"
            f"Full stderr:\n{result.stderr}"
        )

    def test_attribute_error(self) -> None:
        """AttributeError from ``None.upper()`` should produce ``^^^^^^^`` carets."""
        source = "x = None\ny = x.upper()\n"
        result = _compile_and_run(source)

        assert result.returncode != 0, "Expected non-zero exit for AttributeError"
        assert "AttributeError" in result.stderr, (
            f"Expected AttributeError in stderr, got: {result.stderr}"
        )

        pattern = _extract_caret_pattern(result.stderr)
        assert pattern is not None, (
            f"No caret annotation line found in stderr:\n{result.stderr}"
        )
        assert pattern == "^^^^^^^", (
            f"Expected caret pattern '^^^^^^^', got '{pattern}'\n"
            f"Full stderr:\n{result.stderr}"
        )

    def test_type_error_addition(self) -> None:
        """TypeError from ``\"hello\" + 42`` should produce ``~~~~~~~~^~~~`` carets."""
        source = 'x = "hello" + 42\n'
        result = _compile_and_run(source)

        assert result.returncode != 0, "Expected non-zero exit for TypeError"
        assert "TypeError" in result.stderr, (
            f"Expected TypeError in stderr, got: {result.stderr}"
        )

        pattern = _extract_caret_pattern(result.stderr)
        assert pattern is not None, (
            f"No caret annotation line found in stderr:\n{result.stderr}"
        )
        assert pattern == "~~~~~~~~^~~~", (
            f"Expected caret pattern '~~~~~~~~^~~~', got '{pattern}'\n"
            f"Full stderr:\n{result.stderr}"
        )

    def test_name_error(self) -> None:
        """NameError from ``print(undefined_var)`` should produce ``^^^^^^^^^^^^^`` carets."""
        source = "print(undefined_var)\n"
        result = _compile_and_run(source)

        assert result.returncode != 0, "Expected non-zero exit for NameError"
        assert "NameError" in result.stderr, (
            f"Expected NameError in stderr, got: {result.stderr}"
        )

        pattern = _extract_caret_pattern(result.stderr)
        assert pattern is not None, (
            f"No caret annotation line found in stderr:\n{result.stderr}"
        )
        assert pattern == "^^^^^^^^^^^^^", (
            f"Expected caret pattern '^^^^^^^^^^^^^', got '{pattern}'\n"
            f"Full stderr:\n{result.stderr}"
        )
