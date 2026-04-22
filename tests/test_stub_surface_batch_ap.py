from __future__ import annotations

import builtins
import importlib.util
import sys
import types
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MODULE_PATHS = [
    ROOT / "src/molt/stdlib/idlelib/__init__.py",
    ROOT / "src/molt/stdlib/idlelib/__main__.py",
    ROOT / "src/molt/stdlib/idlelib/autocomplete.py",
    ROOT / "src/molt/stdlib/idlelib/autocomplete_w.py",
    ROOT / "src/molt/stdlib/idlelib/autoexpand.py",
    ROOT / "src/molt/stdlib/idlelib/browser.py",
    ROOT / "src/molt/stdlib/idlelib/calltip.py",
    ROOT / "src/molt/stdlib/idlelib/calltip_w.py",
    ROOT / "src/molt/stdlib/idlelib/codecontext.py",
    ROOT / "src/molt/stdlib/idlelib/colorizer.py",
    ROOT / "src/molt/stdlib/idlelib/config.py",
    ROOT / "src/molt/stdlib/idlelib/config_key.py",
    ROOT / "src/molt/stdlib/idlelib/configdialog.py",
    ROOT / "src/molt/stdlib/idlelib/debugger.py",
    ROOT / "src/molt/stdlib/idlelib/debugger_r.py",
    ROOT / "src/molt/stdlib/idlelib/debugobj.py",
    ROOT / "src/molt/stdlib/idlelib/debugobj_r.py",
    ROOT / "src/molt/stdlib/idlelib/delegator.py",
    ROOT / "src/molt/stdlib/idlelib/dynoption.py",
    ROOT / "src/molt/stdlib/idlelib/editor.py",
]


def _install_intrinsics() -> tuple[types.ModuleType | None, object]:
    previous_intrinsics_mod = sys.modules.get("_intrinsics")
    previous_builtins = getattr(builtins, "_molt_intrinsics", None)
    builtins._molt_intrinsics = {"molt_capabilities_has": lambda name: True}

    intrinsics_mod = types.ModuleType("_intrinsics")

    def require_intrinsic(name: str, namespace: dict[str, object] | None = None):
        value = builtins._molt_intrinsics.get(name)
        if value is None:
            raise RuntimeError(f"missing intrinsic: {name}")
        if namespace is not None:
            namespace[name] = value
        return value

    intrinsics_mod.require_intrinsic = require_intrinsic
    sys.modules["_intrinsics"] = intrinsics_mod
    return previous_intrinsics_mod, previous_builtins


def _restore_intrinsics(
    previous_intrinsics_mod: types.ModuleType | None, previous_builtins: object
) -> None:
    if previous_intrinsics_mod is None:
        sys.modules.pop("_intrinsics", None)
    else:
        sys.modules["_intrinsics"] = previous_intrinsics_mod
    if previous_builtins is None:
        del builtins._molt_intrinsics
    else:
        builtins._molt_intrinsics = previous_builtins


def _load_module(path: Path, index: int) -> types.ModuleType:
    module_name = f"_molt_test_stub_surface_batch_ap_{index}"
    spec = importlib.util.spec_from_file_location(module_name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_idlelib_stub_batch_hides_raw_capability_intrinsic() -> None:
    previous_intrinsics_mod, previous_builtins = _install_intrinsics()
    try:
        for index, path in enumerate(MODULE_PATHS):
            module = _load_module(path, index)
            assert "molt_capabilities_has" not in module.__dict__
            assert "_MOLT_CAPABILITIES_HAS" in module.__dict__
            try:
                getattr(module, "sentinel")
            except RuntimeError as exc:
                assert "only an intrinsic-first stub is available" in str(exc)
            else:
                raise AssertionError(
                    f"{path} did not raise RuntimeError from __getattr__"
                )
    finally:
        _restore_intrinsics(previous_intrinsics_mod, previous_builtins)
