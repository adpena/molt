"""Intrinsic-backed locale shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand locale parity beyond deterministic runtime shim semantics.

__all__ = ["LC_ALL", "setlocale", "getpreferredencoding", "getlocale"]

LC_ALL = 6

_MOLT_LOCALE_SETLOCALE = _require_intrinsic("molt_locale_setlocale", globals())
_MOLT_LOCALE_GETPREFERREDENCODING = _require_intrinsic(
    "molt_locale_getpreferredencoding", globals()
)
_MOLT_LOCALE_GETLOCALE = _require_intrinsic("molt_locale_getlocale", globals())


def setlocale(category: object, locale: object = None) -> str:
    return _MOLT_LOCALE_SETLOCALE(category, locale)


def getpreferredencoding(do_setlocale: object = True) -> str:
    return _MOLT_LOCALE_GETPREFERREDENCODING(do_setlocale)


def getlocale(category: object | None = None) -> tuple[object, object]:
    return _MOLT_LOCALE_GETLOCALE(category)
