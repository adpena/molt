"""Minimal inspection helpers for Molt."""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic


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
    "iscoroutine",
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
_MOLT_INSPECT_CLEANDOC = _require_intrinsic("molt_inspect_cleandoc", globals())
_MOLT_INSPECT_CURRENTFRAME = _require_intrinsic("molt_inspect_currentframe", globals())
_MOLT_INSPECT_GETDOC = _require_intrinsic("molt_inspect_getdoc", globals())
_MOLT_INSPECT_ISFUNCTION = _require_intrinsic("molt_inspect_isfunction", globals())
_MOLT_INSPECT_ISCLASS = _require_intrinsic("molt_inspect_isclass", globals())
_MOLT_INSPECT_ISMODULE = _require_intrinsic("molt_inspect_ismodule", globals())
_MOLT_INSPECT_ISCOROUTINE = _require_intrinsic("molt_inspect_iscoroutine", globals())
_MOLT_INSPECT_ISCOROUTINEFUNCTION = _require_intrinsic(
    "molt_inspect_iscoroutinefunction", globals()
)
_MOLT_INSPECT_ISASYNCGENFUNCTION = _require_intrinsic(
    "molt_inspect_isasyncgenfunction", globals()
)
_MOLT_INSPECT_ISGENERATORFUNCTION = _require_intrinsic(
    "molt_inspect_isgeneratorfunction", globals()
)
_MOLT_INSPECT_ISAWAITABLE = _require_intrinsic("molt_inspect_isawaitable", globals())
_MOLT_INSPECT_GETGENERATORSTATE = _require_intrinsic(
    "molt_inspect_getgeneratorstate", globals()
)
_MOLT_INSPECT_GETASYNCGENSTATE = _require_intrinsic(
    "molt_inspect_getasyncgenstate", globals()
)
_MOLT_INSPECT_GETCOROUTINESTATE = _require_intrinsic(
    "molt_inspect_getcoroutinestate", globals()
)
_MOLT_INSPECT_SIGNATURE_DATA = _require_intrinsic(
    "molt_inspect_signature_data", globals()
)


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
            if param.kind == Parameter.VAR_POSITIONAL:
                saw_kwonly = True
            if param.kind == Parameter.KEYWORD_ONLY and not saw_kwonly:
                parts.append("*")
                saw_kwonly = True
            parts.append(str(param))
        if saw_posonly and not posonly_done:
            parts.append("/")
        return f"({', '.join(parts)})"


def cleandoc(doc: str | None) -> str:
    return _MOLT_INSPECT_CLEANDOC(doc)


def currentframe() -> object | None:
    return _MOLT_INSPECT_CURRENTFRAME()


def getdoc(obj: Any) -> str | None:
    return _MOLT_INSPECT_GETDOC(obj)


def isfunction(obj: Any) -> bool:
    return _MOLT_INSPECT_ISFUNCTION(obj)


def isclass(obj: Any) -> bool:
    return _MOLT_INSPECT_ISCLASS(obj)


def ismodule(obj: Any) -> bool:
    return _MOLT_INSPECT_ISMODULE(obj)


def iscoroutine(obj: Any) -> bool:
    return _MOLT_INSPECT_ISCOROUTINE(obj)


def iscoroutinefunction(obj: Any) -> bool:
    return _MOLT_INSPECT_ISCOROUTINEFUNCTION(obj)


def isasyncgenfunction(obj: Any) -> bool:
    return _MOLT_INSPECT_ISASYNCGENFUNCTION(obj)


def isgeneratorfunction(obj: Any) -> bool:
    return _MOLT_INSPECT_ISGENERATORFUNCTION(obj)


def isawaitable(obj: Any) -> bool:
    return _MOLT_INSPECT_ISAWAITABLE(obj)


def getgeneratorstate(gen: Any) -> str:
    return _MOLT_INSPECT_GETGENERATORSTATE(gen)


def getasyncgenstate(agen: Any) -> str:
    return _MOLT_INSPECT_GETASYNCGENSTATE(agen)


def getgeneratorlocals(gen: Any) -> dict[str, Any]:
    return _MOLT_GEN_LOCALS(gen)


def getasyncgenlocals(agen: Any) -> dict[str, Any]:
    return _MOLT_ASYNCGEN_LOCALS(agen)


def getcoroutinestate(coro: Any) -> str:
    return _MOLT_INSPECT_GETCOROUTINESTATE(coro)


def _signature_from_intrinsic(obj: Any) -> Signature | None:
    payload = _MOLT_INSPECT_SIGNATURE_DATA(obj)
    if payload is None:
        return None
    (
        arg_names,
        posonly,
        kwonly_names,
        vararg,
        varkw,
        defaults,
        kwdefaults,
    ) = payload
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


def signature(obj: Any) -> Signature:
    sig = getattr(obj, "__signature__", None)
    if isinstance(sig, Signature):
        return sig
    if sig is not None:
        return sig
    intrinsic_sig = _signature_from_intrinsic(obj)
    if intrinsic_sig is not None:
        return intrinsic_sig
    raise TypeError("inspect.signature cannot introspect this object")
