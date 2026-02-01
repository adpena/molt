"""Minimal importlib support for Molt."""

from __future__ import annotations

import builtins as _builtins
import sys as _sys

import importlib.machinery as machinery
import importlib.util as util

__all__ = [
    "import_module",
    "invalidate_caches",
    "reload",
    "machinery",
    "util",
]


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
        return importer(resolved, {}, {}, ["_"], 0)
    if isinstance(modules, dict) and resolved in modules:
        return modules[resolved]
    raise ImportError(f"No module named '{resolved}'")


def invalidate_caches() -> None:
    return None


def reload(module):
    name = getattr(module, "__name__", None)
    if not name:
        raise ImportError("module has no __name__")
    return import_module(name)
