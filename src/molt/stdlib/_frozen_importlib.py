"""Compatibility surface for CPython `_frozen_importlib`."""

from _intrinsics import require_intrinsic as _require_intrinsic

import sys

_machinery = __import__("importlib.machinery", globals(), locals(), ("ModuleSpec",), 0)

_require_intrinsic("molt_capabilities_has")
_MOLT_IMPORTLIB_FROZEN_PAYLOAD = _require_intrinsic("molt_importlib_frozen_payload")
_MOLT_IMPORTLIB_MODULE_FROM_SPEC = _require_intrinsic("molt_importlib_module_from_spec")
_MOLT_IMPORTLIB_SPEC_FROM_LOADER = _require_intrinsic("molt_importlib_spec_from_loader")


def _load_payload() -> dict[str, object]:
    payload = _MOLT_IMPORTLIB_FROZEN_PAYLOAD(_machinery, None)
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
module_from_spec = _MOLT_IMPORTLIB_MODULE_FROM_SPEC
spec_from_loader = _MOLT_IMPORTLIB_SPEC_FROM_LOADER

__all__ = [
    "BuiltinImporter",
    "FrozenImporter",
    "ModuleSpec",
    "module_from_spec",
    "spec_from_loader",
    "sys",
]


globals().pop("_require_intrinsic", None)
