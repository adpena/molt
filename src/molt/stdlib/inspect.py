# Parameter.replace and Signature.replace adapted from CPython 3.13/3.14 (PSF-2.0).
# Copyright Python Software Foundation; see collections/LICENSE.cpython.
# Shim churn audit: 16 intrinsic-direct / 23 total exports
"""Minimal inspection helpers for Molt.

Pure-forwarding shims eliminated per MOL-215 where argument signatures permit.
"""

from __future__ import annotations

from typing import Any
from functools import partial as _partial
import collections.abc as _collections_abc
import types as _types
import sys as _sys
from keyword import iskeyword as _iskeyword

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
    "getmembers",
    "getgeneratorlocals",
    "getasyncgenlocals",
    "getcoroutinelocals",
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
    "isgenerator",
    "isgeneratorfunction",
    "markcoroutinefunction",
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
_molt_getmembers = _require_intrinsic("molt_inspect_getmembers")
_molt_isfunction = _require_intrinsic("molt_inspect_isfunction")
_molt_is_bound_method = _require_intrinsic("molt_is_bound_method")
_molt_isclass = _require_intrinsic("molt_inspect_isclass")
_molt_ismodule = _require_intrinsic("molt_inspect_ismodule")
_molt_iscoroutine = _require_intrinsic("molt_inspect_iscoroutine")
_molt_iscoroutinefunction = _require_intrinsic("molt_inspect_iscoroutinefunction")
_molt_isasyncgenfunction = _require_intrinsic("molt_inspect_isasyncgenfunction")
_molt_isgeneratorfunction = _require_intrinsic("molt_inspect_isgeneratorfunction")
_molt_isnativeawaitable = _require_intrinsic("molt_inspect_isnativeawaitable")
_molt_getgeneratorstate = _require_intrinsic("molt_inspect_getgeneratorstate")
_molt_getasyncgenstate = _require_intrinsic("molt_inspect_getasyncgenstate")
_molt_getcoroutinestate = _require_intrinsic("molt_inspect_getcoroutinestate")
_molt_getgeneratorlocals = _require_intrinsic("molt_gen_locals")
_molt_getasyncgenlocals = _require_intrinsic("molt_asyncgen_locals")

# This explicit opt-in protocol is not the compiled code's execution kind.
# Identity (not truthiness) prevents arbitrary user attributes from opting in.
_is_coroutine_mark = object()
_partial_type = type(_partial(bool))


def _unwrap_callable_kind(obj):
    while True:
        if _molt_is_bound_method(obj):
            obj = obj.__func__
        elif isinstance(obj, _partial_type):
            obj = obj.func
        else:
            return obj


def cleandoc(doc):
    return _molt_cleandoc(doc)


def currentframe():
    return _molt_currentframe()


def getdoc(obj):
    return _molt_getdoc(obj)


def getmembers(obj, predicate=None):
    return _molt_getmembers(obj, predicate)


def isfunction(obj):
    return _molt_isfunction(obj)


def isclass(obj):
    return _molt_isclass(obj)


def ismodule(obj):
    return _molt_ismodule(obj)


def iscoroutine(obj):
    return _molt_iscoroutine(obj)


def iscoroutinefunction(obj):
    obj = _unwrap_callable_kind(obj)
    return _molt_iscoroutinefunction(obj) or (
        getattr(obj, "_is_coroutine_marker", None) is _is_coroutine_mark
    )


def markcoroutinefunction(func):
    func = getattr(func, "__func__", func)
    func._is_coroutine_marker = _is_coroutine_mark
    return func


def isasyncgenfunction(obj):
    return _molt_isasyncgenfunction(_unwrap_callable_kind(obj))


def isgenerator(obj):
    return isinstance(obj, _types.GeneratorType)


def isgeneratorfunction(obj):
    return _molt_isgeneratorfunction(_unwrap_callable_kind(obj))


def isawaitable(obj):
    return _molt_isnativeawaitable(obj) or isinstance(obj, _collections_abc.Awaitable)


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


def getcoroutinelocals(coroutine):
    frame = getattr(coroutine, "cr_frame", None)
    if frame is not None:
        return frame.f_locals
    return {}


# --- Intrinsics used by retained wrappers ---

_MOLT_INSPECT_SIGNATURE_DATA = _require_intrinsic("molt_inspect_signature_data")


class _Empty:
    pass


_empty = _Empty()
_void = object()
_KIND_DESCRIPTIONS = ("positional-only", "positional or keyword", "variadic positional", "keyword-only", "variadic keyword")

POSITIONAL_ONLY = 0
POSITIONAL_OR_KEYWORD = 1
VAR_POSITIONAL = 2
KEYWORD_ONLY = 3
VAR_KEYWORD = 4


class Parameter:
    empty = _empty
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
        if kind not in (0, 1, 2, 3, 4):
            raise ValueError(f"value {kind!r} is not a valid Parameter.kind")
        kind = int(kind)
        if default is not _empty and kind in (VAR_POSITIONAL, VAR_KEYWORD):
            raise ValueError(f"{_KIND_DESCRIPTIONS[kind]} parameters cannot have default values")
        if name is _empty:
            raise ValueError("name is a required attribute for Parameter")
        if not isinstance(name, str):
            raise TypeError(f"name must be a str, not a {type(name).__name__}")
        if name.startswith(".") and name[1:].isdigit():
            if kind != POSITIONAL_OR_KEYWORD:
                raise ValueError(
                    "implicit arguments must be passed as positional or keyword arguments, "
                    f"not {_KIND_DESCRIPTIONS[kind]}"
                )
            kind = POSITIONAL_ONLY
            name = f"implicit{name[1:]}"
        if (_iskeyword(name) and kind != POSITIONAL_ONLY) or not name.isidentifier():
            raise ValueError(f"{name!r} is not a valid parameter name")
        self.name = name
        self.kind = kind
        self.default = default
        self.annotation = annotation

    # CPython v3.13.9/v3.14.0 Lib/inspect.py: Parameter.replace.
    def replace(self, *, name=_void, kind=_void, annotation=_void, default=_void):
        if name is _void:
            name = self.name
        if kind is _void:
            kind = self.kind
        if annotation is _void:
            annotation = self.annotation
        if default is _void:
            default = self.default
        return type(self)(name, kind, default=default, annotation=annotation)

    if _sys.version_info >= (3, 13):
        __replace__ = replace

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
    empty = _empty
    def __init__(
        self, parameters: list[Parameter] | None = None, return_annotation: Any = _empty
    ) -> None:
        self._parameters: dict[str, Parameter] = {}
        top_kind = POSITIONAL_ONLY
        seen_default = False
        for param in (() if parameters is None else parameters):
            kind = param.kind
            if kind < top_kind:
                raise ValueError(
                    f"wrong parameter order: {_KIND_DESCRIPTIONS[top_kind]} parameter "
                    f"before {_KIND_DESCRIPTIONS[kind]} parameter"
                )
            if kind > top_kind:
                top_kind = kind
            if kind in (POSITIONAL_ONLY, POSITIONAL_OR_KEYWORD):
                if param.default is _empty:
                    if seen_default:
                        raise ValueError("non-default argument follows default argument")
                else:
                    seen_default = True
            if param.name in self._parameters:
                raise ValueError(f"duplicate parameter name: {param.name!r}")
            self._parameters[param.name] = param
        self._order = list(self._parameters.values())
        self.return_annotation = return_annotation

    # CPython v3.13.9/v3.14.0 Lib/inspect.py: Signature.replace.
    def replace(self, *, parameters=_void, return_annotation=_void):
        if parameters is _void:
            parameters = self.parameters.values()
        if return_annotation is _void:
            return_annotation = self.return_annotation
        return type(self)(parameters, return_annotation=return_annotation)

    if _sys.version_info >= (3, 13):
        __replace__ = replace

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


def _signature_bind_bound_method(sig: Signature) -> Signature:
    ordered = list(sig.parameters.values())
    if not ordered or ordered[0].kind in (
        Parameter.KEYWORD_ONLY, Parameter.VAR_KEYWORD
    ):
        raise ValueError("invalid method signature")
    first = ordered[0]
    if first.kind not in (Parameter.POSITIONAL_ONLY, Parameter.POSITIONAL_OR_KEYWORD):
        return sig
    return Signature(ordered[1:], getattr(sig, "return_annotation", _empty))


def signature(obj: Any) -> Signature:
    # Managed methods forward their function's metadata. Resolve that function
    # before reading the forwarded attributes, then bind exactly once. Native
    # callable roles use their own receiver-aware Clinic signature in runtime.
    if _molt_is_bound_method(obj):
        return _signature_bind_bound_method(signature(obj.__func__))
    sig = getattr(obj, "__signature__", None)
    if sig is not None and not isinstance(sig, Signature):
        raise TypeError(f"unexpected object {sig!r} in __signature__ attribute")
    if isinstance(sig, Signature):
        return sig
    intrinsic_sig = _signature_from_intrinsic(obj)
    if intrinsic_sig is not None:
        return intrinsic_sig
    if not callable(obj):
        raise TypeError(f"{obj!r} is not a callable object")
    # CPython: for callable instances, delegate to their `__call__` method.
    if not isinstance(obj, type):
        call_attr = getattr(obj, "__call__", None)
        if call_attr is not None and call_attr is not obj:
            return signature(call_attr)
    if isinstance(obj, type):
        if obj is type:
            raise ValueError(f"no signature found for builtin {obj!r}")
        raise ValueError(f"no signature found for builtin type {obj!r}")
    raise ValueError(f"no signature found for {obj!r}")


globals().pop("_require_intrinsic", None)
