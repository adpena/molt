"""Import-only importlib stubs for Molt."""

from __future__ import annotations

# TODO(stdlib-compat, owner:stdlib, milestone:SL3): implement import hooks + loaders.

from . import util

__all__ = [
    "import_module",
    "invalidate_caches",
    "reload",
    "util",
]


def import_module(_name: str, _package: str | None = None):
    raise ImportError("importlib.import_module is not supported in Molt")


def invalidate_caches() -> None:
    return None


def reload(_module):
    raise ImportError("importlib.reload is not supported in Molt")
