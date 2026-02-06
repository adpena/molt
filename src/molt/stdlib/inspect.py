"""Minimal inspection helpers for Molt."""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

import sys as _sys


__all__ = [
    "AGEN_CLOSED",
    "AGEN_CREATED",
    "AGEN_RUNNING",
    "AGEN_SUSPENDED",
    "CORO_CLOSED",
    "CORO_CREATED",
    "CORO_RUNNING",
    "CORO_SUSPENDED",
    "GEN_CLOSED",
    "GEN_CREATED",
    "GEN_RUNNING",
    "GEN_SUSPENDED",
    "Parameter",
    "Signature",
    "cleandoc",
    "currentframe",
    "getdoc",
    "getgeneratorlocals",
    "getasyncgenlocals",
    "getasyncgenstate",
    "getcoroutinestate",
    "getgeneratorstate",
    "isclass",
    "isawaitable",
    "isfunction",
    "ismodule",
    "iscoroutinefunction",
    "isasyncgenfunction",
    "isgeneratorfunction",
    "signature",
]

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): add full signature/introspection parity.

GEN_CREATED = "GEN_CREATED"
GEN_RUNNING = "GEN_RUNNING"
GEN_SUSPENDED = "GEN_SUSPENDED"
GEN_CLOSED = "GEN_CLOSED"

AGEN_CREATED = "AGEN_CREATED"
AGEN_RUNNING = "AGEN_RUNNING"
AGEN_SUSPENDED = "AGEN_SUSPENDED"
AGEN_CLOSED = "AGEN_CLOSED"

CORO_CREATED = "CORO_CREATED"
CORO_RUNNING = "CORO_RUNNING"
CORO_SUSPENDED = "CORO_SUSPENDED"
CORO_CLOSED = "CORO_CLOSED"

_MOLT_ASYNCGEN_LOCALS = _require_intrinsic("molt_asyncgen_locals", globals())
_MOLT_GEN_LOCALS = _require_intrinsic("molt_gen_locals", globals())


class _Empty:
    pass


_empty = _Empty()

POSITIONAL_ONLY = 0
POSITIONAL_OR_KEYWORD = 1
VAR_POSITIONAL = 2
KEYWORD_ONLY = 3
VAR_KEYWORD = 4


class Parameter:
    POSITIONAL_ONLY = POSITIONAL_ONLY
    POSITIONAL_OR_KEYWORD = POSITIONAL_OR_KEYWORD
    VAR_POSITIONAL = VAR_POSITIONAL
    KEYWORD_ONLY = KEYWORD_ONLY
    VAR_KEYWORD = VAR_KEYWORD

    def __init__(
        self,
        name: str,
        kind: int = POSITIONAL_OR_KEYWORD,
        default: Any = _empty,
        annotation: Any = _empty,
    ) -> None:
        if not name:
            raise ValueError("parameter name must be non-empty")
        self.name = name
        self.kind = kind
        self.default = default
        self.annotation = annotation

    def __repr__(self) -> str:
        return f"<Parameter {self}>"

    def __str__(self) -> str:
        prefix = ""
        if self.kind == self.VAR_POSITIONAL:
            prefix = "*"
        elif self.kind == self.VAR_KEYWORD:
            prefix = "**"
        out = f"{prefix}{self.name}"
        if self.default is not _empty and self.kind not in {
            self.VAR_POSITIONAL,
            self.VAR_KEYWORD,
        }:
            out = f"{out}={self.default!r}"
        return out


class Signature:
    def __init__(
        self, parameters: list[Parameter], return_annotation: Any = _empty
    ) -> None:
        self._parameters: dict[str, Parameter] = {}
        for param in parameters:
            self._parameters[param.name] = param
        self._order = list(parameters)
        self.return_annotation = return_annotation

    @property
    def parameters(self) -> dict[str, Parameter]:
        return self._parameters

    def __repr__(self) -> str:
        return f"<Signature {self}>"

    def __str__(self) -> str:
        parts: list[str] = []
        saw_posonly = False
        posonly_done = False
        saw_kwonly = False
        for param in self._order:
            if param.kind == Parameter.POSITIONAL_ONLY:
                saw_posonly = True
            elif saw_posonly and not posonly_done:
                parts.append("/")
                posonly_done = True
            if param.kind == Parameter.KEYWORD_ONLY and not saw_kwonly:
                parts.append("*")
                saw_kwonly = True
            parts.append(str(param))
        if saw_posonly and not posonly_done:
            parts.append("/")
        return f"({', '.join(parts)})"


def cleandoc(doc: str | None) -> str:
    if not doc:
        return ""
    lines = _expandtabs(doc).splitlines()
    while lines and not lines[0].strip():
        lines.pop(0)
    while lines and not lines[-1].strip():
        lines.pop()
    if not lines:
        return ""
    indent = None
    for line in lines:
        stripped = line.lstrip()
        if not stripped:
            continue
        margin = len(line) - len(stripped)
        if indent is None or margin < indent:
            indent = margin
    if indent is None:
        indent = 0
    trimmed = [line[indent:] for line in lines]
    return "\n".join(trimmed)


def _expandtabs(text: str, tabsize: int = 8) -> str:
    parts: list[str] = []
    col = 0
    for ch in text:
        if ch == "\t":
            spaces = tabsize - (col % tabsize)
            parts.append(" " * spaces)
            col += spaces
        elif ch in "\r\n":
            parts.append(ch)
            col = 0
        else:
            parts.append(ch)
            col += 1
    return "".join(parts)


def currentframe() -> object | None:
    if hasattr(_sys, "_getframe"):
        try:
            return _sys._getframe(1)
        except Exception:
            return None
    return None


def getdoc(obj: Any) -> str | None:
    doc = getattr(obj, "__doc__", None)
    if doc is None:
        return None
    return cleandoc(doc)


def isfunction(obj: Any) -> bool:
    return hasattr(obj, "__code__") or hasattr(obj, "__molt_arg_names__")


def isclass(obj: Any) -> bool:
    return hasattr(obj, "__mro__")


def ismodule(obj: Any) -> bool:
    return hasattr(obj, "__dict__") and hasattr(obj, "__name__")


def iscoroutinefunction(obj: Any) -> bool:
    if getattr(obj, "__molt_is_coroutine__", False):
        return True
    flags = getattr(getattr(obj, "__code__", None), "co_flags", 0)
    return bool(flags & 0x80)


def isasyncgenfunction(obj: Any) -> bool:
    if getattr(obj, "__molt_is_async_generator__", False):
        return True
    flags = getattr(getattr(obj, "__code__", None), "co_flags", 0)
    return bool(flags & 0x200)


def isgeneratorfunction(obj: Any) -> bool:
    if getattr(obj, "__molt_is_generator__", False):
        return True
    flags = getattr(getattr(obj, "__code__", None), "co_flags", 0)
    return bool(flags & 0x20)


def isawaitable(obj: Any) -> bool:
    if getattr(obj, "__molt_is_coroutine__", False):
        return True
    if hasattr(obj, "__await__"):
        return True
    flags = getattr(getattr(obj, "gi_code", None), "co_flags", 0)
    return bool(flags & 0x100)


def getgeneratorstate(gen: Any) -> str:
    if getattr(gen, "gi_running", False):
        return GEN_RUNNING
    frame = getattr(gen, "gi_frame", None)
    if frame is None:
        return GEN_CLOSED
    lasti = getattr(frame, "f_lasti", -1)
    if lasti == -1:
        return GEN_CREATED
    return GEN_SUSPENDED


def getasyncgenstate(agen: Any) -> str:
    if getattr(agen, "ag_running", False):
        return AGEN_RUNNING
    frame = getattr(agen, "ag_frame", None)
    if frame is None:
        return AGEN_CLOSED
    lasti = getattr(frame, "f_lasti", -1)
    if lasti == -1:
        return AGEN_CREATED
    return AGEN_SUSPENDED


def getgeneratorlocals(gen: Any) -> dict[str, Any]:
    if callable(_MOLT_GEN_LOCALS):
        return _MOLT_GEN_LOCALS(gen)
    if not hasattr(gen, "gi_frame"):
        raise TypeError("expected generator")
    frame = getattr(gen, "gi_frame", None)
    if frame is None:
        return {}
    return getattr(frame, "f_locals", {}) or {}


def getasyncgenlocals(agen: Any) -> dict[str, Any]:
    if callable(_MOLT_ASYNCGEN_LOCALS):
        return _MOLT_ASYNCGEN_LOCALS(agen)
    if not hasattr(agen, "ag_frame"):
        raise TypeError("expected async generator")
    frame = getattr(agen, "ag_frame", None)
    if frame is None:
        return {}
    return getattr(frame, "f_locals", {}) or {}


def getcoroutinestate(coro: Any) -> str:
    if getattr(coro, "cr_running", False):
        return CORO_RUNNING
    frame = getattr(coro, "cr_frame", None)
    if frame is None:
        if getattr(coro, "gi_running", False):
            return CORO_RUNNING
        frame = getattr(coro, "gi_frame", None)
    if frame is None:
        return CORO_CLOSED
    lasti = getattr(frame, "f_lasti", -1)
    if lasti == -1:
        return CORO_CREATED
    return CORO_SUSPENDED


def _signature_from_molt(obj: Any) -> Signature | None:
    arg_names = getattr(obj, "__molt_arg_names__", None)
    if arg_names is None:
        return None
    posonly = getattr(obj, "__molt_posonly__", 0) or 0
    kwonly_names = getattr(obj, "__molt_kwonly_names__", None) or ()
    vararg = getattr(obj, "__molt_vararg__", None)
    varkw = getattr(obj, "__molt_varkw__", None)
    defaults = getattr(obj, "__defaults__", None) or ()
    kwdefaults = getattr(obj, "__kwdefaults__", None)
    if kwdefaults is None:
        kwdefaults = {}
    params: list[Parameter] = []
    default_start = len(arg_names) - len(defaults)
    for idx, name in enumerate(arg_names):
        default = _empty
        if idx >= default_start:
            default = defaults[idx - default_start]
        kind = (
            Parameter.POSITIONAL_ONLY
            if idx < posonly
            else Parameter.POSITIONAL_OR_KEYWORD
        )
        params.append(Parameter(name, kind, default))
    if vararg:
        params.append(Parameter(vararg, Parameter.VAR_POSITIONAL, _empty))
    for name in kwonly_names:
        default = kwdefaults.get(name, _empty)
        params.append(Parameter(name, Parameter.KEYWORD_ONLY, default))
    if varkw:
        params.append(Parameter(varkw, Parameter.VAR_KEYWORD, _empty))
    return Signature(params)


def _signature_from_code(obj: Any) -> Signature | None:
    code = getattr(obj, "__code__", None)
    if code is None:
        return None
    posonly = getattr(code, "co_posonlyargcount", 0)
    argcount = getattr(code, "co_argcount", 0)
    kwonly = getattr(code, "co_kwonlyargcount", 0)
    varnames = list(getattr(code, "co_varnames", ()))
    defaults = getattr(obj, "__defaults__", ()) or ()
    kwdefaults = getattr(obj, "__kwdefaults__", {}) or {}
    flags = getattr(code, "co_flags", 0)

    params: list[Parameter] = []
    total_pos = argcount
    pos_names = varnames[:total_pos]
    pos_defaults_start = total_pos - len(defaults)
    idx = 0
    for name in pos_names:
        if idx < posonly:
            kind = Parameter.POSITIONAL_ONLY
        else:
            kind = Parameter.POSITIONAL_OR_KEYWORD
        default = _empty
        if idx >= pos_defaults_start:
            default = defaults[idx - pos_defaults_start]
        params.append(Parameter(name, kind, default))
        idx += 1

    var_pos = bool(flags & 0x04)
    var_kw = bool(flags & 0x08)
    offset = total_pos
    if var_pos:
        params.append(Parameter(varnames[offset], Parameter.VAR_POSITIONAL))
        offset += 1

    kw_names = varnames[offset : offset + kwonly]
    for name in kw_names:
        default = kwdefaults.get(name, _empty)
        params.append(Parameter(name, Parameter.KEYWORD_ONLY, default))
    offset += kwonly

    if var_kw:
        params.append(Parameter(varnames[offset], Parameter.VAR_KEYWORD))

    return Signature(params)


def signature(obj: Any) -> Signature:
    sig = getattr(obj, "__signature__", None)
    if isinstance(sig, Signature):
        return sig
    if sig is not None:
        return sig
    molt_sig = _signature_from_molt(obj)
    if molt_sig is not None:
        return molt_sig
    code_sig = _signature_from_code(obj)
    if code_sig is not None:
        return code_sig
    raise TypeError("inspect.signature cannot introspect this object")
