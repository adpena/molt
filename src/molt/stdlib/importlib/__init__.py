"""Minimal import support for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


import os as _os

import importlib.machinery as machinery
import importlib.util as util

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_IMPORTLIB_RESOLVE_NAME = _require_intrinsic(
    "molt_importlib_resolve_name", globals()
)
_MOLT_IMPORTLIB_KNOWN_ABSENT_MISSING_NAME = _require_intrinsic(
    "molt_importlib_known_absent_missing_name", globals()
)
_MOLT_IMPORTLIB_IMPORT_MODULE = _require_intrinsic(
    "molt_importlib_import_module", globals()
)
_MOLT_IMPORTLIB_RUNTIME_MODULES = _require_intrinsic(
    "molt_importlib_runtime_modules", globals()
)
_MODULE_ALIASES: dict[str, str] = {}


__all__ = [
    "import_module",
    "invalidate_caches",
    "reload",
    "machinery",
    "util",
    "resources",
    "metadata",
]

if "__path__" not in globals():
    _pkg_file = globals().get("__file__")
    if isinstance(_pkg_file, str):
        __path__ = [_os.path.dirname(_pkg_file)]


def _runtime_modules() -> dict[str, object]:
    modules = _MOLT_IMPORTLIB_RUNTIME_MODULES()
    if not isinstance(modules, dict):
        raise RuntimeError("invalid importlib runtime state payload: modules")
    return modules


def import_module(name: str, package: str | None = None):
    resolved = _MOLT_IMPORTLIB_RESOLVE_NAME(name, package)
    missing_name = _MOLT_IMPORTLIB_KNOWN_ABSENT_MISSING_NAME(resolved)
    if missing_name is not None:
        raise ModuleNotFoundError(f"No module named '{missing_name}'")
    alias = _MODULE_ALIASES.get(resolved)
    if alias is not None:
        target = import_module(alias)
        modules = _runtime_modules()
        modules[resolved] = target
        return target
    mod = _MOLT_IMPORTLIB_IMPORT_MODULE(resolved, util, machinery)
    modules = _runtime_modules()
    if resolved in modules:
        return modules[resolved]
    if mod is not None:
        return mod
    raise ModuleNotFoundError(f"No module named '{resolved}'")


def invalidate_caches() -> None:
    try:
        util._SPEC_CACHE.clear()  # type: ignore[attr-defined]
    except Exception:
        pass
    return None


def reload(module):
    name = getattr(module, "__name__", None)
    if not name:
        raise ImportError("module has no __name__")
    modules = _runtime_modules()
    module_file = getattr(module, "__file__", None)
    module_spec = getattr(module, "__spec__", None)
    module_loader = getattr(module_spec, "loader", None) if module_spec else None
    if isinstance(module_file, str):
        locations = getattr(module, "__path__", None)
        submodule_search_locations = None
        if isinstance(locations, (list, tuple)):
            submodule_search_locations = list(locations)
        loader_override = module_loader
        loader_cls = getattr(machinery, "BuiltinImporter", None)
        if (
            loader_override is not None
            and loader_cls is not None
            and isinstance(loader_override, loader_cls)
        ):
            loader_override = None
        spec = util.spec_from_file_location(
            name,
            module_file,
            loader=loader_override,
            submodule_search_locations=submodule_search_locations,
        )
        if (
            spec is not None
            and spec.loader is not None
            and hasattr(spec.loader, "exec_module")
        ):
            spec.loader.exec_module(module)
            modules[name] = module
            return module
    if module_loader is not None and hasattr(module_loader, "exec_module"):
        modules.pop(name, None)
        try:
            module_loader.exec_module(module)
        except Exception:
            modules[name] = module
            raise
        modules[name] = module
        return module
    spec = util.find_spec(name, None)
    if spec is not None and spec.loader is not None:
        if hasattr(spec.loader, "exec_module"):
            spec.loader.exec_module(module)
            return module
        if hasattr(spec.loader, "load_module"):
            return spec.loader.load_module(name)
    modules.pop(name, None)
    return import_module(name)
