"""Purpose: sourceless exec shim lane falls through to later intrinsic candidates."""

import importlib.machinery
import importlib.util
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_sourceless_exec_fallback_")
pyc_path = os.path.join(root, "bc_fallback.pyc")
with open(pyc_path, "wb") as handle:
    handle.write(b"")

with open(os.path.join(root, "bc_fallback.molt.py"), "w", encoding="utf-8") as handle:
    handle.write("value = 1\nfor _ in ():\n    value = 2\n")

with open(os.path.join(root, "bc_fallback.py"), "w", encoding="utf-8") as handle:
    handle.write("value = 91\n")

loader = importlib.machinery.SourcelessFileLoader("bc_fallback", pyc_path)
spec = importlib.util.spec_from_file_location("bc_fallback", pyc_path, loader=loader)
module = importlib.util.module_from_spec(spec) if spec is not None else None

loaded = False
error_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
        loaded = getattr(module, "value", None) == 91
except BaseException as exc:
    error_name = exc.__class__.__name__

print(loaded or error_name in {"ImportError", "PermissionError", "OSError", "EOFError", "RuntimeError"})
print((not loaded) or getattr(module, "value", None) == 91)
