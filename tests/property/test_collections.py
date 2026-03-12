# MOLT_META: area=property-testing
"""Property-based tests for collection operations.

Tests algebraic invariants for list, dict, set, and tuple — append length,
sort idempotence, key uniqueness, set algebra, and comprehension equivalence.
"""

from __future__ import annotations

from hypothesis import given, settings
from hypothesis import strategies as st

# ---------------------------------------------------------------------------
# Strategies
# ---------------------------------------------------------------------------

small_int_lists = st.lists(st.integers(min_value=-1000, max_value=1000), max_size=50)
small_str_lists = st.lists(
    st.text(
        min_size=0,
        max_size=10,
        alphabet=st.characters(min_codepoint=32, max_codepoint=126),
    ),
    max_size=30,
)
int_sets = st.frozensets(st.integers(min_value=-1000, max_value=1000), max_size=50)
int_dicts = st.dictionaries(
    st.integers(min_value=-100, max_value=100),
    st.integers(min_value=-1000, max_value=1000),
    max_size=30,
)

SETTINGS = dict(max_examples=200, deadline=None, database=None)


# ---------------------------------------------------------------------------
# List properties
# ---------------------------------------------------------------------------


class TestListAppend:
    """List append invariants."""

    @given(lst=small_int_lists, elem=st.integers())
    @settings(**SETTINGS)
    def test_append_increases_len(self, lst: list[int], elem: int) -> None:
        """Appending increases length by exactly 1."""
        original_len = len(lst)
        lst.append(elem)
        assert len(lst) == original_len + 1

    @given(lst=small_int_lists, elem=st.integers())
    @settings(**SETTINGS)
    def test_append_element_is_last(self, lst: list[int], elem: int) -> None:
        """Appended element is the last element."""
        lst.append(elem)
        assert lst[-1] == elem

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_extend_self_doubles_len(self, lst: list[int]) -> None:
        """lst.extend(lst) doubles the length (using a copy)."""
        original = lst.copy()
        original_len = len(lst)
        lst.extend(original)
        assert len(lst) == original_len * 2


class TestListSort:
    """Sorting invariants."""

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_sort_idempotent(self, lst: list[int]) -> None:
        """sorted(sorted(x)) == sorted(x) — sorting is idempotent."""
        once = sorted(lst)
        twice = sorted(once)
        assert once == twice

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_sort_preserves_elements(self, lst: list[int]) -> None:
        """Sorting preserves the multiset of elements."""
        original = lst.copy()
        lst.sort()
        assert sorted(original) == lst
        # Same elements, just reordered
        assert len(lst) == len(original)

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_sort_is_ordered(self, lst: list[int]) -> None:
        """After sorting, each element <= the next."""
        s = sorted(lst)
        for i in range(len(s) - 1):
            assert s[i] <= s[i + 1]

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_reverse_sort(self, lst: list[int]) -> None:
        """sorted(lst, reverse=True) == list(reversed(sorted(lst)))."""
        assert sorted(lst, reverse=True) == list(reversed(sorted(lst)))


class TestListOperations:
    """Miscellaneous list operation properties."""

    @given(a=small_int_lists, b=small_int_lists)
    @settings(**SETTINGS)
    def test_concat_len(self, a: list[int], b: list[int]) -> None:
        """len(a + b) == len(a) + len(b)."""
        assert len(a + b) == len(a) + len(b)

    @given(lst=small_int_lists, n=st.integers(min_value=0, max_value=5))
    @settings(**SETTINGS)
    def test_repeat_len(self, lst: list[int], n: int) -> None:
        """len(lst * n) == len(lst) * n."""
        assert len(lst * n) == len(lst) * n

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_copy_equality(self, lst: list[int]) -> None:
        """lst.copy() == lst and is a distinct object."""
        cp = lst.copy()
        assert cp == lst
        assert cp is not lst


# ---------------------------------------------------------------------------
# Dict properties
# ---------------------------------------------------------------------------


class TestDictProperties:
    """Dictionary invariants."""

    @given(d=int_dicts)
    @settings(**SETTINGS)
    def test_key_uniqueness(self, d: dict[int, int]) -> None:
        """dict keys are unique — len(d) == len(set(d.keys()))."""
        assert len(d) == len(set(d.keys()))

    @given(d=int_dicts, k=st.integers(min_value=-100, max_value=100), v=st.integers())
    @settings(**SETTINGS)
    def test_setitem_getitem_roundtrip(self, d: dict[int, int], k: int, v: int) -> None:
        """d[k] = v then d[k] == v."""
        d[k] = v
        assert d[k] == v

    @given(d=int_dicts)
    @settings(**SETTINGS)
    def test_keys_values_items_len(self, d: dict[int, int]) -> None:
        """len(keys) == len(values) == len(items) == len(d)."""
        assert len(d.keys()) == len(d)
        assert len(d.values()) == len(d)
        assert len(d.items()) == len(d)

    @given(d=int_dicts)
    @settings(**SETTINGS)
    def test_dict_from_items_roundtrip(self, d: dict[int, int]) -> None:
        """dict(d.items()) == d."""
        assert dict(d.items()) == d

    @given(a=int_dicts, b=int_dicts)
    @settings(**SETTINGS)
    def test_update_superset_keys(self, a: dict[int, int], b: dict[int, int]) -> None:
        """After a.update(b), a contains all keys from both."""
        original_keys = set(a.keys())
        a.update(b)
        assert set(a.keys()) == original_keys | set(b.keys())


# ---------------------------------------------------------------------------
# Set properties
# ---------------------------------------------------------------------------


class TestSetAlgebra:
    """Set operation algebraic laws."""

    @given(a=int_sets, b=int_sets)
    @settings(**SETTINGS)
    def test_union_commutative(self, a: frozenset[int], b: frozenset[int]) -> None:
        """A | B == B | A."""
        assert a | b == b | a

    @given(a=int_sets, b=int_sets)
    @settings(**SETTINGS)
    def test_intersection_commutative(
        self, a: frozenset[int], b: frozenset[int]
    ) -> None:
        """A & B == B & A."""
        assert a & b == b & a

    @given(a=int_sets, b=int_sets)
    @settings(**SETTINGS)
    def test_union_superset(self, a: frozenset[int], b: frozenset[int]) -> None:
        """A | B is a superset of both A and B."""
        union = a | b
        assert a <= union
        assert b <= union

    @given(a=int_sets, b=int_sets)
    @settings(**SETTINGS)
    def test_intersection_subset(self, a: frozenset[int], b: frozenset[int]) -> None:
        """A & B is a subset of both A and B."""
        inter = a & b
        assert inter <= a
        assert inter <= b

    @given(a=int_sets, b=int_sets)
    @settings(**SETTINGS)
    def test_difference_disjoint_from_subtracted(
        self, a: frozenset[int], b: frozenset[int]
    ) -> None:
        """(A - B) & B == empty set."""
        diff = a - b
        assert len(diff & b) == 0

    @given(a=int_sets, b=int_sets)
    @settings(**SETTINGS)
    def test_symmetric_difference(self, a: frozenset[int], b: frozenset[int]) -> None:
        """A ^ B == (A | B) - (A & B)."""
        assert a ^ b == (a | b) - (a & b)

    @given(a=int_sets)
    @settings(**SETTINGS)
    def test_union_with_self(self, a: frozenset[int]) -> None:
        """A | A == A."""
        assert a | a == a

    @given(a=int_sets)
    @settings(**SETTINGS)
    def test_intersection_with_self(self, a: frozenset[int]) -> None:
        """A & A == A."""
        assert a & a == a

    @given(a=int_sets, b=int_sets, c=int_sets)
    @settings(**SETTINGS)
    def test_union_associative(
        self, a: frozenset[int], b: frozenset[int], c: frozenset[int]
    ) -> None:
        """(A | B) | C == A | (B | C)."""
        assert (a | b) | c == a | (b | c)

    @given(a=int_sets, b=int_sets, c=int_sets)
    @settings(**SETTINGS)
    def test_intersection_associative(
        self, a: frozenset[int], b: frozenset[int], c: frozenset[int]
    ) -> None:
        """(A & B) & C == A & (B & C)."""
        assert (a & b) & c == a & (b & c)


# ---------------------------------------------------------------------------
# Tuple properties
# ---------------------------------------------------------------------------


class TestTupleProperties:
    """Tuple immutability and operation invariants."""

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_tuple_preserves_elements(self, lst: list[int]) -> None:
        """tuple(lst) preserves all elements and order."""
        t = tuple(lst)
        assert len(t) == len(lst)
        assert list(t) == lst

    @given(
        a=st.tuples(st.integers(), st.integers()),
        b=st.tuples(st.integers(), st.integers()),
    )
    @settings(**SETTINGS)
    def test_tuple_concat_len(self, a: tuple[int, int], b: tuple[int, int]) -> None:
        """len(a + b) == len(a) + len(b)."""
        assert len(a + b) == len(a) + len(b)

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_tuple_hashable(self, lst: list[int]) -> None:
        """Tuples of ints are hashable (can be used as dict keys)."""
        t = tuple(lst)
        d = {t: 1}
        assert d[t] == 1


# ---------------------------------------------------------------------------
# List comprehension equivalence
# ---------------------------------------------------------------------------


class TestComprehensions:
    """List comprehension equivalence with explicit loops."""

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_map_comprehension(self, lst: list[int]) -> None:
        """[x*2 for x in lst] == list(map(lambda x: x*2, lst))."""
        comp = [x * 2 for x in lst]
        mapped = list(map(lambda x: x * 2, lst))
        assert comp == mapped

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_filter_comprehension(self, lst: list[int]) -> None:
        """[x for x in lst if x > 0] == list(filter(lambda x: x > 0, lst))."""
        comp = [x for x in lst if x > 0]
        filtered = list(filter(lambda x: x > 0, lst))
        assert comp == filtered

    @given(lst=small_int_lists)
    @settings(**SETTINGS)
    def test_comprehension_preserves_len_no_filter(self, lst: list[int]) -> None:
        """Comprehension without filter preserves length."""
        comp = [x + 1 for x in lst]
        assert len(comp) == len(lst)
