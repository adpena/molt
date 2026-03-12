# MOLT_META: area=property-testing
"""Property-based tests for Python builtin functions.

Tests range(), len(), bool(), int()/float()/str() conversions,
abs(), min(), max(), zip(), and enumerate() properties.
"""

from __future__ import annotations

import math

from hypothesis import given, settings
from hypothesis import strategies as st

# ---------------------------------------------------------------------------
# Strategies
# ---------------------------------------------------------------------------

small_ints = st.integers(min_value=-10_000, max_value=10_000)
small_positive = st.integers(min_value=0, max_value=500)
small_int_lists = st.lists(
    st.integers(min_value=-1000, max_value=1000), min_size=1, max_size=50
)
nonempty_int_lists = st.lists(
    st.integers(min_value=-1000, max_value=1000), min_size=1, max_size=50
)

SETTINGS = dict(max_examples=200, deadline=None, database=None)


# ---------------------------------------------------------------------------
# range() properties
# ---------------------------------------------------------------------------


class TestRange:
    """range() invariants."""

    @given(n=st.integers(min_value=0, max_value=500))
    @settings(**SETTINGS)
    def test_range_length(self, n: int) -> None:
        """len(range(n)) == n for non-negative n."""
        assert len(range(n)) == n

    @given(n=st.integers(min_value=0, max_value=500))
    @settings(**SETTINGS)
    def test_range_list_length(self, n: int) -> None:
        """list(range(n)) has length n."""
        assert len(list(range(n))) == n

    @given(start=small_ints, stop=small_ints)
    @settings(**SETTINGS)
    def test_range_len_formula(self, start: int, stop: int) -> None:
        """len(range(start, stop)) == max(0, stop - start)."""
        assert len(range(start, stop)) == max(0, stop - start)

    @given(
        start=small_ints, stop=small_ints, step=st.integers(min_value=1, max_value=100)
    )
    @settings(**SETTINGS)
    def test_range_step_len(self, start: int, stop: int, step: int) -> None:
        """len(range(start, stop, step)) matches the ceiling division formula."""
        expected = max(0, math.ceil((stop - start) / step))
        assert len(range(start, stop, step)) == expected

    @given(n=st.integers(min_value=1, max_value=200))
    @settings(**SETTINGS)
    def test_range_contains_all_elements(self, n: int) -> None:
        """Every element in list(range(n)) is in range(n)."""
        r = range(n)
        for x in list(r):
            assert x in r


# ---------------------------------------------------------------------------
# len() properties
# ---------------------------------------------------------------------------


class TestLen:
    """len() invariants across types."""

    @given(lst=st.lists(st.integers(), max_size=50))
    @settings(**SETTINGS)
    def test_len_list_non_negative(self, lst: list[int]) -> None:
        """len(list) >= 0."""
        assert len(lst) >= 0

    @given(s=st.text(max_size=100))
    @settings(**SETTINGS)
    def test_len_str_non_negative(self, s: str) -> None:
        """len(str) >= 0."""
        assert len(s) >= 0

    @given(
        d=st.dictionaries(
            st.integers(min_value=-50, max_value=50), st.integers(), max_size=30
        )
    )
    @settings(**SETTINGS)
    def test_len_dict(self, d: dict[int, int]) -> None:
        """len(dict) == number of unique keys."""
        assert len(d) == len(set(d.keys()))

    @given(t=st.tuples(st.integers(), st.integers(), st.integers()))
    @settings(**SETTINGS)
    def test_len_tuple(self, t: tuple[int, int, int]) -> None:
        """len((a, b, c)) == 3."""
        assert len(t) == 3


# ---------------------------------------------------------------------------
# bool() truthiness
# ---------------------------------------------------------------------------


class TestBool:
    """bool() truthiness invariants matching CPython semantics."""

    @given(n=st.integers())
    @settings(**SETTINGS)
    def test_int_truthiness(self, n: int) -> None:
        """bool(0) is False, bool(non-zero) is True."""
        if n == 0:
            assert bool(n) is False
        else:
            assert bool(n) is True

    @given(f=st.floats(allow_nan=False))
    @settings(**SETTINGS)
    def test_float_truthiness(self, f: float) -> None:
        """bool(0.0) is False, bool(non-zero) is True."""
        if f == 0.0:
            assert bool(f) is False
        else:
            assert bool(f) is True

    @given(s=st.text(max_size=50))
    @settings(**SETTINGS)
    def test_str_truthiness(self, s: str) -> None:
        """bool('') is False, bool(non-empty) is True."""
        if s == "":
            assert bool(s) is False
        else:
            assert bool(s) is True

    @given(lst=st.lists(st.integers(), max_size=10))
    @settings(**SETTINGS)
    def test_list_truthiness(self, lst: list[int]) -> None:
        """bool([]) is False, bool(non-empty) is True."""
        if len(lst) == 0:
            assert bool(lst) is False
        else:
            assert bool(lst) is True


# ---------------------------------------------------------------------------
# Type conversion properties
# ---------------------------------------------------------------------------


class TestConversions:
    """int(), float(), str() conversion invariants."""

    @given(n=st.integers(min_value=-(10**15), max_value=10**15))
    @settings(**SETTINGS)
    def test_int_str_roundtrip(self, n: int) -> None:
        """int(str(n)) == n for all integers."""
        assert int(str(n)) == n

    @given(n=st.integers(min_value=-(2**46), max_value=2**46 - 1))
    @settings(**SETTINGS)
    def test_float_int_roundtrip_small(self, n: int) -> None:
        """int(float(n)) == n for integers in float exact range."""
        assert int(float(n)) == n

    @given(
        f=st.floats(
            allow_nan=False, allow_infinity=False, min_value=-1e10, max_value=1e10
        )
    )
    @settings(**SETTINGS)
    def test_str_float_roundtrip(self, f: float) -> None:
        """float(str(f)) == f for finite floats (via repr roundtrip)."""
        assert float(repr(f)) == f

    @given(n=st.integers())
    @settings(**SETTINGS)
    def test_int_is_identity(self, n: int) -> None:
        """int(n) == n (identity for integers)."""
        assert int(n) == n

    @given(s=st.text(max_size=50))
    @settings(**SETTINGS)
    def test_str_is_identity(self, s: str) -> None:
        """str(s) == s (identity for strings)."""
        assert str(s) == s


# ---------------------------------------------------------------------------
# abs(), min(), max()
# ---------------------------------------------------------------------------


class TestAbsMinMax:
    """Absolute value, minimum, and maximum properties."""

    @given(n=st.integers())
    @settings(**SETTINGS)
    def test_abs_non_negative(self, n: int) -> None:
        """abs(n) >= 0 for all integers."""
        assert abs(n) >= 0

    @given(n=st.integers())
    @settings(**SETTINGS)
    def test_abs_idempotent(self, n: int) -> None:
        """abs(abs(n)) == abs(n)."""
        assert abs(abs(n)) == abs(n)

    @given(n=st.integers())
    @settings(**SETTINGS)
    def test_abs_negate(self, n: int) -> None:
        """abs(-n) == abs(n)."""
        assert abs(-n) == abs(n)

    @given(lst=nonempty_int_lists)
    @settings(**SETTINGS)
    def test_min_in_list(self, lst: list[int]) -> None:
        """min(lst) is an element of lst."""
        assert min(lst) in lst

    @given(lst=nonempty_int_lists)
    @settings(**SETTINGS)
    def test_max_in_list(self, lst: list[int]) -> None:
        """max(lst) is an element of lst."""
        assert max(lst) in lst

    @given(lst=nonempty_int_lists)
    @settings(**SETTINGS)
    def test_min_le_max(self, lst: list[int]) -> None:
        """min(lst) <= max(lst)."""
        assert min(lst) <= max(lst)

    @given(lst=nonempty_int_lists)
    @settings(**SETTINGS)
    def test_min_le_all(self, lst: list[int]) -> None:
        """min(lst) <= x for all x in lst."""
        m = min(lst)
        for x in lst:
            assert m <= x

    @given(lst=nonempty_int_lists)
    @settings(**SETTINGS)
    def test_max_ge_all(self, lst: list[int]) -> None:
        """max(lst) >= x for all x in lst."""
        m = max(lst)
        for x in lst:
            assert m >= x

    @given(f=st.floats(allow_nan=False, allow_infinity=False))
    @settings(**SETTINGS)
    def test_abs_float_non_negative(self, f: float) -> None:
        """abs(f) >= 0.0 for all finite floats."""
        assert abs(f) >= 0.0


# ---------------------------------------------------------------------------
# zip() and enumerate()
# ---------------------------------------------------------------------------


class TestZipEnumerate:
    """zip() and enumerate() properties."""

    @given(
        a=st.lists(st.integers(), max_size=30), b=st.lists(st.integers(), max_size=30)
    )
    @settings(**SETTINGS)
    def test_zip_length(self, a: list[int], b: list[int]) -> None:
        """len(list(zip(a, b))) == min(len(a), len(b))."""
        assert len(list(zip(a, b))) == min(len(a), len(b))

    @given(
        a=st.lists(st.integers(), max_size=30), b=st.lists(st.integers(), max_size=30)
    )
    @settings(**SETTINGS)
    def test_zip_unzip(self, a: list[int], b: list[int]) -> None:
        """Zipping then unzipping recovers truncated inputs."""
        zipped = list(zip(a, b))
        if zipped:
            aa, bb = zip(*zipped)
            n = min(len(a), len(b))
            assert list(aa) == a[:n]
            assert list(bb) == b[:n]

    @given(lst=st.lists(st.integers(), max_size=50))
    @settings(**SETTINGS)
    def test_enumerate_indices(self, lst: list[int]) -> None:
        """enumerate produces sequential indices starting at 0."""
        for i, val in enumerate(lst):
            assert lst[i] == val

    @given(
        lst=st.lists(st.integers(), max_size=50),
        start=st.integers(min_value=-100, max_value=100),
    )
    @settings(**SETTINGS)
    def test_enumerate_start(self, lst: list[int], start: int) -> None:
        """enumerate(lst, start) first index is start."""
        items = list(enumerate(lst, start))
        if items:
            assert items[0][0] == start
            assert items[-1][0] == start + len(lst) - 1

    @given(lst=st.lists(st.integers(), max_size=50))
    @settings(**SETTINGS)
    def test_enumerate_length(self, lst: list[int]) -> None:
        """len(list(enumerate(lst))) == len(lst)."""
        assert len(list(enumerate(lst))) == len(lst)
