"""Minimal test.support.import_helper helpers for Molt (partial)."""

from __future__ import annotations

from typing import Iterable, cast
import contextlib
import importlib
import importlib.util
import os
import sys
from types import ModuleType
import unittest
import warnings

from .os_helper import unlink


# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): extend import_helper coverage (frozen modules, subinterpreters, and script helpers).


@contextlib.contextmanager
def _ignore_deprecated_imports(ignore: bool = True):
    if ignore:
        with warnings.catch_warnings():
            warnings.filterwarnings("ignore", ".+ (module|package)", DeprecationWarning)
            yield
    else:
        yield


def unload(name: str) -> None:
    try:
        del sys.modules[name]
    except KeyError:
        pass


def forget(modname: str) -> None:
    unload(modname)
    for dirname in sys.path:
        source = os.path.join(dirname, f"{modname}.py")
        unlink(source + "c")
        try:
            for opt in ("", 1, 2):
                unlink(importlib.util.cache_from_source(source, optimization=opt))
        except ValueError:
            continue


def import_module(
    name: str, deprecated: bool = False, *, required_on: Iterable[str] = ()
):
    with _ignore_deprecated_imports(deprecated):
        try:
            return importlib.import_module(name)
        except ImportError as exc:
            if sys.platform.startswith(tuple(required_on)):
                raise
            raise unittest.SkipTest(str(exc)) from exc


def _save_and_remove_modules(names: Iterable[str]) -> dict[str, object]:
    orig_modules: dict[str, object] = {}
    prefixes = tuple(f"{name}." for name in names)
    for modname in list(sys.modules):
        if modname in names or modname.startswith(prefixes):
            orig_modules[modname] = sys.modules.pop(modname)
    return orig_modules


def import_fresh_module(
    name: str,
    fresh: Iterable[str] = (),
    blocked: Iterable[str] = (),
    *,
    deprecated: bool = False,
    usefrozen: bool = False,
):
    del usefrozen
    with _ignore_deprecated_imports(deprecated):
        fresh_list = list(fresh)
        blocked_list = list(blocked)
        names = {name, *fresh_list, *blocked_list}
        orig_modules = _save_and_remove_modules(names)
        for modname in blocked_list:
            sys.modules[modname] = cast(ModuleType, None)
        try:
            try:
                for modname in fresh_list:
                    __import__(modname)
            except ImportError:
                return None
            return importlib.import_module(name)
        finally:
            _save_and_remove_modules(names)
            sys.modules.update(orig_modules)


class CleanImport:
    def __init__(self, *module_names: str, usefrozen: bool = False) -> None:
        del usefrozen
        self._original_modules = sys.modules.copy()
        for module_name in module_names:
            if module_name in sys.modules:
                module = sys.modules[module_name]
                if getattr(module, "__name__", module_name) != module_name:
                    sys.modules.pop(getattr(module, "__name__", module_name), None)
                sys.modules.pop(module_name, None)

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        sys.modules.clear()
        sys.modules.update(self._original_modules)
        return False


__all__ = [
    "CleanImport",
    "forget",
    "import_fresh_module",
    "import_module",
    "unload",
]
