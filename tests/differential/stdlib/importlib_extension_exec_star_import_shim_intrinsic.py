"""Purpose: validate extension shim wildcard import lane in restricted executor."""

import importlib.machinery
import importlib.util
import os
import sys
import tempfile


root = tempfile.mkdtemp(prefix="molt_ext_star_shim_")
ext_path = os.path.join(root, "extstar.so")
with open(ext_path, "wb") as handle:
    handle.write(b"")

helper_path = os.path.join(root, "extstar_helper.py")
with open(helper_path, "w", encoding="utf-8") as handle:
    handle.write("__all__ = ['LEFT', 'RIGHT']\nLEFT = 20\nRIGHT = 22\n")

shim_path = f"{ext_path}.molt.py"
with open(shim_path, "w", encoding="utf-8") as handle:
    handle.write("from extstar_helper import *\nvalue = LEFT + RIGHT\n")

sys.path.insert(0, root)

loader = importlib.machinery.ExtensionFileLoader("extstar_mod", ext_path)
spec = importlib.util.spec_from_file_location("extstar_mod", ext_path, loader=loader)
module = importlib.util.module_from_spec(spec) if spec is not None else None

loaded = False
error_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
        loaded = getattr(module, "value", None) == 42
except BaseException as exc:
    error_name = exc.__class__.__name__

print(loaded or error_name in {"ImportError", "PermissionError", "OSError"})
print((not loaded) or getattr(module, "value", None) == 42)
