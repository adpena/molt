"""CPython test harness stubs for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import os
from pathlib import Path
import importlib

_require_intrinsic("molt_capabilities_has", globals())


def _extend_cpython_test_path() -> None:
    cpython_dir = os.environ.get("MOLT_REGRTEST_CPYTHON_DIR")
    if not cpython_dir:
        return
    test_root = Path(cpython_dir) / "Lib" / "test"
    if not test_root.exists():
        return
    test_path = str(test_root.resolve())
    path_list = globals().get("__path__")
    if path_list is None:
        path_list = [str(Path(__file__).resolve().parent)]
        globals()["__path__"] = path_list
    if test_path in path_list:
        return
    path_list.insert(0, test_path)


_extend_cpython_test_path()


def _load_test_module(name: str, fallback: str):
    _extend_cpython_test_path()
    try:
        return importlib.import_module(name)
    except (ModuleNotFoundError, NotImplementedError):
        return importlib.import_module(f"{__name__}.{fallback}")


_MODULE_ALIASES = {
    "list_tests": ("test.list_tests", "list_tests"),
    "seq_tests": ("test.seq_tests", "seq_tests"),
    "support": ("test.support", "support"),
    "import_helper": ("test.support.import_helper", "support.import_helper"),
    "os_helper": ("test.support.os_helper", "support.os_helper"),
    "warnings_helper": ("test.support.warnings_helper", "support.warnings_helper"),
}


def __getattr__(name: str):
    target = _MODULE_ALIASES.get(name)
    if target is None:
        raise AttributeError(name)
    module = _load_test_module(*target)
    globals()[name] = module
    return module


__all__ = [
    "import_helper",
    "list_tests",
    "os_helper",
    "seq_tests",
    "support",
    "warnings_helper",
]
