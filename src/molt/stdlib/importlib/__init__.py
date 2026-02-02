"""Minimal import support for Molt."""

from __future__ import annotations

import builtins as _builtins
import os as _os
import sys as _sys

import importlib.machinery as machinery
import importlib.util as util

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


def import_module(name: str, package: str | None = None):
    resolved = _resolve_name(name, package)
    modules = getattr(_sys, "modules", None)
    if isinstance(modules, dict) and resolved in modules:
        return modules[resolved]
    importer = getattr(_builtins, "__import__", None)
    if callable(importer):
        try:
            mod = importer(resolved, {}, {}, ["_"], 0)
        except Exception:
            mod = None
        if isinstance(modules, dict) and resolved in modules:
            return modules[resolved]
        if mod is not None:
            return mod
    spec = util.find_spec(resolved, None)
    if spec is None:
        raise ImportError(f"No module named '{resolved}'")
    module = util.module_from_spec(spec)
    if isinstance(modules, dict):
        modules[resolved] = module
    if spec.loader is not None:
        if hasattr(spec.loader, "exec_module"):
            spec.loader.exec_module(module)
        elif hasattr(spec.loader, "load_module"):
            module = spec.loader.load_module(resolved)
    return module


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
    try:
        del _sys.modules[name]
    except Exception:
        pass
    return import_module(name)
