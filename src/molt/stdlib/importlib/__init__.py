"""Minimal import support for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


import os as _os
import sys as _sys

import importlib.machinery as machinery
import importlib.util as util

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_MODULE_IMPORT = _require_intrinsic("molt_module_import", globals())
_MOLT_IMPORTLIB_RUNTIME_STATE_PAYLOAD = _require_intrinsic(
    "molt_importlib_runtime_state_payload", globals()
)
_MOLT_EXCEPTION_CLEAR = _require_intrinsic("molt_exception_clear", globals())
_SPEC_FIRST_IMPORTS = {"asyncio.graph"}


def _known_absence_error(resolved: str) -> BaseException | None:
    if resolved == "asyncio.graph":
        return _sys.version_info < (3, 14)
    if resolved == "json.__main__":
        return _sys.version_info < (3, 14)
    return False


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
    payload = _MOLT_IMPORTLIB_RUNTIME_STATE_PAYLOAD()
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib runtime state payload: dict expected")
    modules = payload.get("modules")
    if not isinstance(modules, dict):
        raise RuntimeError("invalid importlib runtime state payload: modules")
    return modules


def _resolve_name(name: str, package: str | None) -> str:
    if not name.startswith("."):
        return name
    if not package:
        raise ImportError("relative import requires package")
    level = len(name) - len(name.lstrip("."))
    if level <= 0:
        return name
    pkg_bits = package.split(".")
    if level > len(pkg_bits):
        raise ImportError("attempted relative import beyond top-level package")
    base = ".".join(pkg_bits[:-level])
    return f"{base}{name[level:]}" if base else name[level:]


def _import_via_spec(resolved: str):
    modules = _runtime_modules()
    existing = modules.get(resolved)
    if existing is not None:
        return existing

    spec = util.find_spec(resolved)
    if spec is None:
        raise ModuleNotFoundError(f"No module named '{resolved}'")

    module = util.module_from_spec(spec)
    loader = getattr(spec, "loader", None)
    loader_cls = getattr(machinery, "MoltLoader", None)
    preseed_modules = not (
        loader is not None and loader_cls is not None and isinstance(loader, loader_cls)
    )
    if preseed_modules:
        modules[resolved] = module
    try:
        if loader is not None:
            if hasattr(loader, "exec_module"):
                loader.exec_module(module)
            elif hasattr(loader, "load_module"):
                loaded = loader.load_module(resolved)
                if loaded is not None:
                    module = loaded
        if not preseed_modules:
            modules[resolved] = module
        return modules.get(resolved, module)
    except Exception:
        modules.pop(resolved, None)
        raise


def _module_import_with_fallback(resolved: str):
    if resolved in _SPEC_FIRST_IMPORTS:
        return _import_via_spec(resolved)
    try:
        mod = _MOLT_MODULE_IMPORT(resolved)
        if mod is not None:
            return mod
        _MOLT_EXCEPTION_CLEAR()
        return _import_via_spec(resolved)
    except TypeError as exc:
        # Some dynamic modules may not round-trip through the direct runtime import
        # return path yet; fall back to the intrinsic-backed spec/loader flow.
        if "import returned non-module payload" not in str(exc):
            raise
        _MOLT_EXCEPTION_CLEAR()
        return _import_via_spec(resolved)
    except ImportError:
        _MOLT_EXCEPTION_CLEAR()
        return _import_via_spec(resolved)
    except BaseException as exc:  # noqa: BLE001
        if type(exc).__name__ not in {"ImportError", "ModuleNotFoundError"}:
            raise
        _MOLT_EXCEPTION_CLEAR()
        return _import_via_spec(resolved)


def import_module(name: str, package: str | None = None):
    resolved = _resolve_name(name, package)
    known_absence = _known_absence_error(resolved)
    if known_absence is not None:
        raise known_absence
    modules = _runtime_modules()
    if resolved in modules:
        return modules[resolved]
    mod = _module_import_with_fallback(resolved)
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
        loader_cls = getattr(machinery, "MoltLoader", None)
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
