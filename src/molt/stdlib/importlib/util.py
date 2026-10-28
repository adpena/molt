"""Minimal importlib.util helpers for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import abc as _abc
import sys

_machinery = __import__("importlib.machinery", globals(), locals(), ("ModuleSpec",), 0)

if False:
    from importlib.machinery import ModuleSpec

_MOLT_IMPORTLIB_CACHE_FROM_SOURCE = _require_intrinsic(
    "molt_importlib_cache_from_source"
)
_MOLT_IMPORTLIB_DECODE_SOURCE = _require_intrinsic("molt_importlib_decode_source")
_MOLT_IMPORTLIB_SOURCE_HASH = _require_intrinsic("molt_importlib_source_hash")
_MOLT_IMPORTLIB_SOURCE_FROM_CACHE = _require_intrinsic(
    "molt_importlib_source_from_cache"
)
_MOLT_IMPORTLIB_RESOLVE_NAME = _require_intrinsic("molt_importlib_resolve_name")
_MOLT_IMPORTLIB_FIND_SPEC = _require_intrinsic("molt_importlib_find_spec")
_MOLT_IMPORTLIB_MODULE_FROM_SPEC = _require_intrinsic("molt_importlib_module_from_spec")
_MOLT_IMPORTLIB_SPEC_FROM_LOADER = _require_intrinsic("molt_importlib_spec_from_loader")
_MOLT_IMPORTLIB_SPEC_FROM_FILE_LOCATION = _require_intrinsic(
    "molt_importlib_spec_from_file_location"
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


def _find_spec(name, path, target=None):
    return _MOLT_IMPORTLIB_FIND_SPEC(name, path, target)


def find_spec(name: str, package: str | None = None):
    fullname = resolve_name(name, package) if name.startswith(".") else name
    if fullname not in sys.modules:
        parent_name = fullname.rpartition(".")[0]
        if parent_name:
            parent = __import__(parent_name, fromlist=["__path__"])
            try:
                parent_path = parent.__path__
            except AttributeError as exc:
                raise ModuleNotFoundError(
                    f"__path__ attribute not found on {parent_name!r} "
                    f"while trying to find {fullname!r}",
                    name=fullname,
                ) from exc
        else:
            parent_path = None
        return _find_spec(fullname, parent_path)
    module = sys.modules[fullname]
    if module is None:
        return None
    try:
        spec = module.__spec__
    except AttributeError:
        raise ValueError(f"{name}.__spec__ is not set") from None
    if spec is None:
        raise ValueError(f"{name}.__spec__ is None")
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


globals().pop("_require_intrinsic", None)
