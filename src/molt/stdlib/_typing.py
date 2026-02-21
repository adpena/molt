"""Compatibility surface for CPython `_typing`."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_TYPING_PRIVATE_PAYLOAD = _require_intrinsic(
    "molt_typing_private_payload", globals()
)


def _load_payload() -> dict[str, object]:
    typing_module = __import__("typing")
    payload = _MOLT_TYPING_PRIVATE_PAYLOAD(typing_module)
    if not isinstance(payload, dict):
        raise RuntimeError("invalid _typing payload: dict expected")
    return payload


def _payload_get(payload: dict[str, object], name: str) -> object:
    if name not in payload:
        raise RuntimeError(f"invalid _typing payload: missing {name}")
    return payload[name]


_PAYLOAD = _load_payload()

Generic = _payload_get(_PAYLOAD, "Generic")
ParamSpec = _payload_get(_PAYLOAD, "ParamSpec")
ParamSpecArgs = _payload_get(_PAYLOAD, "ParamSpecArgs")
ParamSpecKwargs = _payload_get(_PAYLOAD, "ParamSpecKwargs")
TypeAliasType = _payload_get(_PAYLOAD, "TypeAliasType")
TypeVar = _payload_get(_PAYLOAD, "TypeVar")
TypeVarTuple = _payload_get(_PAYLOAD, "TypeVarTuple")

__all__ = [
    "Generic",
    "ParamSpec",
    "ParamSpecArgs",
    "ParamSpecKwargs",
    "TypeAliasType",
    "TypeVar",
    "TypeVarTuple",
]
