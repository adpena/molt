"""Intrinsic-backed unicodedata for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_UNICODEDATA_RUNTIME_READY = _require_intrinsic("molt_unicodedata_runtime_ready")
_MOLT_UNICODEDATA_UNIDATA_VERSION = _require_intrinsic(
    "molt_unicodedata_unidata_version"
)
_MOLT_UNICODEDATA_NAME = _require_intrinsic("molt_unicodedata_name")
_MOLT_UNICODEDATA_LOOKUP = _require_intrinsic("molt_unicodedata_lookup")
_MOLT_UNICODEDATA_CATEGORY = _require_intrinsic("molt_unicodedata_category")
_MOLT_UNICODEDATA_BIDIRECTIONAL = _require_intrinsic("molt_unicodedata_bidirectional")
_MOLT_UNICODEDATA_COMBINING = _require_intrinsic("molt_unicodedata_combining")
_MOLT_UNICODEDATA_MIRRORED = _require_intrinsic("molt_unicodedata_mirrored")
_MOLT_UNICODEDATA_DECOMPOSITION = _require_intrinsic("molt_unicodedata_decomposition")
_MOLT_UNICODEDATA_DECIMAL = _require_intrinsic("molt_unicodedata_decimal")
_MOLT_UNICODEDATA_DIGIT = _require_intrinsic("molt_unicodedata_digit")
_MOLT_UNICODEDATA_NUMERIC = _require_intrinsic("molt_unicodedata_numeric")
_MOLT_UNICODEDATA_EAST_ASIAN_WIDTH = _require_intrinsic(
    "molt_unicodedata_east_asian_width"
)
_MOLT_UNICODEDATA_NORMALIZE = _require_intrinsic("molt_unicodedata_normalize")
_MOLT_UNICODEDATA_IS_NORMALIZED = _require_intrinsic("molt_unicodedata_is_normalized")

_SENTINEL = object()

unidata_version: str = _MOLT_UNICODEDATA_UNIDATA_VERSION()


def _validate_char(ch: object) -> str:
    if not isinstance(ch, str):
        raise TypeError(
            "argument must be a unicode character, not " + type(ch).__name__
        )
    if len(ch) != 1:
        raise TypeError("need a single Unicode character as parameter")
    return ch


def name(chr: str, default: object = _SENTINEL) -> str:  # noqa: A002
    """Return the name assigned to the character chr as a string."""
    _validate_char(chr)
    result = _MOLT_UNICODEDATA_NAME(chr, None if default is _SENTINEL else default)
    if result is None and default is _SENTINEL:
        raise ValueError("no such name")
    return result


def lookup(name: str) -> str:  # noqa: A001
    """Look up character by name."""
    if not isinstance(name, str):
        raise TypeError("argument must be str, not " + type(name).__name__)
    return _MOLT_UNICODEDATA_LOOKUP(name)


def category(chr: str) -> str:  # noqa: A002
    """Return the general category assigned to the character chr."""
    _validate_char(chr)
    return _MOLT_UNICODEDATA_CATEGORY(chr)


def bidirectional(chr: str) -> str:  # noqa: A002
    """Return the bidirectional category assigned to the character chr."""
    _validate_char(chr)
    return _MOLT_UNICODEDATA_BIDIRECTIONAL(chr)


def combining(chr: str) -> int:  # noqa: A002
    """Return the canonical combining class assigned to the character chr."""
    _validate_char(chr)
    return _MOLT_UNICODEDATA_COMBINING(chr)


def mirrored(chr: str) -> int:  # noqa: A002
    """Return the mirrored property assigned to the character chr."""
    _validate_char(chr)
    return _MOLT_UNICODEDATA_MIRRORED(chr)


def decomposition(chr: str) -> str:  # noqa: A002
    """Return the character decomposition mapping assigned to the character chr."""
    _validate_char(chr)
    return _MOLT_UNICODEDATA_DECOMPOSITION(chr)


def decimal(chr: str, default: object = _SENTINEL) -> int:  # noqa: A002
    """Return the decimal value assigned to the character chr."""
    _validate_char(chr)
    result = _MOLT_UNICODEDATA_DECIMAL(chr, None if default is _SENTINEL else default)
    if result is None and default is _SENTINEL:
        raise ValueError("not a decimal")
    return result


def digit(chr: str, default: object = _SENTINEL) -> int:  # noqa: A002
    """Return the digit value assigned to the character chr."""
    _validate_char(chr)
    result = _MOLT_UNICODEDATA_DIGIT(chr, None if default is _SENTINEL else default)
    if result is None and default is _SENTINEL:
        raise ValueError("not a digit")
    return result


def numeric(chr: str, default: object = _SENTINEL) -> float:  # noqa: A002
    """Return the numeric value assigned to the character chr."""
    _validate_char(chr)
    result = _MOLT_UNICODEDATA_NUMERIC(chr, None if default is _SENTINEL else default)
    if result is None and default is _SENTINEL:
        raise ValueError("not a numeric character")
    return result


def east_asian_width(chr: str) -> str:  # noqa: A002
    """Return the east asian width assigned to the character chr."""
    _validate_char(chr)
    return _MOLT_UNICODEDATA_EAST_ASIAN_WIDTH(chr)


def normalize(form: str, unistr: str) -> str:
    """Return the normal form 'form' for the Unicode string unistr."""
    if not isinstance(form, str):
        raise TypeError(
            "normalize() argument 1 must be str, not " + type(form).__name__
        )
    if not isinstance(unistr, str):
        raise TypeError(
            "normalize() argument 2 must be str, not " + type(unistr).__name__
        )
    form_upper = form.upper()
    if form_upper not in ("NFC", "NFKC", "NFD", "NFKD"):
        raise ValueError("invalid normalization form")
    return _MOLT_UNICODEDATA_NORMALIZE(form_upper, unistr)


def is_normalized(form: str, unistr: str) -> bool:
    """Return whether the Unicode string unistr is in the normal form 'form'."""
    if not isinstance(form, str):
        raise TypeError(
            "is_normalized() argument 1 must be str, not " + type(form).__name__
        )
    if not isinstance(unistr, str):
        raise TypeError(
            "is_normalized() argument 2 must be str, not " + type(unistr).__name__
        )
    form_upper = form.upper()
    if form_upper not in ("NFC", "NFKC", "NFD", "NFKD"):
        raise ValueError("invalid normalization form")
    return _MOLT_UNICODEDATA_IS_NORMALIZED(form_upper, unistr)


class UCD:
    """Unicode character database (for ucd_3_2_0 compatibility object)."""

    def normalize(self, form: str, unistr: str) -> str:
        return normalize(form, unistr)

    def name(self, chr: str, default: object = _SENTINEL) -> str:  # noqa: A002
        return name(chr, default) if default is not _SENTINEL else name(chr)

    def lookup(self, name: str) -> str:  # noqa: A001
        return lookup(name)

    def category(self, chr: str) -> str:  # noqa: A002
        return category(chr)

    def bidirectional(self, chr: str) -> str:  # noqa: A002
        return bidirectional(chr)

    def combining(self, chr: str) -> int:  # noqa: A002
        return combining(chr)

    def east_asian_width(self, chr: str) -> str:  # noqa: A002
        return east_asian_width(chr)

    def mirrored(self, chr: str) -> int:  # noqa: A002
        return mirrored(chr)

    def decomposition(self, chr: str) -> str:  # noqa: A002
        return decomposition(chr)

    def numeric(self, chr: str, default: object = _SENTINEL) -> float:  # noqa: A002
        return numeric(chr, default) if default is not _SENTINEL else numeric(chr)

    def decimal(self, chr: str, default: object = _SENTINEL) -> int:  # noqa: A002
        return decimal(chr, default) if default is not _SENTINEL else decimal(chr)

    def digit(self, chr: str, default: object = _SENTINEL) -> int:  # noqa: A002
        return digit(chr, default) if default is not _SENTINEL else digit(chr)

    def is_normalized(self, form: str, unistr: str) -> bool:
        return is_normalized(form, unistr)


ucd_3_2_0 = UCD()


__all__ = [
    "bidirectional",
    "category",
    "combining",
    "decimal",
    "decomposition",
    "digit",
    "east_asian_width",
    "is_normalized",
    "lookup",
    "mirrored",
    "name",
    "normalize",
    "numeric",
    "ucd_3_2_0",
    "unidata_version",
]

globals().pop("_require_intrinsic", None)
