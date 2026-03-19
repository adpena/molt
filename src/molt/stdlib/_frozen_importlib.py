"""Compatibility surface for CPython `_frozen_importlib`."""

from _intrinsics import require_intrinsic as _require_intrinsic

import importlib.machinery as _machinery
import importlib.util as _util
import sys

_require_intrinsic("molt_capabilities_has")
_MOLT_IMPORTLIB_FROZEN_PAYLOAD = _require_intrinsic("molt_importlib_frozen_payload")


def _load_payload() -> dict[str, object]:
    payload = _MOLT_IMPORTLIB_FROZEN_PAYLOAD(_machinery, _util)
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib frozen payload: dict expected")
    return payload


def _payload_get(payload: dict[str, object], name: str):
    if name not in payload:
        raise RuntimeError(f"invalid importlib frozen payload: missing {name}")
    return payload[name]


_PAYLOAD = _load_payload()
BuiltinImporter = _payload_get(_PAYLOAD, "BuiltinImporter")
FrozenImporter = _payload_get(_PAYLOAD, "FrozenImporter")
ModuleSpec = _payload_get(_PAYLOAD, "ModuleSpec")
module_from_spec = _payload_get(_PAYLOAD, "module_from_spec")
spec_from_loader = _payload_get(_PAYLOAD, "spec_from_loader")

__all__ = [
    "BuiltinImporter",
    "FrozenImporter",
    "ModuleSpec",
    "module_from_spec",
    "spec_from_loader",
    "sys",
]


globals().pop("_require_intrinsic", None)
