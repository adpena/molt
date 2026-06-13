"""Compatibility surface for CPython `_frozen_importlib_external`."""

from _intrinsics import require_intrinsic as _require_intrinsic

import marshal
import sys

_machinery = __import__("importlib.machinery", globals(), locals(), ("ModuleSpec",), 0)

_require_intrinsic("molt_capabilities_has")
_MOLT_IMPORTLIB_FROZEN_EXTERNAL_PAYLOAD = _require_intrinsic(
    "molt_importlib_frozen_external_payload"
)
_MOLT_IMPORTLIB_CACHE_FROM_SOURCE = _require_intrinsic(
    "molt_importlib_cache_from_source"
)
_MOLT_IMPORTLIB_DECODE_SOURCE = _require_intrinsic("molt_importlib_decode_source")
_MOLT_IMPORTLIB_SOURCE_FROM_CACHE = _require_intrinsic(
    "molt_importlib_source_from_cache"
)
_MOLT_IMPORTLIB_SPEC_FROM_FILE_LOCATION = _require_intrinsic(
    "molt_importlib_spec_from_file_location"
)


def _load_payload() -> dict[str, object]:
    payload = _MOLT_IMPORTLIB_FROZEN_EXTERNAL_PAYLOAD(_machinery, None)
    if not isinstance(payload, dict):
        raise RuntimeError("invalid importlib frozen external payload: dict expected")
    return payload


def _payload_get(payload: dict[str, object], name: str):
    if name not in payload:
        raise RuntimeError(f"invalid importlib frozen external payload: missing {name}")
    return payload[name]


_PAYLOAD = _load_payload()
BYTECODE_SUFFIXES = list(_payload_get(_PAYLOAD, "BYTECODE_SUFFIXES"))
DEBUG_BYTECODE_SUFFIXES = list(_payload_get(_PAYLOAD, "DEBUG_BYTECODE_SUFFIXES"))
EXTENSION_SUFFIXES = list(_payload_get(_PAYLOAD, "EXTENSION_SUFFIXES"))
MAGIC_NUMBER = bytes(_payload_get(_PAYLOAD, "MAGIC_NUMBER"))
OPTIMIZED_BYTECODE_SUFFIXES = list(
    _payload_get(_PAYLOAD, "OPTIMIZED_BYTECODE_SUFFIXES")
)
SOURCE_SUFFIXES = list(_payload_get(_PAYLOAD, "SOURCE_SUFFIXES"))

ExtensionFileLoader = _payload_get(_PAYLOAD, "ExtensionFileLoader")
FileFinder = _payload_get(_PAYLOAD, "FileFinder")
FileLoader = _payload_get(_PAYLOAD, "FileLoader")
NamespaceLoader = _payload_get(_PAYLOAD, "NamespaceLoader")
PathFinder = _payload_get(_PAYLOAD, "PathFinder")
SourceFileLoader = _payload_get(_PAYLOAD, "SourceFileLoader")
SourceLoader = _payload_get(_PAYLOAD, "SourceLoader")
SourcelessFileLoader = _payload_get(_PAYLOAD, "SourcelessFileLoader")
_LoaderBasics = _payload_get(_PAYLOAD, "_LoaderBasics")
WindowsRegistryFinder = _payload_get(_PAYLOAD, "WindowsRegistryFinder")


def cache_from_source(path: str, debug_override=None, *, optimization=None) -> str:
    del debug_override, optimization
    return _MOLT_IMPORTLIB_CACHE_FROM_SOURCE(path)


def decode_source(source):
    return _MOLT_IMPORTLIB_DECODE_SOURCE(source)


def source_from_cache(path):
    return _MOLT_IMPORTLIB_SOURCE_FROM_CACHE(path)


def spec_from_file_location(
    name: str,
    location,
    loader=None,
    submodule_search_locations=None,
):
    return _MOLT_IMPORTLIB_SPEC_FROM_FILE_LOCATION(
        name, location, loader, submodule_search_locations, _machinery
    )

path_sep = "/"
path_sep_tuple = ("/", "\\")
path_separators = "/\\"

__all__ = [
    "BYTECODE_SUFFIXES",
    "DEBUG_BYTECODE_SUFFIXES",
    "EXTENSION_SUFFIXES",
    "ExtensionFileLoader",
    "FileFinder",
    "FileLoader",
    "MAGIC_NUMBER",
    "NamespaceLoader",
    "OPTIMIZED_BYTECODE_SUFFIXES",
    "PathFinder",
    "SOURCE_SUFFIXES",
    "SourceFileLoader",
    "SourceLoader",
    "SourcelessFileLoader",
    "_LoaderBasics",
    "WindowsRegistryFinder",
    "cache_from_source",
    "decode_source",
    "marshal",
    "path_sep",
    "path_sep_tuple",
    "path_separators",
    "source_from_cache",
    "spec_from_file_location",
    "sys",
]


globals().pop("_require_intrinsic", None)
