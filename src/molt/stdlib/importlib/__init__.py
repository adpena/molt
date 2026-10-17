"""Minimal import support for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


_MOLT_IMPORTLIB_IMPORT_MODULE = _require_intrinsic("molt_importlib_import_module")
_MOLT_IMPORTLIB_RELOAD = _require_intrinsic("molt_importlib_reload")


__all__ = [
    "import_module",
    "invalidate_caches",
    "reload",
    "machinery",
    "util",
    "resources",
    "metadata",
]

def import_module(name: object, package: object = None):
    return _MOLT_IMPORTLIB_IMPORT_MODULE(name, package)


import importlib.machinery as machinery
import importlib.util as util
import importlib._bootstrap as _bootstrap  # noqa: F401
import importlib._bootstrap_external as _bootstrap_external  # noqa: F401


def invalidate_caches() -> None:
    import sys

    for finder in sys.meta_path:
        if hasattr(finder, "invalidate_caches"):
            finder.invalidate_caches()


def reload(module):
    return _MOLT_IMPORTLIB_RELOAD(module, util, import_module)


globals().pop("_require_intrinsic", None)
