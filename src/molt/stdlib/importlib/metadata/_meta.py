"""Intrinsic-backed protocol and typing metadata helpers."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import contextlib as _contextlib
import importlib.abc as _importlib_abc
import itertools as _itertools

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_IMPORTLIB_METADATA_TYPES_PAYLOAD = _require_intrinsic(
    "molt_importlib_metadata_types_payload", globals()
)


def _load_payload() -> dict[str, object]:
    payload = _MOLT_IMPORTLIB_METADATA_TYPES_PAYLOAD(
        __import__("typing"), _importlib_abc, _contextlib, _itertools
    )
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib.metadata types payload: dict expected")
    return payload


def _payload_get(payload: dict[str, object], name: str) -> object:
    if name not in payload:
        raise RuntimeError(f"invalid importlib.metadata types payload: missing {name}")
    return payload[name]


_PAYLOAD = _load_payload()

Any = _payload_get(_PAYLOAD, "Any")
Dict = _payload_get(_PAYLOAD, "Dict")
Iterator = _payload_get(_PAYLOAD, "Iterator")
List = _payload_get(_PAYLOAD, "List")
Optional = _payload_get(_PAYLOAD, "Optional")
Protocol = _payload_get(_PAYLOAD, "Protocol")
TypeVar = _payload_get(_PAYLOAD, "TypeVar")
Union = _payload_get(_PAYLOAD, "Union")
overload = _payload_get(_PAYLOAD, "overload")


class SimplePath(Protocol):
    pass


class PackageMetadata(Protocol):
    pass
