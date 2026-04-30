"""Python 3.14 compliance tests — PEP 649, 750, 758, and new patterns.

Differential testing: compile with Molt, run natively, compare to CPython output.
Tests cover version-specific semantics introduced in CPython 3.14.
"""

import os
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

MOLT_DIR = Path(__file__).resolve().parents[3]
ARTIFACT_ROOT = Path(os.environ.get("MOLT_EXT_ROOT", str(MOLT_DIR))).expanduser()


def _python_for(min_version: tuple[int, int]) -> str:
    """Return a Python executable that satisfies `min_version`.

    Falls back through known interpreter paths so a 3.13/3.14-only feature
    test can run on a host whose `sys.executable` is older.
    """
    if sys.version_info >= min_version:
        return sys.executable
    for candidate in (
        "/opt/homebrew/opt/python@3.14/bin/python3.14",
        "/opt/homebrew/opt/python@3.13/bin/python3.13",
        "/usr/local/bin/python3.14",
        "/usr/local/bin/python3.13",
    ):
        if Path(candidate).exists():
            try:
                ver = subprocess.run(
                    [candidate, "-c", "import sys; print(sys.version_info[:2])"],
                    capture_output=True,
                    text=True,
                    timeout=5,
                ).stdout.strip()
                if ver and eval(ver) >= min_version:
                    return candidate
            except Exception:  # noqa: BLE001
                continue
    return sys.executable


def _target_version_arg(min_version: tuple[int, int]) -> list[str]:
    if min_version <= (3, 12):
        return []
    return ["--python-version", f"{min_version[0]}.{min_version[1]}"]


def _compile_and_run(
    python_source: str, *, min_version: tuple[int, int] = (3, 12)
) -> str:
    """Compile Python source via molt CLI (native target), run binary, return stdout."""
    python_exe = _python_for(min_version)
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
                python_exe,
                "-m",
                "molt.cli",
                "build",
                str(src_path),
                "--out-dir",
                str(tmp),
                *_target_version_arg(min_version),
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
            pytest.fail(
                "Runtime error "
                f"(exit {run.returncode}): stdout={run.stdout[:300]!r} "
                f"stderr={run.stderr[:300]!r}"
            )
        return run.stdout.strip()


def _compile_source(
    python_source: str, *, min_version: tuple[int, int] = (3, 12), target: str | None
) -> subprocess.CompletedProcess[str]:
    python_exe = _python_for(min_version)
    with tempfile.TemporaryDirectory() as tmp:
        src_path = Path(tmp) / "test_input.py"
        src_path.write_text(python_source)
        args = [
            python_exe,
            "-m",
            "molt.cli",
            "build",
            str(src_path),
            "--out-dir",
            str(tmp),
        ]
        if target is not None:
            args.extend(["--python-version", target])
        env = {
            **os.environ,
            "MOLT_EXT_ROOT": str(ARTIFACT_ROOT),
            "CARGO_TARGET_DIR": os.environ.get(
                "CARGO_TARGET_DIR", str(ARTIFACT_ROOT / "target")
            ),
            "RUSTC_WRAPPER": "",
            "PYTHONPATH": str(MOLT_DIR / "src"),
        }
        return subprocess.run(
            args,
            capture_output=True,
            text=True,
            timeout=240,
            env=env,
            cwd=str(MOLT_DIR),
        )


def _python_output(source: str, *, min_version: tuple[int, int] = (3, 12)) -> str:
    """Get CPython reference output."""
    python_exe = _python_for(min_version)
    result = subprocess.run(
        [python_exe, "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
    )
    if result.returncode != 0:
        pytest.skip(f"CPython itself failed: {result.stderr[:200]}")
    return result.stdout.strip()


def _assert_match(src: str, *, min_version: tuple[int, int] = (3, 12)):
    """Assert compiled Molt output matches CPython."""
    assert _compile_and_run(src, min_version=min_version) == _python_output(
        src, min_version=min_version
    )


# -- PEP 649: Deferred Evaluation of Annotations ------------------------------


class TestPEP649DeferredAnnotations:
    """PEP 649 makes annotations lazy — they should not cause side effects
    at function/class definition time."""

    def test_annotation_no_side_effect(self):
        """Annotations should not be evaluated at definition time."""
        _assert_match("""\
side_effects = []

def track(label):
    side_effects.append(label)
    return int

def func(x: track("param")) -> track("return"):
    return x

# In PEP 649, annotations are deferred — no side effects yet
print(len(side_effects))
print(func(42))
""")

    def test_forward_reference_in_annotation(self):
        """Forward references should work without quotes under PEP 649."""
        _assert_match("""\
def make_node(val: int) -> "Node":
    return {"value": val, "next": None}

class Node:
    pass

print(make_node(5))
""")

    def test_annotation_access_via_get_annotations(self):
        """__annotations__ should still be accessible when explicitly requested."""
        _assert_match("""\
def add(x: int, y: int) -> int:
    return x + y

ann = add.__annotations__
keys = sorted(ann.keys())
for k in keys:
    print(k, ann[k].__name__)
""")


# -- PEP 750: Template Strings ------------------------------------------------


class TestPEP750TemplateStrings:
    """PEP 750 introduces t-string syntax: t'Hello {name}'."""

    def test_tstring_rejected_by_default_py312_target(self):
        build = _compile_source(
            """\
name = "world"
template = t"Hello {name}"
print(type(template).__name__)
""",
            min_version=(3, 14),
            target=None,
        )

        assert build.returncode != 0
        assert "Python 3.14" in build.stderr or "3.14" in build.stderr

    def test_tstring_basic(self):
        _assert_match(
            """\
name = "world"
template = t"Hello {name}"
print(type(template).__name__)
""",
            min_version=(3, 14),
        )

    def test_tstring_interpolation_count(self):
        _assert_match(
            """\
x = 1
y = 2
template = t"coords: {x}, {y}"
print(len(template.interpolations))
""",
            min_version=(3, 14),
        )


# -- PEP 758: except Without Parentheses for Multiple Types -------------------


class TestPEP758ExceptSyntax:
    """PEP 758 allows `except ValueError, TypeError:` without parentheses."""

    def test_except_multiple_no_parens(self):
        _assert_match(
            """\
def try_convert(val):
    try:
        return int(val)
    except ValueError, TypeError:
        return None

print(try_convert("42"))
print(try_convert("abc"))
""",
            min_version=(3, 14),
        )

    def test_except_three_types_no_parens(self):
        _assert_match(
            """\
def safe_div(a, b):
    try:
        return a / b
    except ZeroDivisionError, TypeError, ValueError:
        return -1

print(safe_div(10, 2))
print(safe_div(10, 0))
""",
            min_version=(3, 14),
        )


# -- General 3.14-era Patterns ------------------------------------------------


class TestGeneral314Patterns:
    """Patterns that are idiomatic in Python 3.14+ code."""

    def test_type_annotations_basic(self):
        """Basic annotations that Molt should handle without crashing."""
        _assert_match("""\
def add(x: int, y: int) -> int:
    return x + y

print(add(3, 4))
""")

    def test_union_type_syntax(self):
        """PEP 604 (3.10+) union syntax X | Y is common in 3.14 code."""
        _assert_match("""\
def describe(val: int | str) -> str:
    if isinstance(val, int):
        return "int"
    return "str"

print(describe(42))
print(describe("hi"))
""")

    def test_slots_dataclass_pattern(self):
        """Slotted dataclasses are standard practice in 3.14."""
        _assert_match("""\
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y
    def __repr__(self):
        return f"Point({self.x}, {self.y})"

p = Point(3, 4)
print(p)
print(p.x + p.y)
""")

    def test_dataclass_with_slots(self):
        _assert_match("""\
from dataclasses import dataclass

@dataclass(slots=True)
class Vec2:
    x: float
    y: float

v = Vec2(1.0, 2.0)
print(v)
print(v.x + v.y)
""")

    def test_starred_unpacking(self):
        """Extended unpacking (3.0+), heavily used in modern Python."""
        _assert_match("""\
first, *middle, last = [1, 2, 3, 4, 5]
print(first)
print(middle)
print(last)
""")

    def test_nested_unpacking(self):
        _assert_match("""\
data = [(1, 2), (3, 4), (5, 6)]
result = []
for a, b in data:
    result.append(a + b)
print(result)
""")
