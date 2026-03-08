"""Minimal importlib.util helpers for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from typing import TYPE_CHECKING as _TYPE_CHECKING

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
_MOLT_IMPORTLIB_RESOLVE_NAME = _require_intrinsic(
    "molt_importlib_resolve_name", globals()
)
_MOLT_IMPORTLIB_FIND_SPEC_ORCHESTRATE = _require_intrinsic(
    "molt_importlib_find_spec_orchestrate", globals()
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
    "sys",
    "types",
]


class Loader(metaclass=_abc.ABCMeta):
    pass


class LazyLoader(metaclass=_abc.ABCMeta):
    pass


MAGIC_NUMBER = b"\x00\x00\x00\x00"
types = sys.modules.get("types", sys)

_SPEC_CACHE: dict[tuple[object, ...], ModuleSpec | None] = {}


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
    import sys as _ilu_sys

    _ilu_dict = getattr(_ilu_sys.modules.get(__name__), "__dict__", None) or globals()
    module_file = _ilu_dict.get("__file__")
    if isinstance(module_file, str) and module_file:
        return module_file
    return None


_ensure_default_meta_path()


def find_spec(name: str, package: str | None = None):
    resolved = resolve_name(name, package)
    return _MOLT_IMPORTLIB_FIND_SPEC_ORCHESTRATE(
        resolved,
        _path_snapshot(),
        _importlib_module_file(),
        _SPEC_CACHE,
        _machinery,
    )


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
