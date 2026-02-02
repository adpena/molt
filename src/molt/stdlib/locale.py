"""Minimal locale shim for Molt."""

from __future__ import annotations

__all__ = ["LC_ALL", "setlocale", "getpreferredencoding", "getlocale"]

LC_ALL = 6

_current_locale = "C"


def setlocale(_category: int, locale: str | None = None) -> str:
    global _current_locale
    if locale is None:
        return _current_locale
    if locale in {"", "C", "POSIX"}:
        _current_locale = "C"
        return _current_locale
    _current_locale = locale
    return _current_locale


def getpreferredencoding(_do_setlocale: bool = True) -> str:
    if _current_locale in {"C", "POSIX"}:
        return "US-ASCII"
    return "UTF-8"


def getlocale(_category: int | None = None) -> tuple[str | None, str | None]:
    if _current_locale in {"C", "POSIX"}:
        return (None, None)
    return (_current_locale, getpreferredencoding(False))
