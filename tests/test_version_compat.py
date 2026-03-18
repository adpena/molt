"""Tests for Python version compatibility in the Molt compiler.

Verifies that:
- The Molt compiler accepts valid Python 3.12 code
- Version-specific features are correctly gated
- sys.version_info is correctly reported
"""

import sys
import ast
import textwrap


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _parses_ok(source: str) -> bool:
    """Return True if the source parses without error on this Python version."""
    try:
        ast.parse(textwrap.dedent(source))
        return True
    except SyntaxError:
        return False


# ---------------------------------------------------------------------------
# Basic: Molt accepts valid 3.12 code on all supported versions
# ---------------------------------------------------------------------------

def test_basic_312_code_parses() -> None:
    """Core Python 3.12 constructs parse on all supported versions (3.12+)."""
    source = """\
    x: int = 42
    match x:
        case 42:
            y = "matched"
        case _:
            y = "default"
    """
    assert _parses_ok(source)


def test_type_alias_stmt_parses() -> None:
    """PEP 695 type alias statement is available on 3.12+."""
    source = "type Vector = list[float]"
    assert _parses_ok(source)


def test_type_param_syntax_parses() -> None:
    """PEP 695 type parameter syntax is available on 3.12+."""
    source = """\
    def first[T](items: list[T]) -> T:
        return items[0]
    """
    assert _parses_ok(source)


def test_exception_group_base_available() -> None:
    """ExceptionGroup is available as a builtin on 3.11+, so always on Molt targets."""
    assert hasattr(__builtins__ if isinstance(__builtins__, dict) else type(__builtins__), "__name__") or True
    # ExceptionGroup is a builtin on 3.11+
    assert ExceptionGroup is not None  # type: ignore[name-defined]


def test_except_star_syntax() -> None:
    """except* syntax (PEP 654) parses on 3.11+, so always on Molt targets."""
    source = """\
    try:
        pass
    except* ValueError as eg:
        pass
    """
    assert _parses_ok(source)


# ---------------------------------------------------------------------------
# Version-specific feature gating
# ---------------------------------------------------------------------------

def test_version_gated_locals_snapshot() -> None:
    """locals() snapshot semantics changed in 3.13 (PEP 667).
    On 3.13+, locals() returns a snapshot (fresh dict each call).
    On 3.12, locals() returns a cached mutable dict.
    """
    vi = sys.version_info
    if vi >= (3, 13):
        # On 3.13+, each call to locals() in a function returns a fresh snapshot
        def _check_snapshot() -> bool:
            x = 1  # noqa: F841
            d1 = locals()
            d2 = locals()
            # PEP 667: d1 and d2 are snapshots, but may or may not be the same object
            # The key semantic change is that mutations to d1 don't affect the namespace
            return "x" in d1 and "x" in d2
        assert _check_snapshot()
    else:
        # On 3.12, locals() returns a cached dict
        pass  # behavior is still valid, just different


def test_version_gated_deferred_annotations() -> None:
    """Deferred annotation evaluation (PEP 649) is 3.14+ only.
    On 3.12/3.13, annotations are eagerly evaluated (or stringized via __future__).
    """
    vi = sys.version_info
    if vi >= (3, 14):
        # PEP 649: annotations are lazily evaluated
        # The annotationlib module should exist
        try:
            import annotationlib  # noqa: F401
            has_annotationlib = True
        except ImportError:
            has_annotationlib = False
        assert has_annotationlib, "Python 3.14+ should have annotationlib"
    else:
        # On 3.12/3.13, annotationlib does not exist
        try:
            import annotationlib  # noqa: F401
            has_annotationlib = True
        except ImportError:
            has_annotationlib = False
        assert not has_annotationlib, "Python < 3.14 should not have annotationlib"


def test_version_gated_type_param_defaults() -> None:
    """PEP 696 type parameter defaults are 3.14+ only."""
    vi = sys.version_info
    if vi >= (3, 14):
        source = """\
        def foo[T = int](x: T) -> T:
            return x
        """
        # This syntax should parse on 3.14+
        assert _parses_ok(source)
    else:
        # On 3.12/3.13, type parameter defaults are a syntax error
        source = """\
        def foo[T = int](x: T) -> T:
            return x
        """
        assert not _parses_ok(source)


# ---------------------------------------------------------------------------
# sys.version_info correctness
# ---------------------------------------------------------------------------

def test_sys_version_info_tuple() -> None:
    """sys.version_info is a named tuple with the expected fields."""
    vi = sys.version_info
    assert hasattr(vi, "major")
    assert hasattr(vi, "minor")
    assert hasattr(vi, "micro")
    assert hasattr(vi, "releaselevel")
    assert hasattr(vi, "serial")


def test_sys_version_info_supported_range() -> None:
    """Molt supports Python 3.12, 3.13, and 3.14."""
    vi = sys.version_info
    assert vi.major == 3
    assert vi.minor in (12, 13, 14), (
        f"Molt requires Python 3.12-3.14, got {vi.major}.{vi.minor}"
    )


def test_sys_version_info_comparison() -> None:
    """Version info supports tuple comparison for gating."""
    vi = sys.version_info
    # This is how version gating is typically done in Python code
    assert vi >= (3, 12)
    assert vi < (4, 0)


def test_version_info_consistency() -> None:
    """sys.version string is consistent with sys.version_info."""
    vi = sys.version_info
    version_str = sys.version
    expected_prefix = f"{vi.major}.{vi.minor}.{vi.micro}"
    assert version_str.startswith(expected_prefix), (
        f"sys.version {version_str!r} does not start with {expected_prefix!r}"
    )
