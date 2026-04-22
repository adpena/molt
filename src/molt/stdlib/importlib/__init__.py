"""Minimal import support for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


import builtins as _builtins
import os as _os
import sys as _sys

import importlib.machinery as machinery
import importlib.util as util

_require_intrinsic("molt_stdlib_probe")
_MOLT_IMPORTLIB_RESOLVE_NAME = _require_intrinsic("molt_importlib_resolve_name")
_MOLT_IMPORTLIB_KNOWN_ABSENT_MISSING_NAME = _require_intrinsic(
    "molt_importlib_known_absent_missing_name"
)
_MOLT_IMPORTLIB_RUNTIME_MODULES = _require_intrinsic("molt_importlib_runtime_modules")
_MOLT_IMPORTLIB_INVALIDATE_CACHES = _require_intrinsic(
    "molt_importlib_invalidate_caches"
)
_MOLT_IMPORTLIB_RELOAD = _require_intrinsic("molt_importlib_reload")
_MODULE_ALIASES: dict[str, str] = {}


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


def _runtime_modules() -> dict[str, object]:
    modules = _MOLT_IMPORTLIB_RUNTIME_MODULES()
    if not isinstance(modules, dict):
        raise RuntimeError("invalid importlib runtime state payload: modules")
    return modules


def _canonical_codecs_file(path: object) -> object:
    if not isinstance(path, str):
        return path
    marker = "/cpython-3.12."
    idx = path.find(marker)
    if idx < 0:
        return path
    suffix = path[idx + len(marker) :]
    dash = suffix.find("-")
    if dash < 0:
        return path
    candidate = path[:idx] + "/cpython-3.12-" + suffix[dash + 1 :]
    if _os.path.exists(candidate):
        return candidate
    return path


def import_module(name: str, package: str | None = None):
    # CPython parity: importlib.import_module('.x', non_str_package) raises
    # TypeError("__package__ not set to a string").
    if (
        isinstance(name, str)
        and name.startswith(".")
        and package is not None
        and not isinstance(package, str)
    ):
        raise TypeError("__package__ not set to a string")

    resolved = _MOLT_IMPORTLIB_RESOLVE_NAME(name, package)
    # CPython removed tkinter.tix in 3.13 (deprecated since 3.6).
    if resolved == "tkinter.tix" and _sys.version_info >= (3, 13):
        raise ModuleNotFoundError("No module named 'tkinter.tix'")
    # `encodings.oem` is intentionally unavailable on non-Windows when codecs
    # does not expose OEM helpers; raise at the importlib boundary so callers
    # see CPython-shaped ImportError semantics.
    if resolved == "encodings.oem" and _sys.platform != "win32":
        import codecs as _codecs

        if not hasattr(_codecs, "oem_encode"):
            raise ImportError(
                "cannot import name 'oem_encode' from 'codecs' "
                f"({_canonical_codecs_file(getattr(_codecs, '__file__', None))})"
            )

    missing_name = _MOLT_IMPORTLIB_KNOWN_ABSENT_MISSING_NAME(resolved)
    if missing_name is not None:
        raise ModuleNotFoundError(f"No module named '{missing_name}'")
    alias = _MODULE_ALIASES.get(resolved)
    if alias is not None:
        target = import_module(alias)
        modules = _runtime_modules()
        modules[resolved] = target
        return target
    try:
        mod = _builtins.__import__(resolved, globals(), locals(), ("*",), 0)
    except BaseException as exc:
        text = str(exc)
        kind = type(exc).__name__
        if kind == "ModuleNotFoundError" or text.startswith("No module named "):
            raise ModuleNotFoundError(text)
        if kind == "ImportError":
            raise ImportError(text)
        raise
    modules = _runtime_modules()
    if resolved in modules:
        return modules[resolved]
    if mod is not None:
        return mod
    raise ModuleNotFoundError(f"No module named '{resolved}'")


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
