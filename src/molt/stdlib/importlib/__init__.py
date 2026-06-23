"""Minimal import support for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


import os as _os

_require_intrinsic("molt_stdlib_probe")
_MOLT_IMPORTLIB_IMPORT_MODULE = _require_intrinsic("molt_importlib_import_module")
_MOLT_IMPORTLIB_INVALIDATE_CACHES = _require_intrinsic(
    "molt_importlib_invalidate_caches"
)
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

if "__path__" not in globals():
    _pkg_file = globals().get("__file__")
    if isinstance(_pkg_file, str):
        __path__ = [_os.path.dirname(_pkg_file)]


def import_module(name: object, package: object = None):
    return _MOLT_IMPORTLIB_IMPORT_MODULE(name, package)


import importlib.machinery as machinery
import importlib.util as util
import importlib._bootstrap as _bootstrap  # noqa: F401
import importlib._bootstrap_external as _bootstrap_external  # noqa: F401


def invalidate_caches() -> None:
    result = _MOLT_IMPORTLIB_INVALIDATE_CACHES()
    if result is not None:
        raise RuntimeError(
            "invalid importlib invalidate caches intrinsic result: expected None"
        )
    return None


def reload(module):
    return _MOLT_IMPORTLIB_RELOAD(module, util, machinery, import_module)


globals().pop("_require_intrinsic", None)
