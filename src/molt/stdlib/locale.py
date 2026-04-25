"""Intrinsic-backed locale shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = [
    "LC_CTYPE",
    "LC_NUMERIC",
    "LC_TIME",
    "LC_COLLATE",
    "LC_MONETARY",
    "LC_MESSAGES",
    "LC_ALL",
    "CHAR_MAX",
    "Error",
    "setlocale",
    "getpreferredencoding",
    "getlocale",
    "getdefaultlocale",
    "normalize",
    "localeconv",
    "strcoll",
    "strxfrm",
]

# Standard POSIX category indices — match CPython's order so user code that
# stashes integer values keeps working across translation units.
LC_CTYPE = 0
LC_NUMERIC = 1
LC_TIME = 2
LC_COLLATE = 3
LC_MONETARY = 4
LC_MESSAGES = 5
LC_ALL = 6

CHAR_MAX = 127


class Error(Exception):
    """Locale error."""


_MOLT_LOCALE_SETLOCALE = _require_intrinsic("molt_locale_setlocale")
_MOLT_LOCALE_GETPREFERREDENCODING = _require_intrinsic(
    "molt_locale_getpreferredencoding"
)
_MOLT_LOCALE_GETLOCALE = _require_intrinsic("molt_locale_getlocale")


def setlocale(category: object, locale: object = None) -> str:
    return _MOLT_LOCALE_SETLOCALE(category, locale)


def getpreferredencoding(do_setlocale: object = True) -> str:
    return _MOLT_LOCALE_GETPREFERREDENCODING(do_setlocale)


def getlocale(category: object | None = None) -> tuple[object, object]:
    return _MOLT_LOCALE_GETLOCALE(category)


def getdefaultlocale(envvars=("LC_ALL", "LC_CTYPE", "LANG", "LANGUAGE")):
    """Return the (language code, encoding) tuple of the user's preferred locale.

    Mirrors CPython 3.12 — defers to getlocale(LC_CTYPE) for the resolved
    language code and pairs it with the preferred encoding.
    """
    lang, enc = getlocale(LC_CTYPE)
    if enc is None:
        enc = getpreferredencoding(False)
    return lang, enc


def normalize(localename: str) -> str:
    """Return a normalized locale name for the given alias.

    The molt locale shim is deterministic — there is no large alias table
    to honor. Pass through verbatim, trimmed, with a `.UTF-8` default if
    no encoding is specified, matching the most common CPython mapping.
    """
    name = localename.strip()
    if "." in name or name.upper() == "C" or name == "POSIX":
        return name
    return name + ".UTF-8" if name else name


def localeconv() -> dict:
    """Return a dict of locale-specific numeric and monetary conventions.

    Returns the locale-independent C-style defaults — adequate for the
    deterministic compiled-binary contract. Keys mirror CPython 3.12.
    """
    return {
        "decimal_point": ".",
        "thousands_sep": "",
        "grouping": [],
        "int_curr_symbol": "",
        "currency_symbol": "",
        "mon_decimal_point": "",
        "mon_thousands_sep": "",
        "mon_grouping": [],
        "positive_sign": "",
        "negative_sign": "",
        "int_frac_digits": CHAR_MAX,
        "frac_digits": CHAR_MAX,
        "p_cs_precedes": CHAR_MAX,
        "p_sep_by_space": CHAR_MAX,
        "n_cs_precedes": CHAR_MAX,
        "n_sep_by_space": CHAR_MAX,
        "p_sign_posn": CHAR_MAX,
        "n_sign_posn": CHAR_MAX,
    }


def strcoll(s1: str, s2: str) -> int:
    """Compare strings using the current locale collation order.

    Molt's locale is deterministic — fall back to lexical comparison.
    """
    if s1 == s2:
        return 0
    return -1 if s1 < s2 else 1


def strxfrm(s: str) -> str:
    """Transform a string to one usable for locale-aware comparisons.

    With deterministic locale, the identity transform is correct.
    """
    return s


globals().pop("_require_intrinsic", None)
