"""Library that exposes various tables found in the StringPrep RFC 3454.

There are two kinds of tables: sets, for which a member test is provided,
and mappings, for which a mapping function is provided.

Intrinsic-backed implementation for Molt.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_stringprep_in_table = _require_intrinsic("molt_stringprep_in_table")
_stringprep_map_table_b3 = _require_intrinsic("molt_stringprep_map_table_b3")


def in_table_a1(code):
    """Table A.1: Unassigned code points in Unicode 3.2."""
    return _stringprep_in_table("a1", code)


def in_table_b1(code):
    """Table B.1: Commonly mapped to nothing."""
    return _stringprep_in_table("b1", code)


def map_table_b3(code):
    """Table B.3: Case folding with exceptions."""
    return _stringprep_map_table_b3(code)


def map_table_b2(a):
    """Table B.2: NFKC case folding.

    NOTE: Requires UCD 3.2.0 NFKC normalization. Currently falls back to
    map_table_b3 without NFKC normalization, which is correct for ASCII
    labels but may differ for complex Unicode inputs.
    """
    # Full implementation requires:
    #   al = map_table_b3(a)
    #   b = unicodedata_3_2_0.normalize("NFKC", al)
    #   bl = "".join([map_table_b3(ch) for ch in b])
    #   c = unicodedata_3_2_0.normalize("NFKC", bl)
    #   return c if b != c else al
    # For now, return b3 result (correct for the common case)
    return map_table_b3(a)


def in_table_c11(code):
    """Table C.1.1: ASCII space characters."""
    return _stringprep_in_table("c11", code)


def in_table_c12(code):
    """Table C.1.2: Non-ASCII space characters."""
    return _stringprep_in_table("c12", code)


def in_table_c11_c12(code):
    """Tables C.1.1 + C.1.2: All space characters."""
    return _stringprep_in_table("c11_c12", code)


def in_table_c21(code):
    """Table C.2.1: ASCII control characters."""
    return _stringprep_in_table("c21", code)


def in_table_c22(code):
    """Table C.2.2: Non-ASCII control characters."""
    return _stringprep_in_table("c22", code)


def in_table_c21_c22(code):
    """Tables C.2.1 + C.2.2: All control characters."""
    return _stringprep_in_table("c21_c22", code)


def in_table_c3(code):
    """Table C.3: Private use."""
    return _stringprep_in_table("c3", code)


def in_table_c4(code):
    """Table C.4: Non-character code points."""
    return _stringprep_in_table("c4", code)


def in_table_c5(code):
    """Table C.5: Surrogate codes."""
    return _stringprep_in_table("c5", code)


def in_table_c6(code):
    """Table C.6: Inappropriate for plain text."""
    return _stringprep_in_table("c6", code)


def in_table_c7(code):
    """Table C.7: Inappropriate for canonical representation."""
    return _stringprep_in_table("c7", code)


def in_table_c8(code):
    """Table C.8: Change display properties or deprecated."""
    return _stringprep_in_table("c8", code)


def in_table_c9(code):
    """Table C.9: Tagging characters."""
    return _stringprep_in_table("c9", code)


def in_table_d1(code):
    """Table D.1: Characters with bidirectional property R or AL."""
    return _stringprep_in_table("d1", code)


def in_table_d2(code):
    """Table D.2: Characters with bidirectional property L."""
    return _stringprep_in_table("d2", code)


globals().pop("_require_intrinsic", None)
