"""Minimal importlib.util helpers for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from typing import TYPE_CHECKING as _TYPE_CHECKING, Any as _Any

import abc as _abc
import sys

import importlib.machinery as _machinery

if _TYPE_CHECKING:
    from importlib.machinery import ModuleSpec

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_IMPORTLIB_CACHE_FROM_SOURCE = _require_intrinsic(
    "molt_importlib_cache_from_source", globals()
)
_MOLT_IMPORTLIB_DECODE_SOURCE = _require_intrinsic(
    "molt_importlib_decode_source", globals()
)
_MOLT_IMPORTLIB_SOURCE_HASH = _require_intrinsic(
    "molt_importlib_source_hash", globals()
)
_MOLT_IMPORTLIB_SOURCE_FROM_CACHE = _require_intrinsic(
    "molt_importlib_source_from_cache", globals()
)
_MOLT_IMPORTLIB_FIND_SPEC = _require_intrinsic("molt_importlib_find_spec", globals())
_MOLT_IMPORTLIB_BOOTSTRAP_PAYLOAD = _require_intrinsic(
    "molt_importlib_bootstrap_payload", globals()
)
_MOLT_IMPORTLIB_RESOLVE_NAME = _require_intrinsic(
    "molt_importlib_resolve_name", globals()
)
_MOLT_IMPORTLIB_RUNTIME_STATE_VIEW = _require_intrinsic(
    "molt_importlib_runtime_state_view", globals()
)
_MOLT_IMPORTLIB_EXISTING_SPEC = _require_intrinsic(
    "molt_importlib_existing_spec", globals()
)
_MOLT_IMPORTLIB_PARENT_SEARCH_PATHS = _require_intrinsic(
    "molt_importlib_parent_search_paths", globals()
)
_MOLT_IMPORTLIB_ENSURE_DEFAULT_META_PATH = _require_intrinsic(
    "molt_importlib_ensure_default_meta_path", globals()
)
_MOLT_IMPORTLIB_MODULE_FROM_SPEC = _require_intrinsic(
    "molt_importlib_module_from_spec", globals()
)
_MOLT_IMPORTLIB_SPEC_FROM_LOADER = _require_intrinsic(
    "molt_importlib_spec_from_loader", globals()
)
_MOLT_IMPORTLIB_SPEC_FROM_FILE_LOCATION = _require_intrinsic(
    "molt_importlib_spec_from_file_location", globals()
)
_MOLT_IMPORTLIB_COERCE_SEARCH_PATHS = _require_intrinsic(
    "molt_importlib_coerce_search_paths", globals()
)
_MOLT_IMPORTLIB_FINDER_SIGNATURE = _require_intrinsic(
    "molt_importlib_finder_signature", globals()
)
_MOLT_IMPORTLIB_PATH_IMPORTER_CACHE_SIGNATURE = _require_intrinsic(
    "molt_importlib_path_importer_cache_signature", globals()
)


__all__ = [
    "LazyLoader",
    "Loader",
    "MAGIC_NUMBER",
    "cache_from_source",
    "decode_source",
    "find_spec",
    "module_from_spec",
    "source_hash",
    "source_from_cache",
    "spec_from_file_location",
    "spec_from_loader",
    "resolve_name",
]


class Loader(metaclass=_abc.ABCMeta):
    pass


class LazyLoader(metaclass=_abc.ABCMeta):
    pass


MAGIC_NUMBER = b"\x00\x00\x00\x00"


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


def _runtime_state_view() -> dict[str, _Any]:
    payload = _MOLT_IMPORTLIB_RUNTIME_STATE_VIEW()
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


def _ensure_default_meta_path() -> None:
    _MOLT_IMPORTLIB_ENSURE_DEFAULT_META_PATH(_machinery)


def _cache_from_source(path: str) -> str:
    cached = _MOLT_IMPORTLIB_CACHE_FROM_SOURCE(path)
    if not isinstance(cached, str):
        raise RuntimeError("invalid importlib cache payload: str expected")
    return cached


def cache_from_source(path: str, debug_override=None, *, optimization=None) -> str:
    del debug_override, optimization
    return _cache_from_source(path)


def decode_source(source):
    return _MOLT_IMPORTLIB_DECODE_SOURCE(source)


def source_hash(source_bytes):
    return _MOLT_IMPORTLIB_SOURCE_HASH(source_bytes)


def source_from_cache(path):
    return _MOLT_IMPORTLIB_SOURCE_FROM_CACHE(path)


def resolve_name(name: str, package: str | None) -> str:
    return _MOLT_IMPORTLIB_RESOLVE_NAME(name, package)


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


def _coerce_search_paths(value: _Any, label: str) -> tuple[str, ...]:
    out = _MOLT_IMPORTLIB_COERCE_SEARCH_PATHS(value, label)
    if not isinstance(out, tuple) or not all(isinstance(entry, str) for entry in out):
        raise RuntimeError("invalid importlib coerce search paths payload")
    return out


def _finder_signature(finders: _Any, label: str) -> tuple[int, ...]:
    out = _MOLT_IMPORTLIB_FINDER_SIGNATURE(finders, label)
    if not isinstance(out, tuple) or not all(isinstance(entry, int) for entry in out):
        raise RuntimeError("invalid importlib finder signature payload")
    return out


def _path_importer_cache_signature(
    path_importer_cache: _Any, label: str
) -> tuple[tuple[str, int], ...]:
    out = _MOLT_IMPORTLIB_PATH_IMPORTER_CACHE_SIGNATURE(path_importer_cache, label)
    if not isinstance(out, tuple):
        raise RuntimeError("invalid importlib path importer cache signature payload")
    validated: list[tuple[str, int]] = []
    for entry in out:
        if (
            not isinstance(entry, tuple)
            or len(entry) != 2
            or not isinstance(entry[0], str)
            or not isinstance(entry[1], int)
        ):
            raise RuntimeError(
                "invalid importlib path importer cache signature payload"
            )
        validated.append((entry[0], entry[1]))
    return tuple(validated)


def _search_paths_for_resolved(
    resolved: str, modules: dict[str, _Any], snapshot: tuple[str, ...]
) -> tuple[tuple[str, ...], bool]:
    payload = _MOLT_IMPORTLIB_PARENT_SEARCH_PATHS(resolved, modules)
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib parent search paths payload: dict")
    has_parent = payload.get("has_parent")
    if not isinstance(has_parent, bool):
        raise RuntimeError("invalid importlib parent search paths payload: has_parent")
    if not has_parent:
        return _search_paths_from_snapshot(snapshot), False

    needs_parent_spec = payload.get("needs_parent_spec")
    if not isinstance(needs_parent_spec, bool):
        raise RuntimeError(
            "invalid importlib parent search paths payload: needs_parent_spec"
        )
    if needs_parent_spec:
        parent_name = payload.get("parent_name")
        if not isinstance(parent_name, str):
            raise RuntimeError(
                "invalid importlib parent search paths payload: parent_name"
            )
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

    search_paths = payload.get("search_paths")
    if not isinstance(search_paths, tuple) or not all(
        isinstance(entry, str) for entry in search_paths
    ):
        raise RuntimeError(
            "invalid importlib parent search paths payload: search_paths"
        )
    return search_paths, True


def _find_spec_in_path(
    fullname: str,
    search_paths: list[str],
    meta_path: _Any,
    path_hooks: _Any,
    path_importer_cache: _Any,
    package_context: bool,
) -> ModuleSpec | None:
    spec = _MOLT_IMPORTLIB_FIND_SPEC(
        fullname,
        search_paths,
        _importlib_module_file(),
        meta_path,
        path_hooks,
        path_importer_cache,
        package_context,
        _machinery,
    )
    return spec


_ensure_default_meta_path()


def find_spec(name: str, package: str | None = None):
    resolved = resolve_name(name, package)
    runtime_state = _runtime_state_view()
    modules = runtime_state["modules"]
    existing_spec = _MOLT_IMPORTLIB_EXISTING_SPEC(resolved, modules, _machinery)
    if existing_spec is not None:
        return existing_spec
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
    return _MOLT_IMPORTLIB_MODULE_FROM_SPEC(spec)


def spec_from_loader(
    name: str,
    loader,
    origin: str | None = None,
    is_package: bool | None = None,
):
    return _MOLT_IMPORTLIB_SPEC_FROM_LOADER(
        name, loader, origin, is_package, _machinery
    )


def spec_from_file_location(
    name: str,
    location,
    loader=None,
    submodule_search_locations=None,
):
    return _MOLT_IMPORTLIB_SPEC_FROM_FILE_LOCATION(
        name, location, loader, submodule_search_locations, _machinery
    )
