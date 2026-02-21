"""Compatibility shim for importlib.resources.simple (Python 3.10 surface)."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())

from .resources.simple import (
    ResourceContainer,
    ResourceHandle,
    SimpleReader,
    TraversableReader,
)

__all__ = ["SimpleReader", "ResourceHandle", "ResourceContainer", "TraversableReader"]
