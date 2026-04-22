# MOLT_META: area=property-testing
"""Property-based tests for string operation intrinsics.

Tests algebraic invariants of str.upper/lower, str.split/join,
str.encode, str.startswith, str.replace, and str.strip composition.
"""

from __future__ import annotations

from hypothesis import assume, given, settings
from hypothesis import strategies as st

# ---------------------------------------------------------------------------
# Strategies
# ---------------------------------------------------------------------------

# Printable ASCII strings — safe for case conversion tests.
ascii_text = st.text(
    alphabet=st.characters(min_codepoint=32, max_codepoint=126),
    min_size=0,
    max_size=100,
)

# ASCII letters only — upper/lower roundtrip is clean.
ascii_letters = st.text(
    alphabet=st.characters(
        min_codepoint=65,
        max_codepoint=122,
        blacklist_categories=("Cs",),
        blacklist_characters="[\\]^_`",
    ),
    min_size=0,
    max_size=50,
)

# Unicode strings including BMP characters.
unicode_text = st.text(min_size=0, max_size=50)

# Short strings for operations that may create large outputs.
short_text = st.text(
    alphabet=st.characters(min_codepoint=32, max_codepoint=126),
    min_size=0,
    max_size=20,
)

SETTINGS = dict(max_examples=200, deadline=None, database=None)


# ---------------------------------------------------------------------------
# upper / lower idempotency
# ---------------------------------------------------------------------------


class TestUpperLowerIdempotency:
    """str.upper() and str.lower() idempotency and composition."""

    @given(s=ascii_letters)
    @settings(**SETTINGS)
    def test_upper_lower_upper_idempotent(self, s: str) -> None:
        """s.upper().lower().upper() == s.upper() for ASCII letters."""
        assert s.upper().lower().upper() == s.upper()

    @given(s=ascii_letters)
    @settings(**SETTINGS)
    def test_lower_upper_lower_idempotent(self, s: str) -> None:
        """s.lower().upper().lower() == s.lower() for ASCII letters."""
        assert s.lower().upper().lower() == s.lower()

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_upper_then_lower_idempotent_on_lower(self, s: str) -> None:
        """s.upper().lower().lower() == s.upper().lower() — lower is idempotent."""
        assert s.upper().lower().lower() == s.upper().lower()

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_lower_then_upper_idempotent_on_upper(self, s: str) -> None:
        """s.lower().upper().upper() == s.lower().upper() — upper is idempotent."""
        assert s.lower().upper().upper() == s.lower().upper()


# ---------------------------------------------------------------------------
# split / join roundtrip
# ---------------------------------------------------------------------------


class TestSplitJoinRoundtrip:
    """sep.join(s.split(sep)) == s roundtrip for explicit separators."""

    @given(
        s=ascii_text,
        sep=st.text(
            alphabet=st.characters(min_codepoint=33, max_codepoint=126),
            min_size=1,
            max_size=3,
        ),
    )
    @settings(**SETTINGS)
    def test_split_join_identity(self, s: str, sep: str) -> None:
        """sep.join(s.split(sep)) == s for non-empty separator."""
        assert sep.join(s.split(sep)) == s

    @given(
        parts=st.lists(
            st.text(
                alphabet=st.characters(min_codepoint=33, max_codepoint=126),
                min_size=0,
                max_size=10,
            ),
            min_size=1,
            max_size=10,
        ),
        sep=st.text(
            alphabet=st.characters(min_codepoint=33, max_codepoint=126),
            min_size=1,
            max_size=3,
        ),
    )
    @settings(**SETTINGS)
    def test_join_split_roundtrip(self, parts: list[str], sep: str) -> None:
        """s.split(sep) after sep.join(parts) recovers parts when sep not in parts."""
        assume(all(sep not in part for part in parts))
        joined = sep.join(parts)
        assert joined.split(sep) == parts


# ---------------------------------------------------------------------------
# encode length
# ---------------------------------------------------------------------------


class TestEncodeLength:
    """UTF-8 encoding length properties."""

    @given(s=unicode_text)
    @settings(**SETTINGS)
    def test_utf8_encode_len_ge_str_len(self, s: str) -> None:
        """len(s.encode('utf-8')) >= len(s) for all unicode strings."""
        assert len(s.encode("utf-8")) >= len(s)

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_ascii_encode_len_eq_str_len(self, s: str) -> None:
        """len(s.encode('utf-8')) == len(s) for pure ASCII strings."""
        assert len(s.encode("utf-8")) == len(s)

    @given(s=unicode_text)
    @settings(**SETTINGS)
    def test_encode_decode_roundtrip(self, s: str) -> None:
        """s.encode('utf-8').decode('utf-8') == s."""
        assert s.encode("utf-8").decode("utf-8") == s


# ---------------------------------------------------------------------------
# startswith prefix
# ---------------------------------------------------------------------------


class TestStartsWith:
    """str.startswith() properties."""

    @given(s=ascii_text, n=st.integers(min_value=0, max_value=100))
    @settings(**SETTINGS)
    def test_startswith_own_prefix(self, s: str, n: int) -> None:
        """s.startswith(s[:n]) is True for all valid n."""
        assert s.startswith(s[:n])

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_startswith_self(self, s: str) -> None:
        """s.startswith(s) is always True."""
        assert s.startswith(s)

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_startswith_empty(self, s: str) -> None:
        """s.startswith('') is always True."""
        assert s.startswith("")

    @given(s=ascii_text, n=st.integers(min_value=0, max_value=100))
    @settings(**SETTINGS)
    def test_endswith_own_suffix(self, s: str, n: int) -> None:
        """s.endswith(s[len(s)-n:]) is True for all valid n."""
        suffix = s[len(s) - n :] if n <= len(s) else s
        assert s.endswith(suffix)


# ---------------------------------------------------------------------------
# replace roundtrip
# ---------------------------------------------------------------------------


class TestReplaceRoundtrip:
    """str.replace() roundtrip for disjoint a, b."""

    @given(
        s=short_text,
        a=st.text(
            alphabet=st.characters(min_codepoint=65, max_codepoint=90),
            min_size=1,
            max_size=3,
        ),
        b=st.text(
            alphabet=st.characters(min_codepoint=97, max_codepoint=122),
            min_size=1,
            max_size=3,
        ),
    )
    @settings(**SETTINGS)
    def test_replace_roundtrip_disjoint(self, s: str, a: str, b: str) -> None:
        """s.replace(a, b).replace(b, a) recovers s when a, b are disjoint and b not in s."""
        assume(a not in b and b not in a)
        assume(b not in s)
        assert s.replace(a, b).replace(b, a) == s

    @given(s=ascii_text, a=st.just(""))
    @settings(**SETTINGS)
    def test_replace_empty_with_empty(self, s: str, a: str) -> None:
        """s.replace('', '') == s."""
        assert s.replace("", "") == s

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_replace_self_identity(self, s: str) -> None:
        """s.replace(s, s) == s."""
        assert s.replace(s, s) == s


# ---------------------------------------------------------------------------
# strip composition
# ---------------------------------------------------------------------------


class TestStripComposition:
    """strip / lstrip / rstrip composition properties."""

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_strip_equals_lstrip_rstrip(self, s: str) -> None:
        """s.strip() == s.lstrip().rstrip()."""
        assert s.strip() == s.lstrip().rstrip()

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_strip_equals_rstrip_lstrip(self, s: str) -> None:
        """s.strip() == s.rstrip().lstrip()."""
        assert s.strip() == s.rstrip().lstrip()

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_lstrip_idempotent(self, s: str) -> None:
        """s.lstrip().lstrip() == s.lstrip()."""
        assert s.lstrip().lstrip() == s.lstrip()

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_rstrip_idempotent(self, s: str) -> None:
        """s.rstrip().rstrip() == s.rstrip()."""
        assert s.rstrip().rstrip() == s.rstrip()

    @given(s=ascii_text)
    @settings(**SETTINGS)
    def test_strip_preserves_inner_content(self, s: str) -> None:
        """s.strip() is a substring of s."""
        stripped = s.strip()
        if stripped:
            assert stripped in s
