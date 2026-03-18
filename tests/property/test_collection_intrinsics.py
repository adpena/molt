# MOLT_META: area=property-testing
"""Property-based tests for collection operation intrinsics.

Tests algebraic invariants of sorted(), reversed(), dict.keys(),
set union idempotence, dict items roundtrip, and heapq operations.
"""

from __future__ import annotations

import heapq

from hypothesis import given, settings
from hypothesis import strategies as st

# ---------------------------------------------------------------------------
# Strategies
# ---------------------------------------------------------------------------

small_int_lists = st.lists(
    st.integers(min_value=-1000, max_value=1000), max_size=50
)
nonempty_int_lists = st.lists(
    st.integers(min_value=-1000, max_value=1000), min_size=1, max_size=50
)
str_keyed_dicts = st.dictionaries(
    st.text(
        alphabet=st.characters(min_codepoint=32, max_codepoint=126),
        min_size=1,
        max_size=10,
    ),
    st.integers(min_value=-1000, max_value=1000),
    max_size=30,
)
int_sets = st.frozensets(
    st.integers(min_value=-1000, max_value=1000), max_size=50
)

SETTINGS = dict(max_examples=200, deadline=None, database=None)


# ---------------------------------------------------------------------------
# sorted() properties
# ---------------------------------------------------------------------------


class TestSorted:
    """sorted() algebraic properties."""

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_sorted_idempotent(self, lst: list[int]) -> None:
        """sorted(lst) == sorted(sorted(lst)) — sorting is idempotent."""
        assert sorted(lst) == sorted(sorted(lst))

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_sorted_preserves_length(self, lst: list[int]) -> None:
        """len(sorted(lst)) == len(lst)."""
        assert len(sorted(lst)) == len(lst)

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_sorted_preserves_elements(self, lst: list[int]) -> None:
        """sorted(lst) is a permutation of lst (same multiset)."""
        assert sorted(sorted(lst)) == sorted(lst)
        # Also check element counts match
        from collections import Counter

        assert Counter(sorted(lst)) == Counter(lst)

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_sorted_ordered(self, lst: list[int]) -> None:
        """sorted(lst) is non-decreasing."""
        s = sorted(lst)
        for i in range(len(s) - 1):
            assert s[i] <= s[i + 1]


# ---------------------------------------------------------------------------
# dict.keys() length
# ---------------------------------------------------------------------------


class TestDictKeys:
    """dict.keys() length invariants."""

    @given(d=str_keyed_dicts)
    @settings(**SETTINGS)
    def test_keys_len_equals_dict_len(self, d: dict[str, int]) -> None:
        """len(dict.keys()) == len(dict)."""
        assert len(d.keys()) == len(d)

    @given(d=str_keyed_dicts)
    @settings(**SETTINGS)
    def test_values_len_equals_dict_len(self, d: dict[str, int]) -> None:
        """len(dict.values()) == len(dict)."""
        assert len(d.values()) == len(d)

    @given(d=str_keyed_dicts)
    @settings(**SETTINGS)
    def test_items_len_equals_dict_len(self, d: dict[str, int]) -> None:
        """len(dict.items()) == len(dict)."""
        assert len(d.items()) == len(d)


# ---------------------------------------------------------------------------
# set union idempotence
# ---------------------------------------------------------------------------


class TestSetIdempotence:
    """Set union and intersection idempotence."""

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_set_union_idempotent(self, lst: list[int]) -> None:
        """set(lst) | set(lst) == set(lst)."""
        s = set(lst)
        assert s | s == s

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_set_intersection_idempotent(self, lst: list[int]) -> None:
        """set(lst) & set(lst) == set(lst)."""
        s = set(lst)
        assert s & s == s

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_set_from_list_unique(self, lst: list[int]) -> None:
        """len(set(lst)) <= len(lst)."""
        assert len(set(lst)) <= len(lst)

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_set_contains_all_elements(self, lst: list[int]) -> None:
        """Every element of lst is in set(lst)."""
        s = set(lst)
        for x in lst:
            assert x in s


# ---------------------------------------------------------------------------
# reversed() roundtrip
# ---------------------------------------------------------------------------


class TestReversed:
    """list(reversed(list(reversed(xs)))) == xs."""

    @given(xs=small_int_lists)
    @settings(**SETTINGS)
    def test_double_reverse_identity(self, xs: list[int]) -> None:
        """Reversing twice restores the original list."""
        assert list(reversed(list(reversed(xs)))) == xs

    @given(xs=small_int_lists)
    @settings(**SETTINGS)
    def test_reverse_preserves_length(self, xs: list[int]) -> None:
        """len(list(reversed(xs))) == len(xs)."""
        assert len(list(reversed(xs))) == len(xs)

    @given(xs=small_int_lists)
    @settings(**SETTINGS)
    def test_reverse_preserves_elements(self, xs: list[int]) -> None:
        """reversed(xs) is a permutation of xs."""
        from collections import Counter

        assert Counter(list(reversed(xs))) == Counter(xs)


# ---------------------------------------------------------------------------
# dict items roundtrip
# ---------------------------------------------------------------------------


class TestDictItemsRoundtrip:
    """dict(list(d.items())) == d for string-keyed dicts."""

    @given(d=str_keyed_dicts)
    @settings(**SETTINGS)
    def test_items_roundtrip(self, d: dict[str, int]) -> None:
        """dict(list(d.items())) == d."""
        assert dict(list(d.items())) == d

    @given(d=str_keyed_dicts)
    @settings(**SETTINGS)
    def test_items_tuple_roundtrip(self, d: dict[str, int]) -> None:
        """dict(tuple(d.items())) == d."""
        assert dict(tuple(d.items())) == d

    @given(d=str_keyed_dicts)
    @settings(**SETTINGS)
    def test_copy_equals_original(self, d: dict[str, int]) -> None:
        """d.copy() == d and is a distinct object."""
        cp = d.copy()
        assert cp == d
        assert cp is not d


# ---------------------------------------------------------------------------
# heapq invariants
# ---------------------------------------------------------------------------


class TestHeapq:
    """heapq.heappush/heappop maintains heap invariant."""

    @given(xs=nonempty_int_lists)
    @settings(**SETTINGS)
    def test_heappop_returns_minimum(self, xs: list[int]) -> None:
        """heappop from a heapified list returns the minimum element."""
        heap = xs.copy()
        heapq.heapify(heap)
        smallest = heapq.heappop(heap)
        assert smallest == min(xs)

    @given(xs=nonempty_int_lists, val=st.integers(min_value=-1000, max_value=1000))
    @settings(**SETTINGS)
    def test_heappush_maintains_invariant(self, xs: list[int], val: int) -> None:
        """After heappush, the heap invariant holds."""
        heap = xs.copy()
        heapq.heapify(heap)
        heapq.heappush(heap, val)
        # Verify heap invariant: parent <= children
        for i in range(len(heap)):
            left = 2 * i + 1
            right = 2 * i + 2
            if left < len(heap):
                assert heap[i] <= heap[left]
            if right < len(heap):
                assert heap[i] <= heap[right]

    @given(xs=nonempty_int_lists)
    @settings(**SETTINGS)
    def test_heapsort_produces_sorted(self, xs: list[int]) -> None:
        """Repeatedly popping from a heap produces a sorted sequence."""
        heap = xs.copy()
        heapq.heapify(heap)
        result = []
        while heap:
            result.append(heapq.heappop(heap))
        assert result == sorted(xs)

    @given(xs=nonempty_int_lists)
    @settings(**SETTINGS)
    def test_nsmallest_matches_sorted(self, xs: list[int]) -> None:
        """heapq.nsmallest(k, xs) == sorted(xs)[:k]."""
        k = min(len(xs), 5)
        assert heapq.nsmallest(k, xs) == sorted(xs)[:k]

    @given(xs=nonempty_int_lists)
    @settings(**SETTINGS)
    def test_nlargest_matches_sorted(self, xs: list[int]) -> None:
        """heapq.nlargest(k, xs) == sorted(xs, reverse=True)[:k]."""
        k = min(len(xs), 5)
        assert heapq.nlargest(k, xs) == sorted(xs, reverse=True)[:k]
