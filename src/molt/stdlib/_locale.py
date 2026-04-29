"""Low-level locale helpers used by `locale`.

CPython exposes this as a C extension module that the public `locale`
Python module wraps. Molt's `locale` module is already intrinsic-backed,
so `_locale` re-exports the same names so any third-party code that
imports `_locale` directly gets the working implementation.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic("molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()
del _MOLT_IMPORT_SMOKE_RUNTIME_READY


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


globals().pop("_require_intrinsic", None)
