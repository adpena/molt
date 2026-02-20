"""Compatibility surface for CPython `_frozen_importlib`."""

from _intrinsics import require_intrinsic as _require_intrinsic

import importlib.machinery as _machinery
import importlib.util as _util
import sys

_require_intrinsic("molt_capabilities_has", globals())


def _fallback_type(name: str):
    return type(name, (), {})


BuiltinImporter = getattr(_machinery, "BuiltinImporter", _fallback_type("BuiltinImporter"))
FrozenImporter = getattr(_machinery, "FrozenImporter", _fallback_type("FrozenImporter"))
ModuleSpec = getattr(_machinery, "ModuleSpec", _fallback_type("ModuleSpec"))
module_from_spec = getattr(_util, "module_from_spec")
spec_from_loader = getattr(_util, "spec_from_loader")

__all__ = [
    "BuiltinImporter",
    "FrozenImporter",
    "ModuleSpec",
    "module_from_spec",
    "spec_from_loader",
    "sys",
]
