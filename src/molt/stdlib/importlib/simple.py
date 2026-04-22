"""Compatibility shim for importlib.resources.simple (Python 3.10 surface)."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe")
_MOLT_IMPORTLIB_IMPORT_REQUIRED = _require_intrinsic("molt_importlib_import_required")

# Keep CPython's import graph shape while ensuring intrinsic-backed import lowering.
_MOLT_IMPORTLIB_IMPORT_REQUIRED("importlib.resources")

from .resources.simple import (
    ResourceContainer,
    ResourceHandle,
    SimpleReader,
    TraversableReader,
)

__all__ = ["SimpleReader", "ResourceHandle", "ResourceContainer", "TraversableReader"]

globals().pop("_require_intrinsic", None)
