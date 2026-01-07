"""Warnings shim for Molt."""

from __future__ import annotations

from typing import Any

import fnmatch

__all__ = [
    "warn",
    "warn_explicit",
    "filterwarnings",
    "simplefilter",
    "resetwarnings",
    "catch_warnings",
    "formatwarning",
    "showwarning",
]

# TODO(stdlib-compat, owner:stdlib, milestone:SL3): implement full filter registry + once/once-per-module caches.

_filters: list[tuple[str, str, type | None, str, int]] = []
_default_action = "default"
_once_registry: set[tuple[str, type, str, int]] = set()
_record_stack: list[list[dict[str, Any]]] = []


def _normalize_category(category: Any) -> type:
    if category is None:
        return UserWarning
    if isinstance(category, type):
        return category
    return UserWarning


def _get_frame(stacklevel: int) -> Any | None:
    try:
        import sys

        return sys._getframe(stacklevel)
    except Exception:
        return None


def _get_location(stacklevel: int) -> tuple[str, int, str]:
    frame = _get_frame(stacklevel + 1)
    if frame is None:
        return "<string>", 1, "__main__"
    filename = frame.f_code.co_filename
    lineno = frame.f_lineno
    module = frame.f_globals.get("__name__", "__main__")
    return filename, lineno, module


def _matches_filter(
    message: str,
    category: type,
    module: str,
    lineno: int,
    filt: tuple[str, str, type | None, str, int],
) -> bool:
    _action, msg_pat, cat, mod_pat, line = filt
    if msg_pat and not fnmatch.fnmatchcase(message, msg_pat):
        return False
    if mod_pat and not fnmatch.fnmatchcase(module, mod_pat):
        return False
    if line and lineno != line:
        return False
    if cat is not None and not issubclass(category, cat):
        return False
    return True


def _action_for(message: str, category: type, module: str, lineno: int) -> str:
    for filt in _filters:
        if _matches_filter(message, category, module, lineno, filt):
            return filt[0]
    return _default_action


def formatwarning(
    message: Any,
    category: Any,
    filename: str,
    lineno: int,
    line: str | None = None,
) -> str:
    name = getattr(category, "__name__", "Warning")
    text = str(message)
    if line:
        return f"{filename}:{lineno}: {name}: {text}\n  {line.strip()}\n"
    return f"{filename}:{lineno}: {name}: {text}\n"


def showwarning(
    message: Any,
    category: Any = None,
    filename: str | None = None,
    lineno: int | None = None,
    file: Any | None = None,
    line: str | None = None,
) -> None:
    if filename is None:
        filename = "<string>"
    if lineno is None:
        lineno = 1
    text = formatwarning(message, category, filename, lineno, line)
    if file is not None and hasattr(file, "write"):
        file.write(text)
        return
    print(text, end="")


def warn(
    message: Any,
    category: Any = None,
    stacklevel: int = 1,
    source: Any | None = None,
) -> None:
    _ = source
    category = _normalize_category(category)
    msg_text = str(message)
    filename, lineno, module = _get_location(stacklevel)
    action = _action_for(msg_text, category, module, lineno)

    if action in {"ignore", "off"}:
        return None
    if action == "error":
        raise category(message)
    if action in {"once", "default"}:
        key = (msg_text, category, filename, lineno)
        if key in _once_registry:
            return None
        _once_registry.add(key)

    record = {
        "message": message,
        "category": category,
        "filename": filename,
        "lineno": lineno,
        "module": module,
    }
    if _record_stack:
        _record_stack[-1].append(record)
        return None

    showwarning(message, category, filename, lineno)
    return None


def warn_explicit(
    message: Any,
    category: Any,
    filename: str,
    lineno: int,
    module: str | None = None,
    registry: Any | None = None,
    module_globals: Any | None = None,
    source: Any | None = None,
) -> None:
    _ = (registry, module_globals, source)
    category = _normalize_category(category)
    msg_text = str(message)
    module_name = module or "__main__"
    action = _action_for(msg_text, category, module_name, lineno)

    if action in {"ignore", "off"}:
        return None
    if action == "error":
        raise category(message)
    if action in {"once", "default"}:
        key = (msg_text, category, filename, lineno)
        if key in _once_registry:
            return None
        _once_registry.add(key)

    record = {
        "message": message,
        "category": category,
        "filename": filename,
        "lineno": lineno,
        "module": module_name,
    }
    if _record_stack:
        _record_stack[-1].append(record)
        return None

    showwarning(message, category, filename, lineno)
    return None


def filterwarnings(
    action: str = "default",
    message: str = "",
    category: Any | None = Warning,
    module: str = "",
    lineno: int = 0,
    append: bool = False,
) -> None:
    cat = None if category is None else category
    filt = (action, message, cat, module, lineno)
    if append:
        _filters.append(filt)
    else:
        _filters.insert(0, filt)


def simplefilter(
    action: str = "default",
    category: Any | None = Warning,
    lineno: int = 0,
    append: bool = False,
) -> None:
    filterwarnings(action, "", category, "", lineno, append=append)


def resetwarnings() -> None:
    _filters.clear()


class _CatchWarnings:
    def __init__(self, record: bool, module: Any | None) -> None:
        self._record = record
        self._module = module
        self._log: list[dict[str, Any]] = []

    def __enter__(self) -> Any:
        if self._record:
            _record_stack.append(self._log)
            return self._log
        _record_stack.append(self._log)
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        _ = (exc_type, exc, tb)
        if _record_stack:
            _record_stack.pop()
        return False


def catch_warnings(record: bool = False, module: Any | None = None) -> _CatchWarnings:
    return _CatchWarnings(record, module)
