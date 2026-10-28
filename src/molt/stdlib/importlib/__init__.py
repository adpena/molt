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


import sys

import importlib.machinery as machinery
import importlib.util as util
import importlib._bootstrap as _bootstrap  # noqa: F401
import importlib._bootstrap_external as _bootstrap_external  # noqa: F401


def invalidate_caches() -> None:
    import sys

    for finder in sys.meta_path:
        if hasattr(finder, "invalidate_caches"):
            finder.invalidate_caches()


# Active reload namespace ownership is Python state, as in CPython. It is
# distinct from Rust's transient synchronization-only execution lock records.
_RELOADING = {}


def reload(module):
    try:
        name = module.__spec__.name
    except AttributeError:
        try:
            name = module.__name__
        except AttributeError:
            raise TypeError("reload() argument must be a module") from None
    if sys.modules.get(name) is not module:
        raise ImportError(f"module {name} not in sys.modules", name=name)
    if name in _RELOADING:
        return _RELOADING[name]
    _RELOADING[name] = module
    try:
        parent_name = name.rpartition(".")[0]
        if parent_name:
            try:
                parent = sys.modules[parent_name]
            except KeyError:
                raise ImportError(
                    f"parent {parent_name!r} not in sys.modules", name=parent_name
                ) from None
            else:
                package_path = parent.__path__
        else:
            package_path = None
        spec = module.__spec__ = _bootstrap._find_spec(name, package_path, module)
        if spec is None:
            raise ModuleNotFoundError(
                f"spec not found for the module {name!r}", name=name
            )
        _MOLT_IMPORTLIB_RELOAD(module, spec, spec.name)
        return sys.modules[name]
    finally:
        try:
            del _RELOADING[name]
        except KeyError:
            pass


globals().pop("_require_intrinsic", None)
