from __future__ import annotations

import builtins
import importlib.util
import sys
import types
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MODULE_PATHS = [
    ROOT / "src/molt/stdlib/idlelib/filelist.py",
    ROOT / "src/molt/stdlib/idlelib/format.py",
    ROOT / "src/molt/stdlib/idlelib/grep.py",
    ROOT / "src/molt/stdlib/idlelib/help.py",
    ROOT / "src/molt/stdlib/idlelib/help_about.py",
    ROOT / "src/molt/stdlib/idlelib/history.py",
    ROOT / "src/molt/stdlib/idlelib/hyperparser.py",
    ROOT / "src/molt/stdlib/idlelib/idle.py",
    ROOT / "src/molt/stdlib/idlelib/iomenu.py",
    ROOT / "src/molt/stdlib/idlelib/macosx.py",
    ROOT / "src/molt/stdlib/idlelib/mainmenu.py",
    ROOT / "src/molt/stdlib/idlelib/multicall.py",
    ROOT / "src/molt/stdlib/idlelib/outwin.py",
    ROOT / "src/molt/stdlib/idlelib/parenmatch.py",
    ROOT / "src/molt/stdlib/idlelib/pathbrowser.py",
    ROOT / "src/molt/stdlib/idlelib/percolator.py",
    ROOT / "src/molt/stdlib/idlelib/pyparse.py",
    ROOT / "src/molt/stdlib/idlelib/pyshell.py",
    ROOT / "src/molt/stdlib/idlelib/query.py",
    ROOT / "src/molt/stdlib/idlelib/redirector.py",
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
    module_name = f"_molt_test_stub_surface_batch_aq_{index}"
    spec = importlib.util.spec_from_file_location(module_name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_idlelib_second_stub_batch_hides_raw_capability_intrinsic() -> None:
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
