"""Native locale category and encoding operations."""

from _intrinsics import require_intrinsic as _require_intrinsic


# This is a target property, not mutable sys.platform or provider probing.
if _require_intrinsic("molt_sys_platform")() == "win32":
    _MOLT_GETDEFAULTLOCALE = _require_intrinsic("molt_locale_getdefaultlocale")

    def _getdefaultlocale():
        return _MOLT_GETDEFAULTLOCALE()

    _getdefaultlocale.__text_signature__ = "($module, /)"
    _require_intrinsic("molt_function_set_builtin")(_getdefaultlocale)

_MOLT_SETLOCALE = _require_intrinsic("molt_locale_setlocale")
_MOLT_GETENCODING = _require_intrinsic("molt_locale_getencoding")
globals().update(_require_intrinsic("molt_locale_constants")())


class Error(Exception):
    """Error raised when the platform refuses a locale setting."""


def setlocale(category, locale=None, /):
    result = _MOLT_SETLOCALE(category, locale)
    if result is None:
        raise Error("unsupported locale setting")
    return result


def getencoding():
    return _MOLT_GETENCODING()


setlocale.__text_signature__ = "($module, category, locale=<unrepresentable>, /)"
getencoding.__text_signature__ = "($module, /)"
_require_intrinsic("molt_function_set_builtin")(setlocale)
_require_intrinsic("molt_function_set_builtin")(getencoding)


def localeconv() -> dict:
    """Return a dict of locale-specific numeric and monetary conventions.

    This existing C-locale projection does not yet read libc lconv fields.
    Non-C numeric and monetary conventions remain unqualified.
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

    Non-C collation remains unqualified; this existing projection is lexical.
    """
    if s1 == s2:
        return 0
    return -1 if s1 < s2 else 1


def strxfrm(s: str) -> str:
    """Transform a string to one usable for locale-aware comparisons.

    Non-C collation transforms remain unqualified in this existing projection.
    """
    return s


globals().pop("_require_intrinsic", None)
