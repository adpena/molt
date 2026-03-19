from __future__ import annotations

import builtins
import importlib.util
import sys
import types
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"


def _load_module(name: str):
    for key in list(sys.modules):
        if key == name or key.startswith(f"{name}."):
            sys.modules.pop(key, None)

    registry = getattr(builtins, "_molt_intrinsics", None)
    if not isinstance(registry, dict):
        registry = {}
        setattr(builtins, "_molt_intrinsics", registry)
    registry["molt_capabilities_has"] = lambda _name=None: True

    intrinsics_mod = types.ModuleType("_intrinsics")

    def _require_intrinsic(name: str, namespace=None):
        intrinsics = getattr(builtins, "_molt_intrinsics", {})
        if name in intrinsics:
            value = intrinsics[name]
            if namespace is not None:
                namespace[name] = value
            return value
        raise RuntimeError(f"intrinsic unavailable: {name}")

    intrinsics_mod.require_intrinsic = _require_intrinsic
    sys.modules["_intrinsics"] = intrinsics_mod

    spec = importlib.util.spec_from_file_location(name, STDLIB_ROOT / "_locale.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def test_locale_private_module_surface() -> None:
    module = _load_module("molt_test__locale")
    assert "molt_capabilities_has" not in module.__dict__
    try:
        module.__getattr__("missing")
    except RuntimeError as exc:
        assert (
            str(exc)
            == 'stdlib module "_locale" is not fully lowered yet; only an intrinsic-first stub is available.'
        )
    else:
        raise AssertionError("expected RuntimeError")
