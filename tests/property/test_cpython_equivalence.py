# MOLT_META: area=property-testing
"""Property-based CPython equivalence tests.

The META property: for randomly generated simple expressions, Molt and
CPython must produce identical output.  Uses custom Hypothesis strategies
to generate arithmetic and string expressions, then compares subprocess
outputs.

These tests require ``--run-molt`` and a working Molt compiler.
"""

from __future__ import annotations

import pytest
from hypothesis import given, settings, HealthCheck
from hypothesis import strategies as st
from hypothesis.strategies import composite

# ---------------------------------------------------------------------------
# Custom strategies for expression generation
# ---------------------------------------------------------------------------


@composite
def arithmetic_expressions(draw: st.DrawFn, max_depth: int = 3) -> str:
    """Generate a simple arithmetic expression string.

    Produces expressions like ``(3 + 7)``, ``((2 * -5) - 1)``, etc.
    All operands are small integers to keep results manageable.
    """
    if max_depth <= 0 or draw(st.booleans()):
        # Leaf: a small integer literal
        n = draw(st.integers(min_value=-100, max_value=100))
        # Parenthesize negatives to avoid ambiguity
        return str(n) if n >= 0 else f"({n})"

    op = draw(st.sampled_from(["+", "-", "*"]))
    left = draw(arithmetic_expressions(max_depth=max_depth - 1))
    right = draw(arithmetic_expressions(max_depth=max_depth - 1))

    return f"({left} {op} {right})"


@composite
def safe_divmod_expressions(draw: st.DrawFn) -> str:
    """Generate a division or modulo expression with a guaranteed non-zero divisor."""
    left = draw(st.integers(min_value=-100, max_value=100))
    right = draw(st.integers(min_value=1, max_value=50))
    op = draw(st.sampled_from(["//", "%"]))
    left_s = str(left) if left >= 0 else f"({left})"
    return f"({left_s} {op} {right})"


@composite
def string_expressions(draw: st.DrawFn) -> str:
    """Generate a simple string expression that produces a string result.

    Produces expressions like ``'hello' + 'world'``, ``'ab' * 3``,
    ``'HELLO'.lower()``, etc.
    """
    base = draw(
        st.text(
            alphabet=st.characters(min_codepoint=32, max_codepoint=126),
            min_size=0,
            max_size=15,
        )
    )
    # Escape for embedding in Python source
    base_repr = repr(base)

    op = draw(
        st.sampled_from(
            [
                "identity",
                "upper",
                "lower",
                "strip",
                "title",
                "concat",
                "repeat",
                "replace",
            ]
        )
    )

    if op == "identity":
        return base_repr
    elif op == "upper":
        return f"{base_repr}.upper()"
    elif op == "lower":
        return f"{base_repr}.lower()"
    elif op == "strip":
        return f"{base_repr}.strip()"
    elif op == "title":
        return f"{base_repr}.title()"
    elif op == "concat":
        other = draw(
            st.text(
                alphabet=st.characters(min_codepoint=32, max_codepoint=126),
                min_size=0,
                max_size=10,
            )
        )
        return f"{base_repr} + {repr(other)}"
    elif op == "repeat":
        n = draw(st.integers(min_value=0, max_value=5))
        return f"{base_repr} * {n}"
    elif op == "replace":
        old_char = draw(st.characters(min_codepoint=32, max_codepoint=126))
        new_char = draw(st.characters(min_codepoint=32, max_codepoint=126))
        return f"{base_repr}.replace({repr(old_char)}, {repr(new_char)})"
    else:
        return base_repr


# ---------------------------------------------------------------------------
# CPython-only equivalence tests (no Molt compilation needed)
# ---------------------------------------------------------------------------


class TestArithmeticExpressionGeneration:
    """Verify that generated arithmetic expressions are valid Python."""

    @given(expr=arithmetic_expressions())
    @settings(
        max_examples=200,
        deadline=None,
        database=None,
        suppress_health_check=[HealthCheck.too_slow],
    )
    def test_arithmetic_expr_valid(self, expr: str) -> None:
        """Generated arithmetic expression produces output in CPython."""
        from tests.property.conftest import run_via_cpython

        code = f"print({expr})"
        result = run_via_cpython(code)
        # Should produce some output (a number)
        assert result != ""

    @given(expr=safe_divmod_expressions())
    @settings(
        max_examples=200,
        deadline=None,
        database=None,
        suppress_health_check=[HealthCheck.too_slow],
    )
    def test_divmod_expr_valid(self, expr: str) -> None:
        """Generated divmod expression produces output in CPython."""
        from tests.property.conftest import run_via_cpython

        code = f"print({expr})"
        result = run_via_cpython(code)
        assert result != ""

    @given(expr=string_expressions())
    @settings(
        max_examples=200,
        deadline=None,
        database=None,
        suppress_health_check=[HealthCheck.too_slow],
    )
    def test_string_expr_valid(self, expr: str) -> None:
        """Generated string expression produces output in CPython."""
        from tests.property.conftest import run_via_cpython

        code = f"print(repr({expr}))"
        result = run_via_cpython(code)
        assert result != ""


# ---------------------------------------------------------------------------
# Full Molt vs CPython equivalence (requires --run-molt)
# ---------------------------------------------------------------------------


@pytest.mark.molt_compile
class TestMoltCPythonArithmeticEquivalence:
    """For random arithmetic expressions, Molt output must match CPython."""

    @given(expr=arithmetic_expressions())
    @settings(
        max_examples=50,
        deadline=None,
        database=None,
        suppress_health_check=[HealthCheck.too_slow],
    )
    def test_arithmetic_equivalence(self, expr: str) -> None:
        """Molt and CPython produce identical output for arithmetic expressions."""
        from tests.property.conftest import assert_molt_matches_cpython

        code = f"print({expr})"
        assert_molt_matches_cpython(code)

    @given(expr=safe_divmod_expressions())
    @settings(
        max_examples=50,
        deadline=None,
        database=None,
        suppress_health_check=[HealthCheck.too_slow],
    )
    def test_divmod_equivalence(self, expr: str) -> None:
        """Molt and CPython produce identical output for divmod expressions."""
        from tests.property.conftest import assert_molt_matches_cpython

        code = f"print({expr})"
        assert_molt_matches_cpython(code)


@pytest.mark.molt_compile
class TestMoltCPythonStringEquivalence:
    """For random string expressions, Molt output must match CPython."""

    @given(expr=string_expressions())
    @settings(
        max_examples=50,
        deadline=None,
        database=None,
        suppress_health_check=[HealthCheck.too_slow],
    )
    def test_string_equivalence(self, expr: str) -> None:
        """Molt and CPython produce identical output for string expressions."""
        from tests.property.conftest import assert_molt_matches_cpython

        code = f"print(repr({expr}))"
        assert_molt_matches_cpython(code)


@pytest.mark.molt_compile
class TestMoltCPythonBuiltinEquivalence:
    """For simple builtin calls, Molt output must match CPython."""

    @given(n=st.integers(min_value=-1000, max_value=1000))
    @settings(max_examples=50, deadline=None, database=None)
    def test_abs_equivalence(self, n: int) -> None:
        """abs() matches between Molt and CPython."""
        from tests.property.conftest import assert_molt_matches_cpython

        assert_molt_matches_cpython(f"print(abs({n}))")

    @given(
        a=st.integers(min_value=-100, max_value=100),
        b=st.integers(min_value=-100, max_value=100),
    )
    @settings(max_examples=50, deadline=None, database=None)
    def test_min_max_equivalence(self, a: int, b: int) -> None:
        """min() and max() match between Molt and CPython."""
        from tests.property.conftest import assert_molt_matches_cpython

        assert_molt_matches_cpython(f"print(min({a}, {b}), max({a}, {b}))")

    @given(n=st.integers(min_value=0, max_value=50))
    @settings(max_examples=50, deadline=None, database=None)
    def test_range_equivalence(self, n: int) -> None:
        """list(range(n)) matches between Molt and CPython."""
        from tests.property.conftest import assert_molt_matches_cpython

        assert_molt_matches_cpython(f"print(list(range({n})))")

    @given(n=st.integers(min_value=-1000, max_value=1000))
    @settings(max_examples=50, deadline=None, database=None)
    def test_bool_equivalence(self, n: int) -> None:
        """bool() matches between Molt and CPython."""
        from tests.property.conftest import assert_molt_matches_cpython

        assert_molt_matches_cpython(f"print(bool({n}))")

    @given(n=st.integers(min_value=-(2**46), max_value=2**46 - 1))
    @settings(max_examples=50, deadline=None, database=None)
    def test_int_str_roundtrip_equivalence(self, n: int) -> None:
        """int(str(n)) roundtrip matches between Molt and CPython."""
        from tests.property.conftest import assert_molt_matches_cpython

        assert_molt_matches_cpython(f"print(int(str({n})))")
