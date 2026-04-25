"""Low-level locale helpers used by `locale`.

CPython exposes this as a C extension module that the public `locale`
Python module wraps. Molt's `locale` module is already intrinsic-backed,
so `_locale` re-exports the same names so any third-party code that
imports `_locale` directly gets the working implementation.
"""

from __future__ import annotations

from locale import (
    CHAR_MAX,
    Error,
    LC_ALL,
    LC_COLLATE,
    LC_CTYPE,
    LC_MESSAGES,
    LC_MONETARY,
    LC_NUMERIC,
    LC_TIME,
    getlocale,
    getpreferredencoding,
    localeconv,
    setlocale,
    strcoll,
    strxfrm,
)


__all__ = [
    "CHAR_MAX",
    "Error",
    "LC_ALL",
    "LC_COLLATE",
    "LC_CTYPE",
    "LC_MESSAGES",
    "LC_MONETARY",
    "LC_NUMERIC",
    "LC_TIME",
    "getlocale",
    "getpreferredencoding",
    "localeconv",
    "setlocale",
    "strcoll",
    "strxfrm",
]
