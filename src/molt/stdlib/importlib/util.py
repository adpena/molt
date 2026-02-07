"""Minimal importlib.util helpers for Molt."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): support namespace packages, custom meta_path/path_hooks finder execution, extension modules, and zip/bytecode loaders via intrinsic payloads.

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from typing import TYPE_CHECKING, Any

from types import ModuleType
import sys

import importlib.machinery as machinery

if TYPE_CHECKING:
    from importlib.machinery import ModuleSpec

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_IMPORTLIB_CACHE_FROM_SOURCE = _require_intrinsic(
    "molt_importlib_cache_from_source", globals()
)
_MOLT_IMPORTLIB_FIND_SPEC_PAYLOAD = _require_intrinsic(
    "molt_importlib_find_spec_payload", globals()
)
_MOLT_IMPORTLIB_BOOTSTRAP_PAYLOAD = _require_intrinsic(
    "molt_importlib_bootstrap_payload", globals()
)
_MOLT_IMPORTLIB_SPEC_FROM_FILE_LOCATION_PAYLOAD = _require_intrinsic(
    "molt_importlib_spec_from_file_location_payload", globals()
)


__all__ = [
    "find_spec",
    "module_from_spec",
    "spec_from_file_location",
    "spec_from_loader",
    "resolve_name",
]


_SPEC_CACHE: dict[
    tuple[str, tuple[str, ...], int, int, int | None, int | None], ModuleSpec | None
] = {}


def _machinery_attr(name: str) -> Any:
    value = getattr(machinery, name, None)
    if value is None:
        raise RuntimeError(f"importlib.machinery missing required attribute: {name}")
    return value


def _module_spec_cls():
    return _machinery_attr("ModuleSpec")


def _source_file_loader_cls():
    return _machinery_attr("SourceFileLoader")


def _molt_loader():
    loader = getattr(machinery, "MOLT_LOADER", None)
    if loader is not None:
        return loader
    cls = getattr(machinery, "MoltLoader", None)
    if cls is None:
        raise RuntimeError("importlib.machinery missing required attribute: MoltLoader")
    loader = cls()
    setattr(machinery, "MOLT_LOADER", loader)
    return loader


def _cache_from_source(path: str) -> str:
    cached = _MOLT_IMPORTLIB_CACHE_FROM_SOURCE(path)
    if not isinstance(cached, str):
        raise RuntimeError("invalid importlib cache payload: str expected")
    return cached


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


def _safe_len(value: object) -> int | None:
    try:
        return len(value)  # type: ignore[arg-type]
    except Exception:
        return None


def _importlib_module_file() -> str | None:
    module_file = globals().get("__file__")
    if isinstance(module_file, str) and module_file:
        return module_file
    return None


def _search_paths_from_snapshot(snapshot: tuple[str, ...]) -> tuple[str, ...]:
    payload = _MOLT_IMPORTLIB_BOOTSTRAP_PAYLOAD(snapshot, _importlib_module_file())
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib bootstrap payload: dict expected")
    resolved = payload.get("resolved_search_paths")
    if not isinstance(resolved, (list, tuple)):
        raise RuntimeError("invalid importlib bootstrap payload: resolved_search_paths")
    out: list[str] = []
    for entry in resolved:
        if not isinstance(entry, str):
            raise RuntimeError(
                "invalid importlib bootstrap payload: str entries expected"
            )
        out.append(entry)
    return tuple(out)


def _set_cached(spec: ModuleSpec) -> None:
    if spec.cached is not None:
        return
    if isinstance(spec.origin, str):
        spec.cached = _cache_from_source(spec.origin)


def _spec_from_file_location_payload(path: str) -> dict[str, object]:
    payload = _MOLT_IMPORTLIB_SPEC_FROM_FILE_LOCATION_PAYLOAD(path)
    if not isinstance(payload, dict):
        raise RuntimeError(
            "invalid importlib spec_from_file_location payload: dict expected"
        )
    resolved_path = payload.get("path")
    is_package = payload.get("is_package")
    package_root = payload.get("package_root")
    if not isinstance(resolved_path, str):
        raise RuntimeError("invalid importlib spec_from_file_location payload: path")
    if not isinstance(is_package, bool):
        raise RuntimeError(
            "invalid importlib spec_from_file_location payload: is_package"
        )
    if package_root is not None and not isinstance(package_root, str):
        raise RuntimeError(
            "invalid importlib spec_from_file_location payload: package_root"
        )
    return {
        "path": resolved_path,
        "is_package": is_package,
        "package_root": package_root,
    }


def _find_spec_in_path(
    fullname: str, search_paths: list[str], meta_path: Any, path_hooks: Any
) -> ModuleSpec | None:
    payload = _MOLT_IMPORTLIB_FIND_SPEC_PAYLOAD(
        fullname,
        search_paths,
        _importlib_module_file(),
        meta_path,
        path_hooks,
    )
    if payload is None:
        return None
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib find-spec payload: dict expected")
    origin = payload.get("origin")
    is_package = payload.get("is_package")
    locations = payload.get("submodule_search_locations")
    cached = payload.get("cached")
    is_builtin = payload.get("is_builtin")
    has_location = payload.get("has_location")
    loader_kind = payload.get("loader_kind")
    meta_path_count = payload.get("meta_path_count")
    path_hooks_count = payload.get("path_hooks_count")
    if origin is not None and not isinstance(origin, str):
        raise RuntimeError("invalid importlib find-spec payload: origin")
    if not isinstance(is_package, bool):
        raise RuntimeError("invalid importlib find-spec payload: is_package")
    if locations is not None:
        if not isinstance(locations, list) or not all(
            isinstance(entry, str) for entry in locations
        ):
            raise RuntimeError(
                "invalid importlib find-spec payload: submodule_search_locations"
            )
    if cached is not None and not isinstance(cached, str):
        raise RuntimeError("invalid importlib find-spec payload: cached")
    if not isinstance(is_builtin, bool):
        raise RuntimeError("invalid importlib find-spec payload: is_builtin")
    if not isinstance(has_location, bool):
        raise RuntimeError("invalid importlib find-spec payload: has_location")
    if not isinstance(loader_kind, str):
        raise RuntimeError("invalid importlib find-spec payload: loader_kind")
    if not isinstance(meta_path_count, int):
        raise RuntimeError("invalid importlib find-spec payload: meta_path_count")
    if not isinstance(path_hooks_count, int):
        raise RuntimeError("invalid importlib find-spec payload: path_hooks_count")

    if loader_kind == "builtin":
        loader = _molt_loader()
    elif loader_kind == "source":
        if not isinstance(origin, str):
            raise RuntimeError(
                "invalid importlib find-spec payload: source loader origin missing"
            )
        loader = _source_file_loader_cls()(fullname, origin)
    else:
        raise RuntimeError(f"unsupported importlib loader kind: {loader_kind}")

    spec = _module_spec_cls()(fullname, loader, origin=origin, is_package=is_package)
    if locations is not None:
        spec.submodule_search_locations = list(locations)
    spec.cached = cached
    spec.has_location = has_location
    if spec.cached is None and isinstance(spec.origin, str) and loader_kind == "source":
        _set_cached(spec)
    return spec


def find_spec(name: str, package: str | None = None):
    resolved = resolve_name(name, package)
    modules = sys.modules
    existing = modules.get(resolved)
    if existing is not None:
        spec = getattr(existing, "__spec__", None)
        if spec is not None:
            return spec
        file_path = getattr(existing, "__file__", None)
        if isinstance(file_path, str):
            return _module_spec_cls()(
                resolved, loader=None, origin=file_path, is_package=False
            )
        return _module_spec_cls()(resolved, loader=None, origin=None, is_package=False)
    snapshot = _path_snapshot()
    search_paths = _search_paths_from_snapshot(snapshot)
    meta_path = getattr(sys, "meta_path", ())
    path_hooks = getattr(sys, "path_hooks", ())
    cache_key = (
        resolved,
        search_paths,
        id(meta_path),
        id(path_hooks),
        _safe_len(meta_path),
        _safe_len(path_hooks),
    )
    if cache_key in _SPEC_CACHE:
        return _SPEC_CACHE[cache_key]
    spec = _find_spec_in_path(resolved, list(search_paths), meta_path, path_hooks)
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
    setattr(module, "__cached__", spec.cached)
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
    spec = _module_spec_cls()(name, loader, origin=origin, is_package=is_package)
    _set_cached(spec)
    return spec


def spec_from_file_location(
    name: str,
    location,
    loader=None,
    submodule_search_locations=None,
):
    raw_path = str(location)
    payload = _spec_from_file_location_payload(raw_path)
    path = payload["path"]
    inferred_is_package = bool(payload["is_package"])
    inferred_package_root = payload["package_root"]
    is_package = submodule_search_locations is not None or inferred_is_package
    if submodule_search_locations is None and inferred_is_package:
        if not isinstance(inferred_package_root, str):
            raise RuntimeError(
                "invalid importlib spec_from_file_location payload: package_root"
            )
        submodule_search_locations = [inferred_package_root]
    if loader is None:
        loader = _source_file_loader_cls()(name, path)
    spec = _module_spec_cls()(name, loader, origin=path, is_package=is_package)
    if submodule_search_locations is not None:
        spec.submodule_search_locations = list(submodule_search_locations)
    _set_cached(spec)
    return spec
