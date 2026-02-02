"""Minimal importlib.util helpers for Molt."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): support namespace packages, meta_path/path_hooks finders, extension modules, and zip/bytecode loaders.

from __future__ import annotations

from types import ModuleType
import sys
import os
import builtins

from importlib.machinery import ModuleSpec, SourceFileLoader
from importlib.machinery import MOLT_LOADER

__all__ = [
    "find_spec",
    "module_from_spec",
    "spec_from_file_location",
    "spec_from_loader",
    "resolve_name",
]


_SPEC_CACHE: dict[tuple[str, tuple[str, ...], str | None], ModuleSpec | None] = {}
_BUILTIN_MODULES = {"math"}


def _cache_from_source(path: str) -> str:
    base = os.path.basename(path)
    if base.endswith(".py"):
        cache_dir = os.path.join(os.path.dirname(path), "__pycache__")
        return os.path.join(cache_dir, f"{base}c")
    return f"{path}c"


def resolve_name(name: str, package: str | None) -> str:
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


def _path_snapshot() -> tuple[str, ...]:
    try:
        return tuple(sys.path)
    except Exception:
        return ()


def _stdlib_root() -> str | None:
    file_path = globals().get("__file__")
    if isinstance(file_path, str) and file_path:
        return os.path.dirname(os.path.dirname(file_path))
    return None


def _stdlib_candidates(paths: tuple[str, ...]) -> list[str]:
    candidates: list[str] = []
    for base in paths:
        if not base:
            base = "."
        candidate = os.path.join(base, "molt", "stdlib")
        if candidate not in candidates:
            candidates.append(candidate)
    return candidates


def _find_spec_via_import(fullname: str) -> ModuleSpec | None:
    importer = getattr(builtins, "__import__", None)
    if not callable(importer):
        return None
    modules = sys.modules
    already_loaded = fullname in modules
    try:
        importer(fullname, {}, {}, ["_"], 0)
    except Exception:
        return None
    module = modules.get(fullname)
    spec = getattr(module, "__spec__", None) if module is not None else None
    if spec is None:
        origin = getattr(module, "__file__", None) if module is not None else None
        loader = getattr(module, "__loader__", None) if module is not None else None
        spec = ModuleSpec(fullname, loader, origin=origin, is_package=False)
    if not already_loaded and fullname in modules:
        try:
            del modules[fullname]
        except Exception:
            pass
    return spec


def _set_cached(spec: ModuleSpec) -> None:
    if spec.cached is not None:
        return
    if isinstance(spec.origin, str):
        spec.cached = _cache_from_source(spec.origin)


def _find_spec_in_path(fullname: str, search_paths: list[str]) -> ModuleSpec | None:
    parts = fullname.split(".")
    current_paths = list(search_paths)
    for idx, part in enumerate(parts):
        is_last = idx == len(parts) - 1
        found_pkg = False
        next_paths: list[str] = []
        for base in current_paths:
            if not base:
                base = "."
            pkg_dir = os.path.join(base, part)
            init_file = os.path.join(pkg_dir, "__init__.py")
            if os.path.exists(init_file):
                if is_last:
                    loader = SourceFileLoader(fullname, init_file)
                    spec = ModuleSpec(
                        fullname, loader, origin=init_file, is_package=True
                    )
                    spec.submodule_search_locations = [pkg_dir]
                    _set_cached(spec)
                    return spec
                next_paths = [pkg_dir]
                found_pkg = True
                break
            mod_file = os.path.join(base, f"{part}.py")
            if is_last and os.path.exists(mod_file):
                loader = SourceFileLoader(fullname, mod_file)
                spec = ModuleSpec(fullname, loader, origin=mod_file, is_package=False)
                _set_cached(spec)
                return spec
        if found_pkg:
            current_paths = next_paths
            continue
        return None
    return None


def find_spec(name: str, package: str | None = None):
    resolved = resolve_name(name, package)
    if resolved in _BUILTIN_MODULES:
        spec = ModuleSpec(
            resolved, loader=MOLT_LOADER, origin="built-in", is_package=False
        )
        spec.cached = None
        spec.has_location = False
        return spec
    modules = sys.modules
    existing = modules.get(resolved)
    if existing is not None:
        spec = getattr(existing, "__spec__", None)
        if spec is not None:
            return spec
        file_path = getattr(existing, "__file__", None)
        if isinstance(file_path, str):
            return ModuleSpec(resolved, loader=None, origin=file_path, is_package=False)
        return ModuleSpec(resolved, loader=None, origin=None, is_package=False)
    snapshot = _path_snapshot()
    stdlib_root = _stdlib_root()
    cache_key = (resolved, snapshot, stdlib_root)
    if cache_key in _SPEC_CACHE:
        return _SPEC_CACHE[cache_key]
    search_paths = list(snapshot)
    if stdlib_root and stdlib_root not in search_paths:
        search_paths.append(stdlib_root)
    for candidate in _stdlib_candidates(snapshot):
        if candidate not in search_paths:
            search_paths.append(candidate)
    spec = _find_spec_in_path(resolved, search_paths)
    if spec is None:
        spec = _find_spec_via_import(resolved)
    _SPEC_CACHE[cache_key] = spec
    return spec


def module_from_spec(spec: ModuleSpec):
    module = None
    loader = spec.loader
    if loader is not None and hasattr(loader, "create_module"):
        try:
            module = loader.create_module(spec)
        except Exception:
            module = None
    if module is None:
        module = ModuleType(spec.name)
    module.__spec__ = spec
    module.__loader__ = spec.loader
    if spec.submodule_search_locations is not None:
        module.__package__ = spec.name
        module.__path__ = list(spec.submodule_search_locations)
    else:
        module.__package__ = spec.parent
    if spec.origin is not None:
        module.__file__ = spec.origin
    module.__cached__ = spec.cached
    return module


def spec_from_loader(
    name: str,
    loader,
    origin: str | None = None,
    is_package: bool | None = None,
):
    if is_package is None and loader is not None and hasattr(loader, "is_package"):
        try:
            is_package = bool(loader.is_package(name))
        except Exception:
            is_package = None
    spec = ModuleSpec(name, loader, origin=origin, is_package=is_package)
    _set_cached(spec)
    return spec


def spec_from_file_location(
    name: str,
    location,
    loader=None,
    submodule_search_locations=None,
):
    path = str(location)
    is_package = False
    if submodule_search_locations is not None:
        is_package = True
    elif os.path.basename(path) == "__init__.py":
        is_package = True
        submodule_search_locations = [os.path.dirname(path)]
    if loader is None:
        loader = SourceFileLoader(name, path)
    spec = ModuleSpec(name, loader, origin=path, is_package=is_package)
    if submodule_search_locations is not None:
        spec.submodule_search_locations = list(submodule_search_locations)
    _set_cached(spec)
    return spec
