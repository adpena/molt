"""Purpose: validate sourceless shim wildcard import lane in restricted executor."""

import importlib.machinery
import importlib.util
import os
import sys
import tempfile


root = tempfile.mkdtemp(prefix="molt_sourceless_star_shim_")
pyc_path = os.path.join(root, "bcstar.pyc")
with open(pyc_path, "wb") as handle:
    handle.write(b"")

helper_path = os.path.join(root, "bcstar_helper.py")
with open(helper_path, "w", encoding="utf-8") as handle:
    handle.write("__all__ = ['A', 'B']\nA = 5\nB = 7\n")

source_path = os.path.join(root, "bcstar.py")
with open(source_path, "w", encoding="utf-8") as handle:
    handle.write("from bcstar_helper import *\nvalue = A * B\n")

sys.path.insert(0, root)

loader = importlib.machinery.SourcelessFileLoader("bcstar_mod", pyc_path)
spec = importlib.util.spec_from_file_location("bcstar_mod", pyc_path, loader=loader)
module = importlib.util.module_from_spec(spec) if spec is not None else None

loaded = False
error_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
        loaded = getattr(module, "value", None) == 35
except BaseException as exc:
    error_name = exc.__class__.__name__

print(
    loaded
    or error_name
    in {"ImportError", "PermissionError", "OSError", "EOFError", "RuntimeError"}
)
print((not loaded) or getattr(module, "value", None) == 35)
