"""Minimal sys shim for Molt."""

from __future__ import annotations

from typing import Any

import collections as _collections

import builtins as _builtins


def _load_intrinsic(name: str) -> Any | None:
    direct = globals().get(name)
    if direct is not None:
        return direct
    return getattr(_builtins, name, None)


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
    "asyncgen_hooks",
    "get_asyncgen_hooks",
    "set_asyncgen_hooks",
]

_MOLT_GETARGV = _load_intrinsic("_molt_getargv")
_MOLT_GETRECURSIONLIMIT = _load_intrinsic("_molt_getrecursionlimit")
_MOLT_SETRECURSIONLIMIT = _load_intrinsic("_molt_setrecursionlimit")
_MOLT_EXCEPTION_ACTIVE = _load_intrinsic("_molt_exception_active")
_MOLT_EXCEPTION_LAST = _load_intrinsic("_molt_exception_last")
_MOLT_ASYNCGEN_HOOKS_GET = _load_intrinsic("_molt_asyncgen_hooks_get")
_MOLT_ASYNCGEN_HOOKS_SET = _load_intrinsic("_molt_asyncgen_hooks_set")

if callable(_MOLT_GETARGV):
    argv = list(_MOLT_GETARGV())
elif _py_sys is not None:
    argv = list(getattr(_py_sys, "argv", []))
else:
    argv = []

_existing_modules = globals().get("modules")

if _py_sys is not None:
    platform = getattr(_py_sys, "platform", "molt")
    version = getattr(_py_sys, "version", "3.14.0 (molt)")
    version_info = getattr(_py_sys, "version_info", (3, 14, 0, "final", 0))
    path = list(getattr(_py_sys, "path", []))
    modules = getattr(_py_sys, "modules", _existing_modules or {})
    stdin = getattr(_py_sys, "stdin", None)
    stdout = getattr(_py_sys, "stdout", None)
    stderr = getattr(_py_sys, "stderr", None)
    _default_encoding = getattr(_py_sys, "getdefaultencoding", lambda: "utf-8")()
    _fs_encoding = getattr(_py_sys, "getfilesystemencoding", lambda: "utf-8")()
else:
    platform = "molt"
    version = "3.14.0 (molt)"
    version_info = (3, 14, 0, "final", 0)
    path = []
    if _existing_modules is None:
        modules: dict[str, Any] = {}
    else:
        modules = _existing_modules
    stdin = None
    stdout = None
    stderr = None
    _default_encoding = "utf-8"
    _fs_encoding = "utf-8"

_recursionlimit = 1000
asyncgen_hooks = _collections.namedtuple("asyncgen_hooks", ["firstiter", "finalizer"])
_ASYNCGEN_FIRSTITER: Any | None = None
_ASYNCGEN_FINALIZER: Any | None = None


def getrecursionlimit() -> int:
    if callable(_MOLT_GETRECURSIONLIMIT):
        return int(_MOLT_GETRECURSIONLIMIT())
    return _recursionlimit


def setrecursionlimit(limit: int) -> None:
    global _recursionlimit
    if callable(_MOLT_SETRECURSIONLIMIT):
        _MOLT_SETRECURSIONLIMIT(limit)
        return
    if not isinstance(limit, int):
        name = type(limit).__name__
        raise TypeError(f"'{name}' object cannot be interpreted as an integer")
    if limit < 1:
        raise ValueError("recursion limit must be greater or equal than 1")
    _recursionlimit = limit


def exc_info() -> tuple[Any, Any, Any]:
    if _py_sys is not None:
        return _py_sys.exc_info()
    exc = None
    if callable(_MOLT_EXCEPTION_ACTIVE):
        exc = _MOLT_EXCEPTION_ACTIVE()
    if exc is None:
        if callable(_MOLT_EXCEPTION_LAST):
            exc = _MOLT_EXCEPTION_LAST()
    if exc is None:
        return None, None, None
    return type(exc), exc, getattr(exc, "__traceback__", None)


def _getframe(depth: int = 0) -> Any | None:
    # TODO(introspection, owner:runtime, milestone:TC2, priority:P2, status:partial): implement sys._getframe for compiled runtimes.
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


def get_asyncgen_hooks() -> Any:
    if callable(_MOLT_ASYNCGEN_HOOKS_GET):
        hooks = _MOLT_ASYNCGEN_HOOKS_GET()
        try:
            firstiter, finalizer = hooks
        except Exception:
            firstiter, finalizer = None, None
        return asyncgen_hooks(firstiter, finalizer)
    return asyncgen_hooks(_ASYNCGEN_FIRSTITER, _ASYNCGEN_FINALIZER)


def set_asyncgen_hooks(
    *, firstiter: Any | None = None, finalizer: Any | None = None
) -> None:
    global _ASYNCGEN_FIRSTITER, _ASYNCGEN_FINALIZER
    if callable(_MOLT_ASYNCGEN_HOOKS_SET):
        _MOLT_ASYNCGEN_HOOKS_SET(firstiter, finalizer)
        return None
    if firstiter is not None and not callable(firstiter):
        raise TypeError("firstiter must be callable or None")
    if finalizer is not None and not callable(finalizer):
        raise TypeError("finalizer must be callable or None")
    _ASYNCGEN_FIRSTITER = firstiter
    _ASYNCGEN_FINALIZER = finalizer
    return None
