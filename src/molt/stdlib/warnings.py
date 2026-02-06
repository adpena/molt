"""Warnings shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


from typing import Any
import sys as _sys

_require_intrinsic("molt_stdlib_probe", globals())


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
]

_filters: list[tuple[str, Any, type | None, Any, int]] = []
_default_action = "default"
_once_registry: set[tuple[str, type]] = set()
_record_stack: list[list["_WarningRecord"]] = []


class _SimpleRegex:
    def __init__(self, pattern: str, ignorecase: bool) -> None:
        self._pattern = pattern
        self._ignorecase = ignorecase

    def match(self, text: str) -> bool:
        return _simple_regex_match(self._pattern, text, self._ignorecase)


def _simple_regex_match(pattern: str, text: str, ignorecase: bool) -> bool:
    if ignorecase:
        pattern = pattern.lower()
        text = text.lower()
    tokens = _tokenize_pattern(pattern)

    memo: dict[tuple[int, int], bool] = {}

    def _char_matches(token: tuple[str, bool], ch: str) -> bool:
        char, is_wildcard = token
        if is_wildcard:
            return True
        return char == ch

    def _match_from(t_idx: int, s_idx: int) -> bool:
        key = (t_idx, s_idx)
        cached = memo.get(key)
        if cached is not None:
            return cached
        if t_idx == len(tokens):
            result = s_idx == len(text)
            memo[key] = result
            return result
        token, star = tokens[t_idx]
        if star:
            if _match_from(t_idx + 1, s_idx):
                memo[key] = True
                return True
            if s_idx < len(text) and _char_matches(token, text[s_idx]):
                result = _match_from(t_idx, s_idx + 1)
                memo[key] = result
                return result
            memo[key] = False
            return False
        if s_idx < len(text) and _char_matches(token, text[s_idx]):
            result = _match_from(t_idx + 1, s_idx + 1)
            memo[key] = result
            return result
        memo[key] = False
        return False

    return _match_from(0, 0)


def _tokenize_pattern(pattern: str) -> list[tuple[tuple[str, bool], bool]]:
    tokens: list[tuple[tuple[str, bool], bool]] = []
    idx = 0
    while idx < len(pattern):
        ch = pattern[idx]
        literal = False
        if ch == "\\" and idx + 1 < len(pattern):
            idx += 1
            ch = pattern[idx]
            literal = True
        is_wildcard = (ch == ".") and not literal
        star = False
        if not literal and idx + 1 < len(pattern) and pattern[idx + 1] == "*":
            star = True
            idx += 1
        tokens.append(((ch, is_wildcard), star))
        idx += 1
    return tokens


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


def _get_frame(stacklevel: int) -> Any | None:
    try:
        import sys

        return sys._getframe(stacklevel)
    except Exception:
        return None


def _get_location(stacklevel: int) -> tuple[str, int, str, Any | None]:
    frame = _get_frame(stacklevel + 1)
    if frame is None:
        return "<string>", 1, "__main__", None
    filename = "<string>"
    lineno = 1
    module = "__main__"
    module_globals = None
    try:
        code = getattr(frame, "f_code", None)
        if code is not None:
            filename = getattr(code, "co_filename", filename)
    except Exception:
        pass
    try:
        lineno = int(getattr(frame, "f_lineno", lineno))
    except Exception:
        pass
    try:
        globals_map = getattr(frame, "f_globals", None)
        if globals_map is not None and hasattr(globals_map, "get"):
            module_globals = globals_map
            module = globals_map.get("__name__", module)
    except Exception:
        pass
    return filename, lineno, module, module_globals


def _bump_filters_version() -> None:
    global _filters_version
    _filters_version += 1


def _resolve_registry(
    registry: Any | None, module_globals: Any | None
) -> dict[object, Any] | None:
    reg: Any | None = registry if isinstance(registry, dict) else None
    if (
        reg is None
        and module_globals is not None
        and hasattr(module_globals, "setdefault")
    ):
        try:
            reg = module_globals.setdefault("__warningregistry__", {})
        except Exception:
            reg = None
    if not isinstance(reg, dict):
        return None
    version = reg.get("version")
    if version != _filters_version:
        reg.clear()
        reg["version"] = _filters_version
    return reg


def _should_suppress(
    action: str,
    msg_text: str,
    category: type,
    lineno: int,
    registry: dict[object, Any] | None,
) -> bool:
    if action in {"ignore", "off"}:
        return True
    if action == "always":
        return False
    if action == "once":
        key = (msg_text, category)
        if key in _once_registry:
            return True
        _once_registry.add(key)
        return False
    if action == "module":
        if registry is None:
            return False
        key = (msg_text, category, 0)
        if key in registry:
            return True
        registry[key] = True
        return False
    if action == "default":
        if registry is None:
            return False
        key = (msg_text, category, lineno)
        if key in registry:
            return True
        registry[key] = True
        return False
    return False


def _resolve_showwarning() -> Any:
    try:
        module = _sys.modules.get(__name__)
        if module is not None:
            candidate = getattr(module, "showwarning", None)
            if callable(candidate):
                return candidate
    except Exception:
        pass
    return showwarning


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


def _file_exists_for_linecache(filename: str) -> bool:
    try:
        import os
        import sys
    except Exception:
        return False
    try:
        if os.path.isabs(filename):
            return os.path.exists(filename)
    except Exception:
        return False
    for dirname in sys.path:
        try:
            candidate = os.path.join(dirname, filename)
        except Exception:
            continue
        try:
            if os.path.exists(candidate):
                return True
        except Exception:
            continue
    return False


def formatwarning(
    message: Any,
    category: Any,
    filename: str,
    lineno: int,
    line: str | None = None,
) -> str:
    name = getattr(category, "__name__", "Warning")
    text = str(message)
    if line is None:
        try:
            from molt import capabilities as _caps

            if _caps.has("fs.read") and _file_exists_for_linecache(filename):
                import linecache

                line = linecache.getline(filename, lineno) or None
            else:
                line = None
        except Exception:
            line = None
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
    if file is None:
        file = getattr(_sys, "stderr", None)
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
    filename, lineno, module, module_globals = _get_location(stacklevel)
    action = _action_for(msg_text, category, module, lineno)

    if action not in _VALID_ACTIONS:
        raise ValueError(f"invalid warnings action: {action!r}")
    if action == "error":
        raise category(message)
    registry = _resolve_registry(None, module_globals)
    if _should_suppress(action, msg_text, category, lineno, registry):
        return None

    record = _WarningRecord(
        category(message),
        category,
        filename,
        lineno,
        module,
    )
    if _record_stack:
        _record_stack[-1].append(record)
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
    category = _normalize_category(category)
    msg_text = str(message)
    module_name = module or "__main__"
    action = _action_for(msg_text, category, module_name, lineno)

    if action not in _VALID_ACTIONS:
        raise ValueError(f"invalid warnings action: {action!r}")
    if action == "error":
        raise category(message)
    reg = _resolve_registry(registry, module_globals)
    if _should_suppress(action, msg_text, category, lineno, reg):
        return None

    record = _WarningRecord(
        category(message),
        category,
        filename,
        lineno,
        module_name,
    )
    if _record_stack:
        _record_stack[-1].append(record)
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
    msg_pat = None
    mod_pat = None
    if message or module:
        try:
            import re

            msg_flags = getattr(re, "IGNORECASE", getattr(re, "I", 0))
            if message:
                msg_pat = re.compile(message, msg_flags)
            if module:
                mod_pat = re.compile(module)
        except Exception:
            if message:
                msg_pat = _SimpleRegex(message, True)
            if module:
                mod_pat = _SimpleRegex(module, False)
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
    filterwarnings(action, "", category, "", lineno, append=append)


def resetwarnings() -> None:
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


def catch_warnings(record: bool = False, module: Any | None = None) -> _CatchWarnings:
    return _CatchWarnings(record, module)
