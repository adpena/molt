"""Minimal importlib.util helpers for Molt."""

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
_MOLT_IMPORTLIB_RUNTIME_STATE_PAYLOAD = _require_intrinsic(
    "molt_importlib_runtime_state_payload", globals()
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
    tuple[
        str,
        tuple[str, ...],
        tuple[int, ...],
        tuple[int, ...],
        tuple[tuple[str, int], ...],
        bool,
    ],
    ModuleSpec | None,
] = {}
_DEFAULT_META_PATH_BOOTSTRAPPED = False


def _runtime_state_payload() -> dict[str, Any]:
    payload = _MOLT_IMPORTLIB_RUNTIME_STATE_PAYLOAD()
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib runtime state payload: dict expected")
    modules = payload.get("modules")
    meta_path = payload.get("meta_path")
    path_hooks = payload.get("path_hooks")
    path_importer_cache = payload.get("path_importer_cache")
    if not isinstance(modules, dict):
        raise RuntimeError("invalid importlib runtime state payload: modules")
    if path_importer_cache is not None and not isinstance(path_importer_cache, dict):
        raise RuntimeError(
            "invalid importlib runtime state payload: path_importer_cache"
        )
    return {
        "modules": modules,
        "meta_path": meta_path,
        "path_hooks": path_hooks,
        "path_importer_cache": path_importer_cache,
    }


def _runtime_state_view() -> dict[str, Any]:
    """Return import runtime state with live sys.* precedence for mutables.

    CPython importlib resolves against the current sys state, not an older
    snapshot. Prefer live sys attributes when available and type-compatible,
    while preserving intrinsic payload as a fallback when attributes are
    unavailable.
    """

    payload = _runtime_state_payload()

    sys_modules = getattr(sys, "modules", None)
    if isinstance(sys_modules, dict):
        payload["modules"] = sys_modules

    # Preserve iterable semantics; concrete type validation happens later.
    for key in ("meta_path", "path_hooks"):
        live = getattr(sys, key, None)
        if live is not None:
            payload[key] = live

    live_cache = getattr(sys, "path_importer_cache", None)
    if live_cache is None or isinstance(live_cache, dict):
        payload["path_importer_cache"] = live_cache

    return payload


def _ensure_default_meta_path() -> None:
    global _DEFAULT_META_PATH_BOOTSTRAPPED
    if _DEFAULT_META_PATH_BOOTSTRAPPED:
        return
    meta_path = getattr(sys, "meta_path", None)
    if not isinstance(meta_path, list):
        _DEFAULT_META_PATH_BOOTSTRAPPED = True
        return
    if meta_path:
        _DEFAULT_META_PATH_BOOTSTRAPPED = True
        return
    path_finder = getattr(machinery, "PathFinder", None)
    if path_finder is None:
        _DEFAULT_META_PATH_BOOTSTRAPPED = True
        return
    if path_finder in meta_path:
        _DEFAULT_META_PATH_BOOTSTRAPPED = True
        return
    meta_path.append(path_finder)
    _DEFAULT_META_PATH_BOOTSTRAPPED = True


def _machinery_attr(name: str) -> Any:
    value = getattr(machinery, name, None)
    if value is None:
        raise RuntimeError(f"importlib.machinery missing required attribute: {name}")
    return value


def _module_spec_cls():
    return _machinery_attr("ModuleSpec")


def _source_file_loader_cls():
    return _machinery_attr("SourceFileLoader")


def _extension_file_loader_cls():
    return _machinery_attr("ExtensionFileLoader")


def _sourceless_file_loader_cls():
    return _machinery_attr("SourcelessFileLoader")


def _zip_source_loader_cls():
    return _machinery_attr("ZipSourceLoader")


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


def _coerce_search_paths(value: Any, label: str) -> tuple[str, ...]:
    if value is None:
        return ()
    if isinstance(value, str):
        return (value,) if value else ()
    if isinstance(value, (list, tuple)):
        return tuple(str(entry) for entry in value if str(entry))
    try:
        return tuple(str(entry) for entry in value if str(entry))
    except Exception as exc:
        raise RuntimeError(label) from exc


def _finder_signature(finders: Any, label: str) -> tuple[int, ...]:
    try:
        return tuple(id(finder) for finder in finders)
    except Exception as exc:
        raise RuntimeError(label) from exc


def _path_importer_cache_signature(
    path_importer_cache: Any, label: str
) -> tuple[tuple[str, int], ...]:
    if path_importer_cache is None:
        return ()
    if not isinstance(path_importer_cache, dict):
        raise RuntimeError(label)
    try:
        return tuple(
            sorted(
                (str(key), id(value))
                for key, value in path_importer_cache.items()
                if isinstance(key, str)
            )
        )
    except Exception as exc:
        raise RuntimeError(label) from exc


def _find_spec_via_meta_path(
    fullname: str,
    search_paths: tuple[str, ...],
    *,
    package_context: bool,
) -> ModuleSpec | None:
    meta_path = getattr(sys, "meta_path", None)
    if meta_path is None:
        return None
    try:
        finders = tuple(meta_path)
    except Exception as exc:
        raise RuntimeError("invalid meta_path iterable") from exc
    for finder in finders:
        find_spec = getattr(finder, "find_spec", None)
        if not callable(find_spec):
            continue
        path_arg = list(search_paths) if package_context else None
        try:
            spec = find_spec(fullname, path_arg, None)
        except TypeError:
            spec = find_spec(fullname, path_arg)
        if spec is not None:
            return spec
    return None


def _search_paths_for_resolved(
    resolved: str, modules: dict[str, Any], snapshot: tuple[str, ...]
) -> tuple[tuple[str, ...], bool]:
    parent_name, _, _ = resolved.rpartition(".")
    if not parent_name:
        return _search_paths_from_snapshot(snapshot), False
    parent = modules.get(parent_name)
    if parent is None:
        parent_spec = find_spec(parent_name)
        if parent_spec is None:
            return (), True
        return (
            _coerce_search_paths(
                getattr(parent_spec, "submodule_search_locations", None),
                "invalid parent package search path",
            ),
            True,
        )
    return (
        _coerce_search_paths(
            getattr(parent, "__path__", None),
            "invalid parent package search path",
        ),
        True,
    )


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
    fullname: str,
    search_paths: list[str],
    meta_path: Any,
    path_hooks: Any,
    path_importer_cache: Any,
    package_context: bool,
) -> ModuleSpec | None:
    payload = _MOLT_IMPORTLIB_FIND_SPEC_PAYLOAD(
        fullname,
        search_paths,
        _importlib_module_file(),
        meta_path,
        path_hooks,
        path_importer_cache,
        package_context,
    )
    if payload is None:
        return None
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib find-spec payload: dict expected")
    direct_spec = payload.get("spec")
    if direct_spec is not None:
        return direct_spec
    origin = payload.get("origin")
    is_package = payload.get("is_package")
    locations = payload.get("submodule_search_locations")
    cached = payload.get("cached")
    is_builtin = payload.get("is_builtin")
    has_location = payload.get("has_location")
    loader_kind = payload.get("loader_kind")
    zip_archive = payload.get("zip_archive")
    zip_inner_path = payload.get("zip_inner_path")
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
    if zip_archive is not None and not isinstance(zip_archive, str):
        raise RuntimeError("invalid importlib find-spec payload: zip_archive")
    if zip_inner_path is not None and not isinstance(zip_inner_path, str):
        raise RuntimeError("invalid importlib find-spec payload: zip_inner_path")
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
    elif loader_kind == "extension":
        if not isinstance(origin, str):
            raise RuntimeError(
                "invalid importlib find-spec payload: extension loader origin missing"
            )
        loader = _extension_file_loader_cls()(fullname, origin)
    elif loader_kind == "bytecode":
        if not isinstance(origin, str):
            raise RuntimeError(
                "invalid importlib find-spec payload: bytecode loader origin missing"
            )
        loader = _sourceless_file_loader_cls()(fullname, origin)
    elif loader_kind == "zip_source":
        if not isinstance(origin, str):
            raise RuntimeError(
                "invalid importlib find-spec payload: zip source origin missing"
            )
        if not isinstance(zip_archive, str):
            raise RuntimeError(
                "invalid importlib find-spec payload: zip source archive missing"
            )
        if not isinstance(zip_inner_path, str):
            raise RuntimeError(
                "invalid importlib find-spec payload: zip source inner path missing"
            )
        loader = _zip_source_loader_cls()(fullname, zip_archive, zip_inner_path)
    elif loader_kind == "namespace":
        if locations is None:
            raise RuntimeError(
                "invalid importlib find-spec payload: namespace locations missing"
            )
        loader = None
        origin = None
        cached = None
        is_package = True
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


_ensure_default_meta_path()


def find_spec(name: str, package: str | None = None):
    resolved = resolve_name(name, package)
    runtime_state = _runtime_state_view()
    modules = runtime_state["modules"]
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
    search_paths, package_context = _search_paths_for_resolved(
        resolved, modules, snapshot
    )
    meta_path = runtime_state["meta_path"]
    path_hooks = runtime_state["path_hooks"]
    path_importer_cache = runtime_state["path_importer_cache"]
    meta_path_sig = _finder_signature(meta_path, "invalid meta_path iterable")
    path_hooks_sig = _finder_signature(path_hooks, "invalid path_hooks iterable")
    path_importer_cache_sig = _path_importer_cache_signature(
        path_importer_cache, "invalid path_importer_cache mapping"
    )
    cache_key = (
        resolved,
        search_paths,
        meta_path_sig,
        path_hooks_sig,
        path_importer_cache_sig,
        package_context,
    )
    if cache_key in _SPEC_CACHE:
        return _SPEC_CACHE[cache_key]
    via_meta_path = _find_spec_via_meta_path(
        resolved, search_paths, package_context=package_context
    )
    if via_meta_path is not None:
        _SPEC_CACHE[cache_key] = via_meta_path
        return via_meta_path
    spec = _find_spec_in_path(
        resolved,
        list(search_paths),
        meta_path,
        path_hooks,
        path_importer_cache,
        package_context,
    )
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
