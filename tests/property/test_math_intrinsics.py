# MOLT_META: area=property-testing
"""Property-based tests for math module intrinsics.

Tests algebraic invariants of math.floor, math.ceil, math.sqrt,
math.isnan, math.isinf, math.gcd, and math.factorial.
"""

from __future__ import annotations

import math

from hypothesis import assume, given, settings
from hypothesis import strategies as st

# ---------------------------------------------------------------------------
# Strategies
# ---------------------------------------------------------------------------

safe_floats = st.floats(allow_nan=False, allow_infinity=False)
non_negative_floats = st.floats(min_value=0.0, allow_nan=False, allow_infinity=False)
all_floats = st.floats()
small_ints = st.integers(min_value=-10_000, max_value=10_000)
positive_ints = st.integers(min_value=1, max_value=10_000)

SETTINGS = dict(max_examples=200, deadline=None, database=None)


# ---------------------------------------------------------------------------
# math.floor / math.ceil
# ---------------------------------------------------------------------------


class TestFloorCeil:
    """math.floor() and math.ceil() invariants."""

    @given(x=safe_floats)
    @settings(**SETTINGS)
    def test_floor_le_x(self, x: float) -> None:
        """math.floor(x) <= x for all finite floats."""
        assert math.floor(x) <= x

    @given(x=safe_floats)
    @settings(**SETTINGS)
    def test_ceil_ge_x(self, x: float) -> None:
        """math.ceil(x) >= x for all finite floats."""
        assert math.ceil(x) >= x

    @given(x=safe_floats)
    @settings(**SETTINGS)
    def test_floor_ceil_bound(self, x: float) -> None:
        """math.ceil(x) - math.floor(x) is 0 or 1."""
        diff = math.ceil(x) - math.floor(x)
        assert diff in (0, 1)

    @given(n=small_ints)
    @settings(**SETTINGS)
    def test_floor_identity_on_int(self, n: int) -> None:
        """math.floor(n) == n for all integers."""
        assert math.floor(n) == n

    @given(n=small_ints)
    @settings(**SETTINGS)
    def test_ceil_identity_on_int(self, n: int) -> None:
        """math.ceil(n) == n for all integers."""
        assert math.ceil(n) == n

    @given(x=safe_floats)
    @settings(**SETTINGS)
    def test_floor_is_int(self, x: float) -> None:
        """math.floor returns an integer type."""
        assert isinstance(math.floor(x), int)

    @given(x=safe_floats)
    @settings(**SETTINGS)
    def test_ceil_is_int(self, x: float) -> None:
        """math.ceil returns an integer type."""
        assert isinstance(math.ceil(x), int)


# ---------------------------------------------------------------------------
# math.sqrt
# ---------------------------------------------------------------------------


class TestSqrt:
    """math.sqrt() properties for non-negative inputs."""

    @given(x=non_negative_floats)
    @settings(**SETTINGS)
    def test_sqrt_non_negative(self, x: float) -> None:
        """math.sqrt(x) >= 0 for x >= 0."""
        assert math.sqrt(x) >= 0.0

    @given(x=non_negative_floats)
    @settings(**SETTINGS)
    def test_sqrt_squared_approx(self, x: float) -> None:
        """math.sqrt(x)**2 is approximately x for finite positive floats."""
        assume(x > 0.0)
        assume(x < 1e150)  # Avoid overflow when squaring sqrt
        result = math.sqrt(x) ** 2
        assert math.isclose(result, x, rel_tol=1e-9, abs_tol=1e-15)

    @given(x=non_negative_floats)
    @settings(**SETTINGS)
    def test_sqrt_monotonic(self, x: float) -> None:
        """math.sqrt is monotonically increasing: sqrt(x) <= sqrt(x + 1)."""
        assume(x < 1e300)  # Avoid infinity on x + 1
        assert math.sqrt(x) <= math.sqrt(x + 1)

    @given(n=st.integers(min_value=0, max_value=100))
    @settings(**SETTINGS)
    def test_sqrt_perfect_squares(self, n: int) -> None:
        """math.sqrt(n*n) == n for small non-negative integers."""
        assert math.sqrt(n * n) == float(n)


# ---------------------------------------------------------------------------
# math.isnan / math.isinf
# ---------------------------------------------------------------------------


class TestNanInf:
    """math.isnan() and math.isinf() consistency."""

    @given(x=safe_floats)
    @settings(**SETTINGS)
    def test_finite_not_nan(self, x: float) -> None:
        """Finite floats are not NaN."""
        assert not math.isnan(x)

    @given(x=safe_floats)
    @settings(**SETTINGS)
    def test_finite_not_inf(self, x: float) -> None:
        """Finite floats are not infinite."""
        assert not math.isinf(x)

    @given(x=all_floats)
    @settings(**SETTINGS)
    def test_isfinite_iff_not_nan_not_inf(self, x: float) -> None:
        """math.isfinite(x) iff not isnan(x) and not isinf(x)."""
        assert math.isfinite(x) == (not math.isnan(x) and not math.isinf(x))

    @given(x=all_floats)
    @settings(**SETTINGS)
    def test_nan_not_equal_to_self(self, x: float) -> None:
        """NaN is the only float not equal to itself."""
        if math.isnan(x):
            assert x != x  # noqa: PLR0124
        else:
            assert x == x  # noqa: PLR0124

    @given(x=all_floats)
    @settings(**SETTINGS)
    def test_isnan_and_isinf_mutually_exclusive(self, x: float) -> None:
        """A float cannot be both NaN and infinite."""
        assert not (math.isnan(x) and math.isinf(x))


# ---------------------------------------------------------------------------
# math.gcd
# ---------------------------------------------------------------------------


class TestGcd:
    """math.gcd() properties."""

    @given(a=small_ints, b=small_ints)
    @settings(**SETTINGS)
    def test_gcd_divides_both(self, a: int, b: int) -> None:
        """gcd(a, b) divides both a and b."""
        g = math.gcd(a, b)
        if g != 0:
            assert a % g == 0
            assert b % g == 0

    @given(a=small_ints, b=small_ints)
    @settings(**SETTINGS)
    def test_gcd_non_negative(self, a: int, b: int) -> None:
        """gcd(a, b) >= 0 for all integers."""
        assert math.gcd(a, b) >= 0

    @given(a=small_ints, b=small_ints)
    @settings(**SETTINGS)
    def test_gcd_commutative(self, a: int, b: int) -> None:
        """gcd(a, b) == gcd(b, a)."""
        assert math.gcd(a, b) == math.gcd(b, a)

    @given(a=small_ints)
    @settings(**SETTINGS)
    def test_gcd_with_zero(self, a: int) -> None:
        """gcd(a, 0) == abs(a)."""
        assert math.gcd(a, 0) == abs(a)

    @given(a=positive_ints, b=positive_ints)
    @settings(**SETTINGS)
    def test_gcd_le_min(self, a: int, b: int) -> None:
        """gcd(a, b) <= min(a, b) for positive a, b."""
        assert math.gcd(a, b) <= min(a, b)

    @given(a=small_ints, b=small_ints)
    @settings(**SETTINGS)
    def test_gcd_abs_invariant(self, a: int, b: int) -> None:
        """gcd(a, b) == gcd(abs(a), abs(b))."""
        assert math.gcd(a, b) == math.gcd(abs(a), abs(b))


# ---------------------------------------------------------------------------
# math.factorial
# ---------------------------------------------------------------------------


class TestFactorial:
    """math.factorial() properties for small n."""

    @given(n=st.integers(min_value=0, max_value=20))
    @settings(**SETTINGS)
    def test_factorial_positive(self, n: int) -> None:
        """factorial(n) > 0 for n >= 0."""
        assert math.factorial(n) > 0

    @given(n=st.integers(min_value=1, max_value=20))
    @settings(**SETTINGS)
    def test_factorial_recurrence(self, n: int) -> None:
        """factorial(n) == n * factorial(n - 1)."""
        assert math.factorial(n) == n * math.factorial(n - 1)

    @given(n=st.integers(min_value=0, max_value=20))
    @settings(**SETTINGS)
    def test_factorial_monotonic(self, n: int) -> None:
        """factorial is monotonically non-decreasing."""
        if n > 0:
            assert math.factorial(n) >= math.factorial(n - 1)

    @given(n=st.integers(min_value=2, max_value=20))
    @settings(**SETTINGS)
    def test_factorial_divisible_by_all_below(self, n: int) -> None:
        """factorial(n) is divisible by all integers from 1 to n."""
        f = math.factorial(n)
        for k in range(1, n + 1):
            assert f % k == 0

    @given(n=st.integers(min_value=0, max_value=0))
    @settings(**SETTINGS)
    def test_factorial_base_case(self, n: int) -> None:
        """factorial(0) == 1."""
        assert math.factorial(0) == 1
