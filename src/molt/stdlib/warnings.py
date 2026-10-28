"""Warnings shim for Molt."""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

import sys as _sys


def _warnings_intrinsic(name: str):
    return _require_intrinsic(name)


# `warn` calls this intrinsic directly from its own body: the intrinsic counts
# exactly that one shim frame before CPython's stacklevel walk begins.
_MOLT_WARNINGS_WARN = _require_intrinsic("molt_warnings_warn")


def _molt_warnings_warn_explicit(*args):
    return _warnings_intrinsic("molt_warnings_warn_explicit")(*args)


def _molt_warnings_formatwarning(*args):
    return _warnings_intrinsic("molt_warnings_formatwarning")(*args)


def _molt_warnings_showwarning(*args):
    return _warnings_intrinsic("molt_warnings_showwarning")(*args)


def _molt_warnings_simplefilter(*args):
    return _warnings_intrinsic("molt_warnings_simplefilter")(*args)


def _molt_warnings_filterwarnings(*args):
    return _warnings_intrinsic("molt_warnings_filterwarnings")(*args)


def _molt_warnings_resetwarnings(*args):
    return _warnings_intrinsic("molt_warnings_resetwarnings")(*args)


def _molt_warnings_filters_get(*args):
    return _warnings_intrinsic("molt_warnings_filters_get")(*args)


class _WarningRecord:
    __slots__ = ("message", "category", "filename", "lineno", "module")

    def __init__(
        self,
        message: object,
        category: type,
        filename: str,
        lineno: int,
        module: str,
    ) -> None:
        self.message = message
        self.category = category
        self.filename = filename
        self.lineno = lineno
        self.module = module


__all__ = [
    "warn",
    "warn_explicit",
    "filterwarnings",
    "simplefilter",
    "resetwarnings",
    "catch_warnings",
    "formatwarning",
    "showwarning",
    "deprecated",
]

_filters: list[tuple[str, Any, type | None, Any, int]] = []
_default_action = "default"
_once_registry: set[tuple[str, type]] = set()
_record_stack: list[list["_WarningRecord"]] = []

_filters_version = 0

_VALID_ACTIONS = {
    "default",
    "error",
    "ignore",
    "always",
    "module",
    "once",
    "off",
}


def _normalize_category(category: Any) -> type:
    if category is None:
        return UserWarning
    if isinstance(category, type):
        return category
    return UserWarning


def _bump_filters_version() -> None:
    global _filters_version
    _filters_version += 1


def _resolve_registry(registry: Any | None) -> dict[object, Any] | None:
    # CPython's `already_warned`: a registry from an older filter version
    # forgets what it suppressed.
    if registry is None:
        return None
    if not isinstance(registry, dict):
        raise TypeError("'registry' must be a dict or None")
    if registry.get("version") != _filters_version:
        registry.clear()
        registry["version"] = _filters_version
    return registry


def _module_from_filename(filename: Any) -> str:
    # CPython's `warn_explicit` names an unnamed module after its file.
    module = filename or "<unknown>"
    if module[-3:].lower() == ".py":
        module = module[:-3]
    return module


def _already_warned(
    action: str,
    msg_text: Any,
    category: type,
    lineno: int,
    registry: dict[object, Any] | None,
) -> bool:
    # CPython `warn_explicit` after the filter search: record the warning
    # unless the action is "always", then apply once/module deduplication.
    if action == "always":
        return False
    if registry is not None:
        registry[(msg_text, category, lineno)] = True
    if action == "once":
        key = (msg_text, category)
        if key in _once_registry:
            return True
        _once_registry.add(key)
        return False
    if action == "module" and registry is not None:
        key = (msg_text, category, 0)
        if registry.get(key):
            return True
        registry[key] = True
    return False


def _resolve_showwarning() -> Any:
    try:
        candidate = globals().get("showwarning")
        if callable(candidate):
            return candidate
        module = _sys.modules.get(__name__)
        if module is not None:
            candidate = getattr(module, "showwarning", None)
            if callable(candidate):
                return candidate
    except Exception:
        pass
    return showwarning


def _has_custom_showwarning() -> bool:
    resolved = _resolve_showwarning()
    return resolved is not _DEFAULT_SHOWWARNING


def _capture_streams() -> list[tuple[Any, str]]:
    try:
        module = _sys.modules.get(__name__)
        if module is None:
            return []
        streams = getattr(module, "_molt_capture_streams", None)
        if isinstance(streams, list):
            return streams
    except Exception:
        pass
    return []


def _matches_filter(
    message: str,
    category: type,
    module: str,
    lineno: int,
    filt: tuple[str, Any, type | None, Any, int],
) -> bool:
    _action, msg_pat, cat, mod_pat, line = filt
    if msg_pat is not None:
        if hasattr(msg_pat, "match"):
            if not msg_pat.match(message):
                return False
        else:
            if message != str(msg_pat):
                return False
    if mod_pat is not None:
        if hasattr(mod_pat, "match"):
            if not mod_pat.match(module):
                return False
        else:
            if module != str(mod_pat):
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
    return _molt_warnings_formatwarning(message, category, filename, lineno, line)


def showwarning(
    message: Any,
    category: Any,
    filename: str,
    lineno: int,
    file: Any | None = None,
    line: str | None = None,
) -> None:
    if file is None:
        _molt_warnings_showwarning(message, category, filename, lineno, file, line)
        return None
    try:
        file.write(formatwarning(message, category, filename, lineno, line))
    except OSError:
        pass
    return None


_DEFAULT_SHOWWARNING = showwarning


def warn(
    message: Any,
    category: Any = None,
    stacklevel: int = 1,
    source: Any | None = None,
) -> None:
    _ = source
    # The runtime attributes the warning as CPython's `setup_context` does
    # (file, line, module and `__warningregistry__` of the frame `stacklevel`
    # levels above this call) and hands it to `warn_explicit`.
    _MOLT_WARNINGS_WARN(message, _normalize_category(category), stacklevel)
    return None


def _deprecated(
    message: str,
    *,
    remove: tuple[int, int] | None = None,
    stacklevel: int = 2,
    **_kwargs: Any,
) -> None:
    text = message
    if remove is not None:
        text = f"{message} is deprecated and will be removed in Python {remove[0]}.{remove[1]}"
    warn(text, DeprecationWarning, stacklevel=stacklevel)


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
    _ = source
    lineno = int(lineno)
    if module is None:
        module = _module_from_filename(filename)
    category = _normalize_category(category)
    registry = _resolve_registry(registry)
    # Fast path: the native engine filters and displays when no record or
    # capture hook is installed.
    if not _record_stack and not _capture_streams() and not _has_custom_showwarning():
        _molt_warnings_warn_explicit(
            message, category, filename, lineno, module, registry, module_globals
        )
        return None
    # Captured path: CPython's `warn_explicit` over the Python filter list.
    if isinstance(message, Warning):
        text = str(message)
        category = message.__class__
    else:
        text = message
        message = category(message)
    if registry is not None and registry.get((text, category, lineno)):
        return None
    action = _action_for(text, category, module, lineno)
    if action not in _VALID_ACTIONS:
        raise ValueError(f"invalid warnings action: {action!r}")
    if action in {"ignore", "off"}:
        return None
    if action == "error":
        raise message
    if _already_warned(action, text, category, lineno, registry):
        return None

    if _record_stack:
        _record_stack[-1].append(
            _WarningRecord(message, category, filename, lineno, module)
        )
        return None

    streams = _capture_streams()
    if streams:
        rendered = formatwarning(message, category, filename, lineno)
        for stream, terminator in streams:
            try:
                if stream is not None and hasattr(stream, "write"):
                    stream.write(rendered.rstrip() + terminator)
                    flush = getattr(stream, "flush", None)
                    if callable(flush):
                        flush()
            except Exception:
                continue
        return None
    _resolve_showwarning()(message, category, filename, lineno)
    return None


def filterwarnings(
    action: str = "default",
    message: str = "",
    category: Any | None = Warning,
    module: str = "",
    lineno: int = 0,
    append: bool = False,
) -> None:
    if not isinstance(action, str):
        raise TypeError("action must be a string")
    action = action.lower()
    if action not in _VALID_ACTIONS:
        raise ValueError(f"invalid warnings action: {action!r}")
    # Sync to Rust filter state.
    _molt_warnings_filterwarnings(action, message, category, module, lineno, append)
    # Also maintain Python-side filter list for catch_warnings save/restore.
    msg_pat = None
    mod_pat = None
    if message or module:
        # Lazy import: `re` is only needed to pre-compile filter patterns, which
        # most callers never supply. Importing it here (instead of at module
        # scope) keeps `re`'s intrinsics out of the static reach of every
        # program that imports `warnings` — `warnings` is pulled transitively by
        # vast amounts of code that never installs a pattern filter.
        import re as _re

        msg_flags = getattr(_re, "IGNORECASE", getattr(_re, "I", 0))
        if message:
            msg_pat = _re.compile(message, msg_flags)
        if module:
            mod_pat = _re.compile(module)
    cat = None if category is None else category
    filt = (action, msg_pat, cat, mod_pat, lineno)
    if append:
        _filters.append(filt)
    else:
        _filters.insert(0, filt)
    _bump_filters_version()


def simplefilter(
    action: str = "default",
    category: Any | None = Warning,
    lineno: int = 0,
    append: bool = False,
) -> None:
    _molt_warnings_simplefilter(action, category, lineno, append)
    filterwarnings(action, "", category, "", lineno, append=append)


def resetwarnings() -> None:
    _molt_warnings_resetwarnings()
    _filters.clear()
    _once_registry.clear()
    _bump_filters_version()


class _CatchWarnings:
    def __init__(self, record: bool, module: Any | None) -> None:
        self._record = record
        self._module = module
        self._entered = False
        self._record_list: list[_WarningRecord] | None = None
        self._saved_filters: list[tuple[str, Any, type | None, Any, int]] | None = None
        self._saved_once_registry: set[tuple[str, type]] | None = None
        self._saved_default_action: str | None = None
        self._saved_filters_version: int | None = None

    def __enter__(self) -> Any:
        global _default_action, _filters, _filters_version, _once_registry
        if self._entered:
            raise RuntimeError("Cannot enter catch_warnings twice")
        self._entered = True
        self._saved_filters = list(_filters)
        self._saved_once_registry = set(_once_registry)
        self._saved_default_action = _default_action
        self._saved_filters_version = _filters_version
        _filters = list(_filters)
        _once_registry = set()
        _bump_filters_version()
        if self._record:
            self._record_list = []
            _record_stack.append(self._record_list)
            return self._record_list
        return None

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        global _default_action, _filters, _filters_version, _once_registry
        if not self._entered:
            raise RuntimeError("Cannot exit catch_warnings without entering first")
        _ = (exc_type, exc, tb)
        if self._record and _record_stack:
            _record_stack.pop()
        if self._saved_filters is not None:
            _filters = self._saved_filters
        if self._saved_once_registry is not None:
            _once_registry = self._saved_once_registry
        if self._saved_default_action is not None:
            _default_action = self._saved_default_action
        if self._saved_filters_version is not None:
            _filters_version = self._saved_filters_version
        return False


class deprecated:
    """Indicate that a class, function or overload is deprecated.

    PEP 702 -- added in Python 3.13.

    Usage::

        @deprecated("Use new_func instead")
        def old_func():
            ...

        @deprecated("Use NewClass instead")
        class OldClass:
            ...
    """

    def __init__(
        self,
        message: str,
        /,
        *,
        category: type = DeprecationWarning,
        stacklevel: int = 1,
    ):
        if not isinstance(message, str):
            raise TypeError("expected a string as the first argument")
        self.message = message
        self.category = category
        self.stacklevel = stacklevel

    def __call__(self, arg: Any) -> Any:
        if isinstance(arg, type):
            original_init = arg.__init__

            def new_init(self_inner: Any, *args: Any, **kwargs: Any) -> None:
                warn(
                    self.message,
                    category=self.category,
                    stacklevel=self.stacklevel + 1,
                )
                original_init(self_inner, *args, **kwargs)

            new_init.__name__ = "__init__"
            new_init.__qualname__ = f"{arg.__qualname__}.__init__"
            arg.__init__ = new_init  # type: ignore[assignment]
            arg.__deprecated__ = self.message  # type: ignore[attr-defined]
            return arg
        else:

            def wrapper(*args: Any, **kwargs: Any) -> Any:
                warn(
                    self.message,
                    category=self.category,
                    stacklevel=self.stacklevel + 1,
                )
                return arg(*args, **kwargs)

            wrapper.__name__ = getattr(arg, "__name__", wrapper.__name__)
            wrapper.__qualname__ = getattr(arg, "__qualname__", wrapper.__qualname__)
            wrapper.__doc__ = getattr(arg, "__doc__", None)
            wrapper.__wrapped__ = arg  # type: ignore[attr-defined]
            wrapper.__deprecated__ = self.message  # type: ignore[attr-defined]
            return wrapper


def catch_warnings(record: bool = False, module: Any | None = None) -> _CatchWarnings:
    return _CatchWarnings(record, module)
