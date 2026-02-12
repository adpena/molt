"""Purpose: ensure restricted sourceless shim errors surface as ImportError lane."""

import importlib.machinery
import importlib.util
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_sourceless_restricted_shim_")
pyc_path = os.path.join(root, "bc_restricted.pyc")
with open(pyc_path, "wb") as handle:
    handle.write(b"")

source_path = os.path.join(root, "bc_restricted.py")
with open(source_path, "w", encoding="utf-8") as handle:
    handle.write("value = 1\nfor _ in ():\n    value = 2\n")

loader = importlib.machinery.SourcelessFileLoader("bc_restricted", pyc_path)
spec = importlib.util.spec_from_file_location("bc_restricted", pyc_path, loader=loader)
module = importlib.util.module_from_spec(spec) if spec is not None else None

error_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
except BaseException as exc:
    error_name = exc.__class__.__name__

print(error_name in {"ImportError", "PermissionError", "OSError", "EOFError", "RuntimeError"})
print(error_name != "NotImplementedError")
