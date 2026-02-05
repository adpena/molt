"""Minimal test.support.import_helper helpers for Molt (partial)."""

from __future__ import annotations

from typing import Iterable, cast
import contextlib
import importlib
import importlib.util
import os
import shutil
import sys
from types import ModuleType
import unittest
import warnings

from .os_helper import temp_dir, unlink


# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): extend
# import_helper coverage (extension loader helpers, importlib.machinery parity, and
# script helper utilities beyond ready_to_import).


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


def make_legacy_pyc(source: str) -> str:
    pyc_file = importlib.util.cache_from_source(source)
    legacy_pyc = source + "c"
    shutil.move(pyc_file, legacy_pyc)
    return legacy_pyc


def _make_script(dirname: str, name: str, source: str) -> str:
    filename = f"{name}.py"
    path = os.path.join(dirname, filename)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(source)
        if source and not source.endswith("\n"):
            handle.write("\n")
    return path


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


@contextlib.contextmanager
def frozen_modules(enabled: bool = True):
    try:
        import _imp as _imp_mod
    except Exception:
        _imp_mod = None
    if _imp_mod is None or not hasattr(_imp_mod, "_override_frozen_modules_for_tests"):
        yield
        return
    _imp_mod._override_frozen_modules_for_tests(1 if enabled else -1)
    try:
        yield
    finally:
        _imp_mod._override_frozen_modules_for_tests(0)


@contextlib.contextmanager
def multi_interp_extensions_check(enabled: bool = True):
    try:
        import _imp as _imp_mod
    except Exception:
        _imp_mod = None
    override = getattr(_imp_mod, "_override_multi_interp_extensions_check", None)
    if _imp_mod is None or override is None:
        yield
        return
    old = override(1 if enabled else -1)
    try:
        yield
    finally:
        override(old)


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
            with frozen_modules(False):
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


class DirsOnSysPath:
    """Context manager to temporarily add directories to sys.path."""

    def __init__(self, *paths: str) -> None:
        self._original_value = sys.path[:]
        self._original_object = sys.path
        sys.path.extend(paths)

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        sys.path = self._original_object
        sys.path[:] = self._original_value
        return False


def modules_setup():
    return (sys.modules.copy(),)


def modules_cleanup(oldmodules):
    encodings = [
        (name, module)
        for name, module in sys.modules.items()
        if name.startswith("encodings.")
    ]
    sys.modules.clear()
    sys.modules.update(encodings)
    sys.modules.update(oldmodules)


@contextlib.contextmanager
def isolated_modules():
    (saved,) = modules_setup()
    try:
        yield
    finally:
        modules_cleanup(saved)


@contextlib.contextmanager
def ready_to_import(name: str | None = None, source: str = ""):
    name = name or "spam"
    with temp_dir() as tempdir:
        path = _make_script(tempdir, name, source)
        old_module = sys.modules.pop(name, None)
        sys.path.insert(0, tempdir)
        try:
            yield name, path
        finally:
            try:
                sys.path.remove(tempdir)
            except ValueError:
                pass
            if old_module is not None:
                sys.modules[name] = old_module
            else:
                sys.modules.pop(name, None)


__all__ = [
    "CleanImport",
    "DirsOnSysPath",
    "forget",
    "frozen_modules",
    "import_fresh_module",
    "import_module",
    "isolated_modules",
    "make_legacy_pyc",
    "multi_interp_extensions_check",
    "modules_cleanup",
    "modules_setup",
    "ready_to_import",
    "unload",
]
