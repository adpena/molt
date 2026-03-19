"""Compatibility surface for CPython `_frozen_importlib_external`."""

from _intrinsics import require_intrinsic as _require_intrinsic

import importlib.machinery as _machinery
import importlib.util as _util
import marshal
import sys

_require_intrinsic("molt_capabilities_has")
_MOLT_IMPORTLIB_FROZEN_EXTERNAL_PAYLOAD = _require_intrinsic(
    "molt_importlib_frozen_external_payload"
)


def _load_payload() -> dict[str, object]:
    payload = _MOLT_IMPORTLIB_FROZEN_EXTERNAL_PAYLOAD(_machinery, _util)
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

cache_from_source = _payload_get(_PAYLOAD, "cache_from_source")
decode_source = _payload_get(_PAYLOAD, "decode_source")
source_from_cache = _payload_get(_PAYLOAD, "source_from_cache")
spec_from_file_location = _payload_get(_PAYLOAD, "spec_from_file_location")

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
