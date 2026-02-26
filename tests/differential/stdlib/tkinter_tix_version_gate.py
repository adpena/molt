"""Purpose: assert CPython version-gated import behavior for tkinter.tix."""

import importlib
import sys

if sys.version_info >= (3, 13):
    try:
        importlib.import_module("tkinter.tix")
    except ModuleNotFoundError:
        print("tkinter_tix_absent", tuple(sys.version_info[:3]))
    else:
        raise AssertionError("tkinter.tix must be absent for Python >= 3.13")
else:
    print("tkinter_tix_gate_not_applicable", tuple(sys.version_info[:3]))
