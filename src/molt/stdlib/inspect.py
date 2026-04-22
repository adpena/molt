# Shim churn audit: 16 intrinsic-direct / 23 total exports
"""Minimal inspection helpers for Molt.

Pure-forwarding shims eliminated per MOL-215 where argument signatures permit.
"""

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

# TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P3, status:planned): continue signature/introspection parity expansion.

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

# --- Intrinsic-backed functions ---
# Thin wrappers around runtime intrinsics so the compiler can emit
# direct ``CALL inspect__<name>`` symbols for cross-module callers.

_molt_cleandoc = _require_intrinsic("molt_inspect_cleandoc")
_molt_currentframe = _require_intrinsic("molt_inspect_currentframe")
_molt_getdoc = _require_intrinsic("molt_inspect_getdoc")
_molt_isfunction = _require_intrinsic("molt_inspect_isfunction")
_molt_isclass = _require_intrinsic("molt_inspect_isclass")
_molt_ismodule = _require_intrinsic("molt_inspect_ismodule")
_molt_iscoroutine = _require_intrinsic("molt_inspect_iscoroutine")
_molt_iscoroutinefunction = _require_intrinsic("molt_inspect_iscoroutinefunction")
_molt_isasyncgenfunction = _require_intrinsic("molt_inspect_isasyncgenfunction")
_molt_isgeneratorfunction = _require_intrinsic("molt_inspect_isgeneratorfunction")
_molt_isawaitable = _require_intrinsic("molt_inspect_isawaitable")
_molt_getgeneratorstate = _require_intrinsic("molt_inspect_getgeneratorstate")
_molt_getasyncgenstate = _require_intrinsic("molt_inspect_getasyncgenstate")
_molt_getcoroutinestate = _require_intrinsic("molt_inspect_getcoroutinestate")
_molt_getgeneratorlocals = _require_intrinsic("molt_gen_locals")
_molt_getasyncgenlocals = _require_intrinsic("molt_asyncgen_locals")


def cleandoc(doc):
    return _molt_cleandoc(doc)


def currentframe():
    return _molt_currentframe()


def getdoc(obj):
    return _molt_getdoc(obj)


def isfunction(obj):
    return _molt_isfunction(obj)


def isclass(obj):
    return _molt_isclass(obj)


def ismodule(obj):
    return _molt_ismodule(obj)


def iscoroutine(obj):
    return _molt_iscoroutine(obj)


def iscoroutinefunction(obj):
    return _molt_iscoroutinefunction(obj)


def isasyncgenfunction(obj):
    return _molt_isasyncgenfunction(obj)


def isgeneratorfunction(obj):
    return _molt_isgeneratorfunction(obj)


def isawaitable(obj):
    return _molt_isawaitable(obj)


def getgeneratorstate(gen):
    return _molt_getgeneratorstate(gen)


def getasyncgenstate(agen):
    return _molt_getasyncgenstate(agen)


def getcoroutinestate(coro):
    return _molt_getcoroutinestate(coro)


def getgeneratorlocals(gen):
    return _molt_getgeneratorlocals(gen)


def getasyncgenlocals(agen):
    return _molt_getasyncgenlocals(agen)


# --- Intrinsics used by retained wrappers ---

_MOLT_INSPECT_SIGNATURE_DATA = _require_intrinsic("molt_inspect_signature_data")


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


def _signature_bind_bound_method(sig: Signature, method_obj: Any) -> Signature:
    if getattr(method_obj, "__self__", None) is None:
        return sig
    ordered = list(sig.parameters.values())
    if not ordered:
        return sig
    first = ordered[0]
    if first.kind not in (Parameter.POSITIONAL_ONLY, Parameter.POSITIONAL_OR_KEYWORD):
        return sig
    return Signature(ordered[1:], getattr(sig, "return_annotation", _empty))


def signature(obj: Any) -> Signature:
    sig = getattr(obj, "__signature__", None)
    if sig is not None and not isinstance(sig, Signature):
        raise TypeError(f"unexpected object {sig!r} in __signature__ attribute")
    if isinstance(sig, Signature):
        return sig
    intrinsic_sig = _signature_from_intrinsic(obj)
    if intrinsic_sig is not None:
        return intrinsic_sig
    method_fn = getattr(obj, "__func__", None)
    if method_fn is not None:
        fn_sig = getattr(method_fn, "__signature__", None)
        if isinstance(fn_sig, Signature):
            return _signature_bind_bound_method(fn_sig, obj)
        if fn_sig is not None and not isinstance(fn_sig, str):
            return _signature_bind_bound_method(fn_sig, obj)
        intrinsic_fn_sig = _signature_from_intrinsic(method_fn)
        if intrinsic_fn_sig is not None:
            return _signature_bind_bound_method(intrinsic_fn_sig, obj)
    if not callable(obj):
        raise TypeError(f"{obj!r} is not a callable object")
    # CPython: for callable instances, delegate to their `__call__` method.
    if not isinstance(obj, type):
        call_attr = getattr(obj, "__call__", None)
        if call_attr is not None and call_attr is not obj:
            try:
                return signature(call_attr)
            except Exception:  # noqa: BLE001
                pass
    if isinstance(obj, type):
        if obj is type:
            raise ValueError(f"no signature found for builtin {obj!r}")
        raise ValueError(f"no signature found for builtin type {obj!r}")
    raise ValueError(f"no signature found for {obj!r}")


globals().pop("_require_intrinsic", None)
