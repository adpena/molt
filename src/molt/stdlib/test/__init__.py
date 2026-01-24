"""CPython test harness stubs for Molt."""

from __future__ import annotations

import os
from pathlib import Path
import importlib


def _extend_cpython_test_path() -> None:
    cpython_dir = os.environ.get("MOLT_REGRTEST_CPYTHON_DIR")
    if not cpython_dir:
        return
    test_root = Path(cpython_dir) / "Lib" / "test"
    if not test_root.exists():
        return
    test_path = str(test_root.resolve())
    path_list = globals().get("__path__")
    if path_list is None or test_path in path_list:
        return
    path_list.insert(0, test_path)


_extend_cpython_test_path()


def _load_test_module(name: str, fallback: str):
    _extend_cpython_test_path()
    try:
        return importlib.import_module(name)
    except ModuleNotFoundError:
        return importlib.import_module(f"{__name__}.{fallback}")


list_tests = _load_test_module("test.list_tests", "list_tests")
seq_tests = _load_test_module("test.seq_tests", "seq_tests")
support = _load_test_module("test.support", "support")
import_helper = _load_test_module("test.support.import_helper", "import_helper")
os_helper = _load_test_module("test.support.os_helper", "os_helper")
warnings_helper = _load_test_module("test.warnings_helper", "warnings_helper")

__all__ = [
    "import_helper",
    "list_tests",
    "os_helper",
    "seq_tests",
    "support",
    "warnings_helper",
]
