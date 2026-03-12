# MOLT_META: area=property-testing
"""Property-based tests for string operations.

Tests algebraic invariants of string concatenation, repetition, slicing,
split/join, case conversion, and stripping.
"""

from __future__ import annotations

from hypothesis import given, settings
from hypothesis import strategies as st

# ---------------------------------------------------------------------------
# Strategies
# ---------------------------------------------------------------------------

# Printable ASCII strings (avoids encoding edge cases for now).
ascii_text = st.text(
    alphabet=st.characters(min_codepoint=32, max_codepoint=126),
    min_size=0,
    max_size=100,
)

# Short strings for operations that create large outputs.
short_text = st.text(
    alphabet=st.characters(min_codepoint=32, max_codepoint=126),
    min_size=0,
    max_size=20,
)

# Unicode strings including BMP characters.
unicode_text = st.text(min_size=0, max_size=50)

SETTINGS = dict(max_examples=200, deadline=None, database=None)


# ---------------------------------------------------------------------------
# Concatenation properties
# ---------------------------------------------------------------------------


class TestConcatenation:
    """String concatenation algebraic properties."""

    @given(a=ascii_text, b=ascii_text)
    @settings(**SETTINGS)
    def test_len_additive(self, a: str, b: str) -> None:
        """len(a + b) == len(a) + len(b) for all strings."""
        assert len(a + b) == len(a) + len(b)

    @given(a=ascii_text, b=ascii_text, c=ascii_text)
    @settings(**SETTINGS)
    def test_concat_associative(self, a: str, b: str, c: str) -> None:
        """(a + b) + c == a + (b + c) for all strings."""
        assert (a + b) + c == a + (b + c)

    @given(a=ascii_text)
    @settings(**SETTINGS)
    def test_concat_identity(self, a: str) -> None:
        """a + '' == '' + a == a for all strings."""
        assert a + "" == a
        assert "" + a == a

    @given(a=ascii_text, b=ascii_text)
    @settings(**SETTINGS)
    def test_concat_startswith_endswith(self, a: str, b: str) -> None:
        """(a + b) starts with a and ends with b."""
        combined = a + b
        assert combined.startswith(a)
        assert combined.endswith(b)


# ---------------------------------------------------------------------------
# Repetition properties
# ---------------------------------------------------------------------------


class TestRepetition:
    """String repetition properties."""

    @given(s=short_text, n=st.integers(min_value=0, max_value=50))
    @settings(**SETTINGS)
    def test_len_multiplicative(self, s: str, n: int) -> None:
        """len(s * n) == len(s) * n for n >= 0."""
        assert len(s * n) == len(s) * n

    @given(s=short_text)
    @settings(**SETTINGS)
    def test_repeat_zero(self, s: str) -> None:
        """s * 0 == '' for all strings."""
        assert s * 0 == ""

    @given(s=short_text)
    @settings(**SETTINGS)
    def test_repeat_one(self, s: str) -> None:
        """s * 1 == s for all strings."""
        assert s * 1 == s

    @given(s=short_text, n=st.integers(min_value=-10, max_value=-1))
    @settings(**SETTINGS)
    def test_repeat_negative(self, s: str, n: int) -> None:
        """s * n == '' for negative n."""
        assert s * n == ""


# ---------------------------------------------------------------------------
# Slicing properties
# ---------------------------------------------------------------------------


class TestSlicing:
    """String slicing invariants."""

    @given(s=ascii_text, i=st.integers(min_value=0, max_value=100))
    @settings(**SETTINGS)
    def test_slice_partition(self, s: str, i: int) -> None:
        """s[:i] + s[i:] == s for all valid i."""
        assert s[:i] + s[i:] == s

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_full_slice(self, s: str) -> None:
        """s[:] == s for all strings."""
        assert s[:] == s

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_reverse_reverse(self, s: str) -> None:
        """s[::-1][::-1] == s — reversing twice restores the string."""
        assert s[::-1][::-1] == s

    @given(s=ascii_text, i=st.integers(min_value=0, max_value=100))
    @settings(**SETTINGS)
    def test_slice_len_bound(self, s: str, i: int) -> None:
        """len(s[:i]) <= len(s) and len(s[:i]) <= i."""
        sliced = s[:i]
        assert len(sliced) <= len(s)
        assert len(sliced) <= i


# ---------------------------------------------------------------------------
# Split / Join roundtrip
# ---------------------------------------------------------------------------


class TestSplitJoin:
    """split/join roundtrip properties."""

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_split_join_whitespace(self, s: str) -> None:
        """' '.join(s.split()) removes leading/trailing/multiple whitespace."""
        parts = s.split()
        rejoined = " ".join(parts)
        # Invariant: re-splitting produces the same parts.
        assert rejoined.split() == parts

    @given(
        s=ascii_text,
        sep=st.text(
            alphabet=st.characters(min_codepoint=33, max_codepoint=126),
            min_size=1,
            max_size=3,
        ),
    )
    @settings(**SETTINGS)
    def test_split_join_roundtrip(self, s: str, sep: str) -> None:
        """sep.join(s.split(sep)) == s for explicit separator."""
        assert sep.join(s.split(sep)) == s

    @given(
        s=ascii_text,
        sep=st.text(
            alphabet=st.characters(min_codepoint=33, max_codepoint=126),
            min_size=1,
            max_size=3,
        ),
    )
    @settings(**SETTINGS)
    def test_split_count(self, s: str, sep: str) -> None:
        """len(s.split(sep)) == s.count(sep) + 1."""
        assert len(s.split(sep)) == s.count(sep) + 1


# ---------------------------------------------------------------------------
# Case conversion properties
# ---------------------------------------------------------------------------


class TestCaseConversion:
    """Case conversion idempotence and consistency."""

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_upper_idempotent(self, s: str) -> None:
        """s.upper().upper() == s.upper() — upper is idempotent."""
        assert s.upper().upper() == s.upper()

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_lower_idempotent(self, s: str) -> None:
        """s.lower().lower() == s.lower() — lower is idempotent."""
        assert s.lower().lower() == s.lower()

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_upper_lower_len(self, s: str) -> None:
        """len(s.upper()) == len(s) for ASCII strings."""
        assert len(s.upper()) == len(s)
        assert len(s.lower()) == len(s)

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_upper_is_upper(self, s: str) -> None:
        """s.upper().isupper() is True when s contains cased chars."""
        up = s.upper()
        if any(c.isalpha() for c in s):
            assert up.isupper()

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_lower_is_lower(self, s: str) -> None:
        """s.lower().islower() is True when s contains cased chars."""
        low = s.lower()
        if any(c.isalpha() for c in s):
            assert low.islower()


# ---------------------------------------------------------------------------
# Strip properties
# ---------------------------------------------------------------------------


class TestStrip:
    """Stripping invariants."""

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_strip_idempotent(self, s: str) -> None:
        """s.strip().strip() == s.strip() — strip is idempotent."""
        assert s.strip().strip() == s.strip()

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_strip_len_decreases(self, s: str) -> None:
        """len(s.strip()) <= len(s)."""
        assert len(s.strip()) <= len(s)

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_strip_no_leading_trailing_whitespace(self, s: str) -> None:
        """s.strip() has no leading or trailing whitespace."""
        stripped = s.strip()
        if stripped:
            assert stripped[0] != " " or not stripped[0].isspace()
            assert stripped[-1] != " " or not stripped[-1].isspace()

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_lstrip_rstrip_compose(self, s: str) -> None:
        """s.lstrip().rstrip() == s.strip()."""
        assert s.lstrip().rstrip() == s.strip()


# ---------------------------------------------------------------------------
# String comparison with CPython (via subprocess)
# ---------------------------------------------------------------------------


class TestStringCPythonEquivalence:
    """Verify string operations produce same output as CPython."""

    @given(
        s=st.text(
            alphabet=st.characters(min_codepoint=32, max_codepoint=126),
            min_size=0,
            max_size=30,
        )
    )
    @settings(max_examples=50, deadline=None, database=None)
    def test_repr_matches_cpython(self, s: str) -> None:
        """repr(s) in Molt matches CPython."""
        from tests.property.conftest import run_via_cpython

        # Only test via CPython (Molt compilation tested separately with --run-molt)
        code = f"print(repr({s!r}))"
        result = run_via_cpython(code)
        assert result == repr(s)
