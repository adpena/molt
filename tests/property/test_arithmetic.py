# MOLT_META: area=property-testing
"""Property-based tests for arithmetic operations.

Tests mathematical invariants (commutativity, associativity, identity,
distributivity) for int, float, and mixed arithmetic, plus edge cases
around bigint transitions, division by zero, modulo, and power.
"""

from __future__ import annotations

import math

from hypothesis import given, settings, assume
from hypothesis import strategies as st

# ---------------------------------------------------------------------------
# Strategies
# ---------------------------------------------------------------------------

# Inline ints that fit in Molt's NaN-boxed representation (47-bit payload).
inline_ints = st.integers(min_value=-(2**46), max_value=2**46 - 1)

# Ints that straddle the inline/bigint boundary.
boundary_ints = st.integers(min_value=2**45, max_value=2**48)

# Small ints for associativity tests (avoid overflow-driven slowness).
small_ints = st.integers(min_value=-10_000, max_value=10_000)

# Reasonable floats — avoid inf/nan which break algebraic laws.
reasonable_floats = st.floats(
    min_value=-1e15, max_value=1e15, allow_nan=False, allow_infinity=False
)

SETTINGS = dict(max_examples=200, deadline=None, database=None)


# ---------------------------------------------------------------------------
# Integer arithmetic properties
# ---------------------------------------------------------------------------


class TestIntCommutative:
    """Addition and multiplication are commutative over integers."""

    @given(a=inline_ints, b=inline_ints)
    @settings(**SETTINGS)
    def test_add_commutative(self, a: int, b: int) -> None:
        """a + b == b + a for all integers."""
        assert a + b == b + a

    @given(a=inline_ints, b=inline_ints)
    @settings(**SETTINGS)
    def test_mul_commutative(self, a: int, b: int) -> None:
        """a * b == b * a for all integers."""
        assert a * b == b * a


class TestIntAssociative:
    """Addition and multiplication are associative over integers."""

    @given(a=small_ints, b=small_ints, c=small_ints)
    @settings(**SETTINGS)
    def test_add_associative(self, a: int, b: int, c: int) -> None:
        """(a + b) + c == a + (b + c) for all integers."""
        assert (a + b) + c == a + (b + c)

    @given(a=small_ints, b=small_ints, c=small_ints)
    @settings(**SETTINGS)
    def test_mul_associative(self, a: int, b: int, c: int) -> None:
        """(a * b) * c == a * (b * c) for all integers."""
        assert (a * b) * c == a * (b * c)


class TestIntIdentity:
    """Additive and multiplicative identity elements."""

    @given(a=inline_ints)
    @settings(**SETTINGS)
    def test_add_identity(self, a: int) -> None:
        """a + 0 == a for all integers."""
        assert a + 0 == a

    @given(a=inline_ints)
    @settings(**SETTINGS)
    def test_mul_identity(self, a: int) -> None:
        """a * 1 == a for all integers."""
        assert a * 1 == a

    @given(a=inline_ints)
    @settings(**SETTINGS)
    def test_mul_zero(self, a: int) -> None:
        """a * 0 == 0 for all integers."""
        assert a * 0 == 0


class TestIntDistributive:
    """Multiplication distributes over addition."""

    @given(a=small_ints, b=small_ints, c=small_ints)
    @settings(**SETTINGS)
    def test_distributive(self, a: int, b: int, c: int) -> None:
        """a * (b + c) == a * b + a * c for all integers."""
        assert a * (b + c) == a * b + a * c


class TestIntBigintTransition:
    """Values crossing the inline/bigint boundary preserve correctness."""

    @given(a=boundary_ints, b=boundary_ints)
    @settings(**SETTINGS)
    def test_add_bigint_boundary(self, a: int, b: int) -> None:
        """Addition near bigint boundary is correct."""
        result = a + b
        assert result == a + b  # deterministic
        assert result - b == a  # invertible

    @given(a=boundary_ints)
    @settings(**SETTINGS)
    def test_negate_bigint_boundary(self, a: int) -> None:
        """Negation near bigint boundary round-trips."""
        assert -(-a) == a

    @given(a=boundary_ints, b=st.integers(min_value=1, max_value=1000))
    @settings(**SETTINGS)
    def test_mul_bigint_boundary(self, a: int, b: int) -> None:
        """Multiplication producing bigint results is correct."""
        product = a * b
        assert product // b == a


# ---------------------------------------------------------------------------
# Float arithmetic properties
# ---------------------------------------------------------------------------


class TestFloatArithmetic:
    """Basic float arithmetic properties (within tolerance)."""

    @given(a=reasonable_floats)
    @settings(**SETTINGS)
    def test_add_identity(self, a: float) -> None:
        """a + 0.0 == a for all finite floats."""
        assert a + 0.0 == a

    @given(a=reasonable_floats)
    @settings(**SETTINGS)
    def test_mul_identity(self, a: float) -> None:
        """a * 1.0 == a for all finite floats."""
        assert a * 1.0 == a

    @given(a=reasonable_floats)
    @settings(**SETTINGS)
    def test_negate_involution(self, a: float) -> None:
        """-(-a) == a for all finite floats."""
        assert -(-a) == a

    @given(a=reasonable_floats, b=reasonable_floats)
    @settings(**SETTINGS)
    def test_add_commutative(self, a: float, b: float) -> None:
        """a + b == b + a for all finite floats."""
        assert a + b == b + a

    @given(a=reasonable_floats, b=reasonable_floats)
    @settings(**SETTINGS)
    def test_mul_commutative(self, a: float, b: float) -> None:
        """a * b == b * a for all finite floats."""
        assert a * b == b * a


# ---------------------------------------------------------------------------
# Mixed int/float promotion
# ---------------------------------------------------------------------------


class TestMixedPromotion:
    """int/float mixed arithmetic promotes correctly."""

    @given(a=inline_ints, b=reasonable_floats)
    @settings(**SETTINGS)
    def test_int_plus_float_is_float(self, a: int, b: float) -> None:
        """int + float produces a float."""
        result = a + b
        assert isinstance(result, float)

    @given(a=inline_ints, b=reasonable_floats)
    @settings(**SETTINGS)
    def test_int_mul_float_is_float(self, a: int, b: float) -> None:
        """int * float produces a float."""
        result = a * b
        assert isinstance(result, float)

    @given(a=inline_ints)
    @settings(**SETTINGS)
    def test_int_to_float_exact(self, a: int) -> None:
        """float(int) preserves value for inline-range ints."""
        assert float(a) == a


# ---------------------------------------------------------------------------
# Division edge cases
# ---------------------------------------------------------------------------


class TestDivision:
    """Division properties and error semantics."""

    @given(a=inline_ints)
    @settings(**SETTINGS)
    def test_div_by_zero_raises(self, a: int) -> None:
        """Division by zero raises ZeroDivisionError."""
        try:
            _ = a / 0
            assert False, "Should have raised ZeroDivisionError"
        except ZeroDivisionError:
            pass

    @given(a=inline_ints)
    @settings(**SETTINGS)
    def test_floordiv_by_zero_raises(self, a: int) -> None:
        """Floor division by zero raises ZeroDivisionError."""
        try:
            _ = a // 0
            assert False, "Should have raised ZeroDivisionError"
        except ZeroDivisionError:
            pass

    @given(a=inline_ints, b=inline_ints)
    @settings(**SETTINGS)
    def test_divmod_identity(self, a: int, b: int) -> None:
        """divmod(a, b) == (a // b, a % b) for non-zero b."""
        assume(b != 0)
        q, r = divmod(a, b)
        assert q == a // b
        assert r == a % b
        assert a == q * b + r

    @given(a=inline_ints, b=inline_ints)
    @settings(**SETTINGS)
    def test_true_div_returns_float(self, a: int, b: int) -> None:
        """True division of ints produces a float."""
        assume(b != 0)
        result = a / b
        assert isinstance(result, float)


# ---------------------------------------------------------------------------
# Modulo properties
# ---------------------------------------------------------------------------


class TestModulo:
    """Modulo operator properties."""

    @given(a=inline_ints, b=inline_ints)
    @settings(**SETTINGS)
    def test_modulo_range(self, a: int, b: int) -> None:
        """a % b has the same sign as b (Python semantics)."""
        assume(b != 0)
        r = a % b
        if b > 0:
            assert 0 <= r < b
        else:
            assert b < r <= 0

    @given(a=inline_ints, b=inline_ints)
    @settings(**SETTINGS)
    def test_modulo_reconstruction(self, a: int, b: int) -> None:
        """a == (a // b) * b + (a % b) for non-zero b."""
        assume(b != 0)
        assert a == (a // b) * b + (a % b)


# ---------------------------------------------------------------------------
# Power operation
# ---------------------------------------------------------------------------


class TestPower:
    """Exponentiation properties."""

    @given(
        base=st.integers(min_value=-10, max_value=10),
        exp=st.integers(min_value=0, max_value=20),
    )
    @settings(**SETTINGS)
    def test_power_zero_exponent(self, base: int, exp: int) -> None:
        """x**0 == 1 for any non-zero x (and 0**0 == 1 in Python)."""
        if exp == 0:
            assert base**exp == 1

    @given(
        base=st.integers(min_value=-10, max_value=10),
        exp=st.integers(min_value=0, max_value=20),
    )
    @settings(**SETTINGS)
    def test_power_one_exponent(self, base: int, exp: int) -> None:
        """x**1 == x for all x."""
        assert base**1 == base

    @given(
        base=st.integers(min_value=2, max_value=10),
        a=st.integers(min_value=0, max_value=10),
        b=st.integers(min_value=0, max_value=10),
    )
    @settings(**SETTINGS)
    def test_power_addition_law(self, base: int, a: int, b: int) -> None:
        """x**(a+b) == x**a * x**b for positive base."""
        assert base ** (a + b) == base**a * base**b

    @given(base=st.integers(min_value=-100, max_value=100))
    @settings(**SETTINGS)
    def test_negative_exponent_raises_or_float(self, base: int) -> None:
        """Negative exponent on int produces float (or ZeroDivisionError for 0)."""
        if base == 0:
            try:
                _ = base**-1
                assert False, "0**-1 should raise"
            except ZeroDivisionError:
                pass
        else:
            result = base**-1
            assert isinstance(result, float)
            assert math.isclose(result, 1.0 / base, rel_tol=1e-9)
