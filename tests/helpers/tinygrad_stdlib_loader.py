from __future__ import annotations

from contextlib import contextmanager
import importlib.util
from pathlib import Path
import sys
import types


ROOT = Path(__file__).resolve().parents[2]
TINYGRAD_STDLIB = ROOT / "src" / "molt" / "stdlib" / "tinygrad"


def _load_module(module_name: str, path: Path):
    if path.is_dir():
        spec = importlib.util.spec_from_file_location(
            module_name,
            path / "__init__.py",
            submodule_search_locations=[str(path)],
        )
    else:
        spec = importlib.util.spec_from_file_location(module_name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


@contextmanager
def tinygrad_stdlib_context(*extra_leaves: str):
    leaves = tuple(
        dict.fromkeys(("dtypes", "lazy", "realize", "tensor", *extra_leaves))
    )
    module_names = (
        "_intrinsics",
        "tinygrad",
        *(f"tinygrad.{leaf}" for leaf in leaves),
    )
    sentinel = object()
    saved = {name: sys.modules.get(name, sentinel) for name in module_names}

    try:
        intrinsics = types.ModuleType("_intrinsics")
        intrinsics.require_intrinsic = lambda _name: lambda *args, **kwargs: None
        sys.modules["_intrinsics"] = intrinsics

        package = types.ModuleType("tinygrad")
        package.__path__ = [str(TINYGRAD_STDLIB)]
        sys.modules["tinygrad"] = package

        modules = {}
        for leaf in leaves:
            module_path = TINYGRAD_STDLIB / f"{leaf}.py"
            if not module_path.exists():
                module_path = TINYGRAD_STDLIB / leaf
            module = _load_module(f"tinygrad.{leaf}", module_path)
            setattr(package, leaf, module)
            modules[leaf] = module

        yield modules
    finally:
        for name, module in saved.items():
            if module is sentinel:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = module
