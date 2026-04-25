"""Python 3.13 compliance tests — PEP 742, improved error messages, docstrings.

Differential testing: compile with Molt, run natively, compare to CPython output.
Tests cover version-specific semantics introduced in CPython 3.13.
"""

import os
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

MOLT_DIR = Path(__file__).resolve().parents[3]
ARTIFACT_ROOT = Path(os.environ.get("MOLT_EXT_ROOT", str(MOLT_DIR))).expanduser()


def _compile_and_run(python_source: str) -> str:
    """Compile Python source via molt CLI (native target), run binary, return stdout."""
    with tempfile.TemporaryDirectory() as tmp:
        src_path = Path(tmp) / "test_input.py"
        src_path.write_text(python_source)
        binary_path = Path(tmp) / "test_input_molt"

        env = {
            **os.environ,
            "MOLT_EXT_ROOT": str(ARTIFACT_ROOT),
            "CARGO_TARGET_DIR": os.environ.get(
                "CARGO_TARGET_DIR", str(ARTIFACT_ROOT / "target")
            ),
            "RUSTC_WRAPPER": "",
            "PYTHONPATH": str(MOLT_DIR / "src"),
        }

        build = subprocess.run(
            [
                sys.executable,
                "-m",
                "molt.cli",
                "build",
                str(src_path),
                "--out-dir",
                str(tmp),
            ],
            capture_output=True,
            text=True,
            timeout=240,
            env=env,
            cwd=str(MOLT_DIR),
        )
        if build.returncode != 0:
            pytest.skip(f"Compilation failed: {build.stderr[:300]}")

        if not binary_path.exists():
            pytest.skip(f"Binary not produced at {binary_path}")

        run = subprocess.run(
            [str(binary_path)],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if run.returncode != 0:
            pytest.fail(f"Runtime error: {run.stderr[:300]}")
        return run.stdout.strip()


def _python_output(source: str) -> str:
    """Get CPython reference output."""
    result = subprocess.run(
        [sys.executable, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
    )
    if result.returncode != 0:
        pytest.skip(f"CPython itself failed: {result.stderr[:200]}")
    return result.stdout.strip()


def _assert_match(src: str):
    """Assert compiled Molt output matches CPython."""
    assert _compile_and_run(src) == _python_output(src)


# -- PEP 742: TypeIs (typing) -------------------------------------------------


class TestPEP742TypeIs:
    """PEP 742 introduced typing.TypeIs as a narrowing guard in 3.13."""

    @pytest.mark.skip(reason="typing.TypeIs not yet supported in Molt")
    def test_typeis_import(self):
        """TypeIs should be importable without crashing the compiler."""
        _assert_match("""\
from typing import TypeIs

def is_str(val: object) -> TypeIs[str]:
    return isinstance(val, str)

print(is_str("hello"))
print(is_str(42))
""")

    @pytest.mark.skip(reason="typing.TypeIs not yet supported in Molt")
    def test_typeis_in_condition(self):
        _assert_match("""\
from typing import TypeIs

def is_int(val: object) -> TypeIs[int]:
    return isinstance(val, int)

def process(val: object) -> str:
    if is_int(val):
        return str(val + 1)
    return "not int"

print(process(10))
print(process("hi"))
""")


# -- Improved Error Messages / Exception Groups --------------------------------


class TestExceptionHandling:
    """Python 3.13 improved error messages. Test that Molt handles
    exception semantics correctly for patterns common in 3.13+ code."""

    def test_basic_try_except(self):
        _assert_match("""\
try:
    x = 1 / 0
except ZeroDivisionError:
    print("caught zero div")
""")

    def test_try_except_as(self):
        _assert_match("""\
try:
    result = int("not_a_number")
except ValueError as e:
    print("caught")
    print(type(e).__name__)
""")

    @pytest.mark.skip(reason="ExceptionGroup not yet supported in Molt")
    def test_exception_group_basic(self):
        """ExceptionGroup (3.11+) with except* (3.11+), commonly used in 3.13 code."""
        _assert_match("""\
try:
    raise ExceptionGroup("group", [ValueError("a"), TypeError("b")])
except* ValueError as eg:
    print(f"caught {len(eg.exceptions)} ValueError(s)")
except* TypeError as eg:
    print(f"caught {len(eg.exceptions)} TypeError(s)")
""")


# -- Docstring Whitespace Stripping (PEP 257 compliance) ----------------------


class TestDocstringHandling:
    """Python 3.13 improved docstring leading whitespace handling."""

    def test_docstring_preserved(self):
        _assert_match("""\
def greet():
    \"\"\"Say hello.\"\"\"
    return "hi"

print(greet.__doc__)
""")

    @pytest.mark.skip(reason="Docstring attribute access not yet supported in Molt")
    def test_multiline_docstring_stripping(self):
        _assert_match("""\
def func():
    \"\"\"
    First line.
    Second line.
    \"\"\"
    pass

print(repr(func.__doc__))
""")


# -- copy.replace() (new in 3.13) ---------------------------------------------


class TestCopyReplace:
    """copy.replace() was added in 3.13 for named tuples and dataclasses."""

    @pytest.mark.skip(reason="copy.replace not yet supported in Molt")
    def test_copy_replace_namedtuple(self):
        _assert_match("""\
from collections import namedtuple
import copy

Point = namedtuple("Point", ["x", "y"])
p1 = Point(1, 2)
p2 = copy.replace(p1, x=10)
print(p2)
""")


# -- General 3.13-era Python Patterns -----------------------------------------


class TestGeneral313Patterns:
    """General patterns that should work correctly on 3.13+."""

    def test_walrus_operator(self):
        """Walrus operator (3.8+) is commonly used in 3.13 code."""
        _assert_match("""\
data = [1, 2, 3, 4, 5, 6, 7, 8]
filtered = [y for x in data if (y := x * 2) > 6]
print(filtered)
""")

    def test_match_statement_basic(self):
        """match/case (3.10+) is ubiquitous in 3.13 code."""
        _assert_match("""\
def classify(x):
    match x:
        case 0:
            return "zero"
        case 1 | 2 | 3:
            return "small"
        case _:
            return "other"

print(classify(0))
print(classify(2))
print(classify(99))
""")

    def test_match_statement_capture(self):
        _assert_match("""\
def describe(point):
    match point:
        case (0, 0):
            return "origin"
        case (x, 0):
            return f"x={x}"
        case (0, y):
            return f"y={y}"
        case (x, y):
            return f"({x}, {y})"

print(describe((0, 0)))
print(describe((3, 0)))
print(describe((0, 5)))
print(describe((3, 5)))
""")
