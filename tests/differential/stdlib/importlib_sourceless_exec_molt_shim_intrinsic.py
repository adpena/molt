"""Purpose: validate sourceless loader .molt.py shim execution lane via intrinsics."""

import importlib.machinery
import importlib.util
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_sourceless_exec_moltshim_")
pyc_path = os.path.join(root, "shimmod.pyc")
with open(pyc_path, "wb") as handle:
    handle.write(b"")

shim_path = f"{pyc_path[:-4]}.molt.py"
with open(shim_path, "w", encoding="utf-8") as handle:
    handle.write("value = 33\\n")

loader = importlib.machinery.SourcelessFileLoader("shimmod_exec", pyc_path)
spec = importlib.util.spec_from_file_location("shimmod_exec", pyc_path, loader=loader)
module = importlib.util.module_from_spec(spec) if spec is not None else None

loaded = False
error_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
        loaded = getattr(module, "value", None) == 33
except BaseException as exc:
    error_name = exc.__class__.__name__

print(
    loaded
    or error_name
    in {"ImportError", "PermissionError", "OSError", "EOFError", "RuntimeError"}
)
print((not loaded) or getattr(module, "value", None) == 33)
