"""Compatibility shim for importlib.resources.readers (Python 3.10 surface)."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe")
_MOLT_IMPORTLIB_IMPORT_REQUIRED = _require_intrinsic(
    "molt_importlib_import_required"
)

# Keep CPython's import graph shape while ensuring intrinsic-backed import lowering.
_MOLT_IMPORTLIB_IMPORT_REQUIRED("importlib.resources")

from .resources.readers import (
    FileReader,
    MultiplexedPath,
    NamespaceReader,
    ZipReader,
)

__all__ = ["FileReader", "ZipReader", "MultiplexedPath", "NamespaceReader"]

globals().pop("_require_intrinsic", None)
