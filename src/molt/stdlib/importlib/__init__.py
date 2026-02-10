"""Minimal import support for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


import os as _os

import importlib.machinery as machinery
import importlib.util as util

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_MODULE_IMPORT = _require_intrinsic("molt_module_import", globals())
_MOLT_IMPORTLIB_RUNTIME_STATE_PAYLOAD = _require_intrinsic(
    "molt_importlib_runtime_state_payload", globals()
)


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
    modules[resolved] = module
    try:
        loader = getattr(spec, "loader", None)
        if loader is not None:
            if hasattr(loader, "exec_module"):
                loader.exec_module(module)
            elif hasattr(loader, "load_module"):
                loaded = loader.load_module(resolved)
                if loaded is not None:
                    module = loaded
        return modules.get(resolved, module)
    except Exception:
        modules.pop(resolved, None)
        raise


def _module_import_with_fallback(resolved: str):
    try:
        return _MOLT_MODULE_IMPORT(resolved)
    except TypeError as exc:
        # Some dynamic modules may not round-trip through the direct runtime import
        # return path yet; fall back to the intrinsic-backed spec/loader flow.
        if "import returned non-module payload" not in str(exc):
            raise
        return _import_via_spec(resolved)
    except ImportError:
        return _import_via_spec(resolved)


def import_module(name: str, package: str | None = None):
    resolved = _resolve_name(name, package)
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
    spec = util.find_spec(name, None)
    if spec is not None and spec.loader is not None:
        if hasattr(spec.loader, "exec_module"):
            spec.loader.exec_module(module)
            return module
        if hasattr(spec.loader, "load_module"):
            return spec.loader.load_module(name)
    modules = _runtime_modules()
    modules.pop(name, None)
    return import_module(name)
