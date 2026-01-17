"""Minimal sys shim for Molt."""

from __future__ import annotations

from typing import Any


try:
    from molt import capabilities as _capabilities

    _ALLOW_ENV = _capabilities.has("env.read")
except Exception:
    _ALLOW_ENV = False

if _ALLOW_ENV:
    try:
        import importlib as _importlib

        _py_sys = _importlib.import_module("sys")
    except Exception:
        _py_sys = None
else:
    _py_sys = None

__all__ = [
    "argv",
    "platform",
    "version",
    "version_info",
    "path",
    "modules",
    "stdin",
    "stdout",
    "stderr",
    "getrecursionlimit",
    "setrecursionlimit",
    "exc_info",
    "_getframe",
    "getdefaultencoding",
    "getfilesystemencoding",
]

try:
    argv = _molt_getargv()  # type: ignore[unresolved-reference]
except NameError:
    if _py_sys is not None:
        argv = list(getattr(_py_sys, "argv", []))
    else:
        argv = []

if _py_sys is not None:
    platform = getattr(_py_sys, "platform", "molt")
    version = getattr(_py_sys, "version", "3.13.0 (molt)")
    version_info = getattr(_py_sys, "version_info", (3, 13, 0, "final", 0))
    path = list(getattr(_py_sys, "path", []))
    modules = getattr(_py_sys, "modules", {})
    stdin = getattr(_py_sys, "stdin", None)
    stdout = getattr(_py_sys, "stdout", None)
    stderr = getattr(_py_sys, "stderr", None)
    _default_encoding = getattr(_py_sys, "getdefaultencoding", lambda: "utf-8")()
    _fs_encoding = getattr(_py_sys, "getfilesystemencoding", lambda: "utf-8")()
else:
    platform = "molt"
    version = "3.13.0 (molt)"
    version_info = (3, 13, 0, "final", 0)
    path = []
    modules: dict[str, Any] = {}
    stdin = None
    stdout = None
    stderr = None
    _default_encoding = "utf-8"
    _fs_encoding = "utf-8"

_recursionlimit = 1000


def getrecursionlimit() -> int:
    try:
        return _molt_getrecursionlimit()  # type: ignore[unresolved-reference]
    except NameError:
        pass
    return _recursionlimit


def setrecursionlimit(limit: int) -> None:
    global _recursionlimit
    try:
        _molt_setrecursionlimit(limit)  # type: ignore[unresolved-reference]
        return
    except NameError:
        pass
    if not isinstance(limit, int):
        name = type(limit).__name__
        raise TypeError(f"'{name}' object cannot be interpreted as an integer")
    if limit < 1:
        raise ValueError("recursion limit must be greater or equal than 1")
    _recursionlimit = limit


def exc_info() -> tuple[Any, Any, Any]:
    if _py_sys is not None:
        return _py_sys.exc_info()
    try:
        exc = _molt_exception_active()  # type: ignore[unresolved-reference]
    except NameError:
        exc = None
    if exc is None:
        try:
            exc = _molt_exception_last()  # type: ignore[unresolved-reference]
        except NameError:
            exc = None
    if exc is None:
        return None, None, None
    return type(exc), exc, getattr(exc, "__traceback__", None)


def _getframe(depth: int = 0) -> Any | None:
    if _py_sys is not None and hasattr(_py_sys, "_getframe"):
        try:
            return _py_sys._getframe(depth + 1)
        except Exception:
            return None
    return None


def getdefaultencoding() -> str:
    return _default_encoding


def getfilesystemencoding() -> str:
    return _fs_encoding
