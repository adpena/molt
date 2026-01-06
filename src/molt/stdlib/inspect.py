"""Minimal inspection helpers for Molt."""

from __future__ import annotations

from typing import Any

__all__ = [
    "Parameter",
    "Signature",
    "cleandoc",
    "getdoc",
    "isclass",
    "isfunction",
    "ismodule",
    "iscoroutinefunction",
    "isgeneratorfunction",
    "signature",
]

# TODO(stdlib-compat, owner:stdlib, milestone:SL3): add full signature/introspection parity.


class _Empty:
    pass


_empty = _Empty()


class Parameter:
    POSITIONAL_ONLY = 0
    POSITIONAL_OR_KEYWORD = 1
    VAR_POSITIONAL = 2
    KEYWORD_ONLY = 3
    VAR_KEYWORD = 4

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
        self._parameters = {param.name: param for param in parameters}
        self._order = list(parameters)
        self.return_annotation = return_annotation

    @property
    def parameters(self) -> dict[str, Parameter]:
        return self._parameters

    def __repr__(self) -> str:
        return f"<Signature {self}>"

    def __str__(self) -> str:
        parts: list[str] = []
        saw_kwonly = False
        for param in self._order:
            if param.kind == Parameter.KEYWORD_ONLY and not saw_kwonly:
                parts.append("*")
                saw_kwonly = True
            parts.append(str(param))
        return f"({', '.join(parts)})"


def cleandoc(doc: str | None) -> str:
    if not doc:
        return ""
    lines = doc.expandtabs().splitlines()
    while lines and not lines[0].strip():
        lines.pop(0)
    while lines and not lines[-1].strip():
        lines.pop()
    if not lines:
        return ""
    indent = None
    for line in lines[1:]:
        stripped = line.lstrip()
        if not stripped:
            continue
        margin = len(line) - len(stripped)
        if indent is None or margin < indent:
            indent = margin
    if indent is None:
        indent = 0
    trimmed = [lines[0].strip()]
    for line in lines[1:]:
        trimmed.append(line[indent:])
    return "\n".join(trimmed)


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


def isgeneratorfunction(obj: Any) -> bool:
    if getattr(obj, "__molt_is_generator__", False):
        return True
    flags = getattr(getattr(obj, "__code__", None), "co_flags", 0)
    return bool(flags & 0x20)


def _signature_from_molt(obj: Any) -> Signature | None:
    arg_names = getattr(obj, "__molt_arg_names__", None)
    if arg_names is None:
        return None
    defaults = getattr(obj, "__defaults__", None)
    if defaults is None:
        defaults = ()
    params: list[Parameter] = []
    default_start = len(arg_names) - len(defaults)
    for idx, name in enumerate(arg_names):
        default = _empty
        if idx >= default_start:
            default = defaults[idx - default_start]
        params.append(Parameter(name, default=default))
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
    for idx, name in enumerate(pos_names):
        kind = (
            Parameter.POSITIONAL_ONLY
            if idx < posonly
            else Parameter.POSITIONAL_OR_KEYWORD
        )
        default = _empty
        if idx >= pos_defaults_start:
            default = defaults[idx - pos_defaults_start]
        params.append(Parameter(name, kind=kind, default=default))

    var_pos = bool(flags & 0x04)
    var_kw = bool(flags & 0x08)
    offset = total_pos
    if var_pos:
        params.append(Parameter(varnames[offset], kind=Parameter.VAR_POSITIONAL))
        offset += 1

    kw_names = varnames[offset : offset + kwonly]
    for name in kw_names:
        default = kwdefaults.get(name, _empty)
        params.append(Parameter(name, kind=Parameter.KEYWORD_ONLY, default=default))
    offset += kwonly

    if var_kw:
        params.append(Parameter(varnames[offset], kind=Parameter.VAR_KEYWORD))

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
