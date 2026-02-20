"""Compatibility surface for CPython `_frozen_importlib_external`."""

from _intrinsics import require_intrinsic as _require_intrinsic

import importlib.machinery as _machinery
import importlib.util as _util
import marshal
import sys

_require_intrinsic("molt_capabilities_has", globals())


def _fallback_type(name: str):
    return type(name, (), {})


BYTECODE_SUFFIXES = list(getattr(_machinery, "BYTECODE_SUFFIXES", [".pyc"]))
DEBUG_BYTECODE_SUFFIXES = list(
    getattr(_machinery, "DEBUG_BYTECODE_SUFFIXES", [".pyc"])
)
EXTENSION_SUFFIXES = list(getattr(_machinery, "EXTENSION_SUFFIXES", []))
MAGIC_NUMBER = bytes(getattr(_machinery, "MAGIC_NUMBER", b"\x00\x00\x00\x00"))
OPTIMIZED_BYTECODE_SUFFIXES = list(
    getattr(_machinery, "OPTIMIZED_BYTECODE_SUFFIXES", [".pyc"])
)
SOURCE_SUFFIXES = list(getattr(_machinery, "SOURCE_SUFFIXES", [".py"]))

ExtensionFileLoader = getattr(
    _machinery, "ExtensionFileLoader", _fallback_type("ExtensionFileLoader")
)
FileFinder = getattr(_machinery, "FileFinder", _fallback_type("FileFinder"))
FileLoader = getattr(_machinery, "FileLoader", _fallback_type("FileLoader"))
NamespaceLoader = getattr(
    _machinery, "NamespaceLoader", _fallback_type("NamespaceLoader")
)
PathFinder = getattr(_machinery, "PathFinder", _fallback_type("PathFinder"))
SourceFileLoader = getattr(
    _machinery, "SourceFileLoader", _fallback_type("SourceFileLoader")
)
SourceLoader = getattr(_machinery, "SourceLoader", _fallback_type("SourceLoader"))
SourcelessFileLoader = getattr(
    _machinery, "SourcelessFileLoader", _fallback_type("SourcelessFileLoader")
)
WindowsRegistryFinder = getattr(
    _machinery, "WindowsRegistryFinder", _fallback_type("WindowsRegistryFinder")
)


def _cache_from_source(path, debug_override=None, *, optimization=None):
    del debug_override, optimization
    return path


def _decode_source(source):
    if isinstance(source, (bytes, bytearray)):
        return bytes(source).decode("utf-8")
    return source


def _source_from_cache(path):
    return path


cache_from_source = getattr(_util, "cache_from_source", _cache_from_source)
decode_source = getattr(_util, "decode_source", _decode_source)
source_from_cache = getattr(_util, "source_from_cache", _source_from_cache)
spec_from_file_location = getattr(_util, "spec_from_file_location")

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
