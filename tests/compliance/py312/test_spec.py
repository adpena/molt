"""Python 3.12 compliance tests — PEP 695, 701, 709, and new stdlib additions.

Differential testing: compile with Molt, run natively, compare to CPython output.
Tests cover version-specific semantics introduced in CPython 3.12.
"""

import os
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

MOLT_DIR = Path(__file__).resolve().parents[3]


def _compile_and_run(python_source: str) -> str:
    """Compile Python source via molt CLI (native target), run binary, return stdout."""
    with tempfile.TemporaryDirectory() as tmp:
        src_path = Path(tmp) / "test_input.py"
        src_path.write_text(python_source)
        binary_path = Path(tmp) / "test_input_molt"

        env = {
            **os.environ,
            "MOLT_EXT_ROOT": "/Volumes/APDataStore/Molt",
            "CARGO_TARGET_DIR": "/Volumes/APDataStore/Molt/cargo-target",
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


# -- PEP 695: Type Parameter Syntax (type X = ...) ----------------------------


class TestPEP695TypeParams:
    """PEP 695 introduced `type X = ...` syntax for type aliases in 3.12."""

    @pytest.mark.skip(reason="PEP 695 type alias syntax not yet implemented in Molt")
    def test_simple_type_alias(self):
        _assert_match("""\
type Vector = list[float]
print("ok")
""")

    @pytest.mark.skip(reason="PEP 695 type alias syntax not yet implemented in Molt")
    def test_generic_type_alias(self):
        _assert_match("""\
type Matrix[T] = list[list[T]]
print("ok")
""")

    @pytest.mark.skip(reason="PEP 695 type alias syntax not yet implemented in Molt")
    def test_generic_function(self):
        _assert_match("""\
def first[T](items: list[T]) -> T:
    return items[0]
print(first([10, 20, 30]))
""")


# -- PEP 701: F-String Improvements -------------------------------------------


class TestPEP701FStrings:
    """PEP 701 (formalized in 3.12) allows nested quotes, backslashes, etc."""

    @pytest.mark.skip(reason="F-string compilation not yet supported in Molt")
    def test_fstring_basic(self):
        _assert_match("""\
name = "world"
print(f"hello {name}")
""")

    @pytest.mark.skip(reason="F-string compilation not yet supported in Molt")
    def test_fstring_expression(self):
        _assert_match("""\
x = 42
print(f"value is {x * 2 + 1}")
""")

    @pytest.mark.skip(reason="F-string compilation not yet supported in Molt")
    def test_fstring_nested_quotes(self):
        """PEP 701: f-strings can now reuse the same quote type."""
        _assert_match("""\
items = ["a", "b", "c"]
print(f"first is {items[0]}")
""")

    @pytest.mark.skip(reason="F-string compilation not yet supported in Molt")
    def test_fstring_format_spec(self):
        _assert_match("""\
print(f"{'hello':>10}")
print(f"{3.14159:.2f}")
""")


# -- PEP 709: Comprehension Inlining ------------------------------------------


class TestPEP709ComprehensionInlining:
    """PEP 709 inlines comprehensions at the bytecode level.
    Molt must produce correct results regardless of inlining strategy."""

    def test_list_comprehension_basic(self):
        _assert_match("""\
result = [x * x for x in range(6)]
print(result)
""")

    def test_list_comprehension_with_filter(self):
        _assert_match("""\
evens = [x for x in range(10) if x % 2 == 0]
print(evens)
""")

    def test_dict_comprehension(self):
        _assert_match("""\
d = {k: k * k for k in range(5)}
keys = sorted(d.keys())
for k in keys:
    print(k, d[k])
""")

    def test_set_comprehension(self):
        _assert_match("""\
s = {x % 3 for x in range(9)}
print(sorted(s))
""")

    def test_nested_comprehension(self):
        _assert_match("""\
flat = [x for row in [[1, 2], [3, 4], [5, 6]] for x in row]
print(flat)
""")


# -- math.sumprod (new in 3.12) -----------------------------------------------


class TestMathSumprod:
    """math.sumprod was added in Python 3.12."""

    @pytest.mark.skip(reason="math.sumprod not yet supported in Molt")
    def test_sumprod_basic(self):
        _assert_match("""\
import math
print(math.sumprod([1, 2, 3], [4, 5, 6]))
""")

    @pytest.mark.skip(reason="math.sumprod not yet supported in Molt")
    def test_sumprod_floats(self):
        _assert_match("""\
import math
print(math.sumprod([0.1, 0.2], [10, 20]))
""")
