"""Compatibility shim for importlib.resources.readers (Python 3.10 surface)."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())

from .resources.readers import (
    FileReader,
    MultiplexedPath,
    NamespaceReader,
    ZipReader,
)

__all__ = ["FileReader", "ZipReader", "MultiplexedPath", "NamespaceReader"]
